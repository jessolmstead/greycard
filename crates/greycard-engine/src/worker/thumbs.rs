//! Thumbnails: the cache on disk, and making one when it misses.

use super::*;

/// How [`thumbnail`] makes a picture, as the cache's entries record
/// it in their names: raise it when the picture it makes changes (the
/// downscale, the turn, the choice of preview, or the fallback
/// develop through `DevelopSettings::default()`, whose definition in
/// core names this constant), and every entry made the old way is a
/// miss and is made again. The long edge is in the key already, so a
/// change of the sizes made (`made::MADE`) needs no bump.
pub const THUMB_RECIPE: u16 = 5;

/// How a picture file's thumbnails and local previews (a JPEG, PNG or
/// TIFF, not a raw) are made, past [`THUMB_RECIPE`]: raise it when
/// theirs change and a raw's do not, and only their entries are made
/// again. It sits in the recipe's high byte, [`THUMB_RECIPE`] in the
/// low one, so a raise of that remakes theirs too; the top bit is the
/// edited pictures' (`edited_pictures::EDITED_RECIPE`).
///
/// 1: the embedded profile read, 2026-11-24. Before it a picture in a
/// space wider than sRGB was shown as if in sRGB.
pub const PICTURE_STEP: u16 = 1;

const _: () = assert!(THUMB_RECIPE < 0x100 && PICTURE_STEP < 0x80);

/// The recipe the file at `path`'s thumbnails and local preview are
/// kept under: [`THUMB_RECIPE`] for a raw, with [`PICTURE_STEP`] over
/// it for a picture.
pub fn thumb_recipe(path: &std::path::Path) -> u16 {
    if is_picture_path(path) {
        THUMB_RECIPE | PICTURE_STEP << 8
    } else {
        THUMB_RECIPE
    }
}

/// The recipe an entry of the file at `path` may also be read under
/// when its own misses and the file is out of reach: a picture's
/// entries kept before [`PICTURE_STEP`] were [`THUMB_RECIPE`]'s, and a
/// frame on a drive unplugged since shows the picture it had rather
/// than nothing until the drive is back. None for a raw, and none while
/// the file can be read, whose entry is made again the new way. A stat
/// of the file, asked only after the miss.
pub fn earlier_recipe(path: &std::path::Path) -> Option<u16> {
    (is_picture_path(path) && file_stat(path).is_none()).then_some(THUMB_RECIPE)
}

/// How much larger than a thumbnail the preview it is made from is
/// asked to be, where the file has several to choose from: from five
/// times the size, the whole-factor downscale gives at least five
/// sixths of it, and every size made (`made::MADE`) comes from the
/// 720 or 2112 pixel preview of a camera whose DNG lists a 160 px
/// thumbnail first, not its full-size one.
const PREVIEW_OVER: u32 = 5;

/// Count what `cache` holds on a thread of its own, walking the disk
/// without its lock, and hand `then` what is known after: the count
/// seeded, or the one some write had already made. A walk over a few
/// hundred thousand entries takes a second or more, which is not the
/// window's to wait through. Nothing to count is `None`.
pub fn count_thumb_cache(
    cache: &ThumbCache,
    then: impl FnOnce(Option<greycard_library::thumbs::Usage>) + Send + 'static,
) {
    let cache = cache.clone();
    let spawned = std::thread::Builder::new()
        .name("thumbnail cache count".into())
        .spawn(move || {
            let root = cache
                .lock()
                .expect("thumbnail cache")
                .as_ref()
                .map(|c| (c.root().to_path_buf(), c.previews_from()));
            let Some((root, previews_from)) = root else {
                return then(None);
            };
            // The thumbnails and the previews apart, each under its cap.
            let counted = greycard_library::thumbs::split_at(&root, previews_from);
            let known = cache
                .lock()
                .expect("thumbnail cache")
                .as_mut()
                .and_then(|c| {
                    c.seed_split(counted);
                    c.known_usage()
                });
            then(known);
        });
    if let Err(e) = spawned {
        tracing::warn!("thumbnail cache not counted: {e}");
    }
}

/// A file's length and modification time in nanoseconds, the two a
/// change to it past its first 64 KB shows in.
pub fn file_stat(path: &std::path::Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    Some((meta.len(), mtime))
}

/// What a cache entry for the file at `path` of this stat must have
/// been made under: its recipe ([`thumb_recipe`]) and the file's
/// modification time. The hash reads
/// the head and the length, and a file can change past its head with
/// neither moving: a picture re-exported (an uncompressed TIFF
/// retouched in its lower half), a DNG's previews or orientation
/// rewritten in place (DNGLab's IFD0 is at the end of the file), and a
/// raw still being copied by a copier that set its length first —
/// Windows' CopyFile, `rsync --preallocate`, many card importers — whose
/// preview is half zeros and decodes mostly grey. The time tells all
/// three apart. A move or rename keeps it; the cost is one more making
/// of each thumbnail after a copy that does not keep it (`cp` without
/// `-p`, a drag to another disk on some desktops).
pub fn thumb_tag(path: &std::path::Path, stat: (u64, u64)) -> Tag {
    Tag {
        recipe: thumb_recipe(path),
        stamp: stat.1,
    }
}

/// What a file's thumbnails are kept under, its content hash and the
/// stamp of [`thumb_tag`], for taking them out of the cache when the
/// file is deleted: read before it goes, since neither can be after.
pub fn thumb_key(path: &std::path::Path) -> Option<(String, u64)> {
    let stat = file_stat(path)?;
    let hash = greycard_library::hash_file(path).ok()?;
    Some((hash, thumb_tag(path, stat).stamp))
}

/// The file's stat and cache key, when the cache is on and the file
/// hashes, and the entry kept under that key if any. What a lookup
/// finds is noted with `previews`, which owes the file a local preview
/// when none is kept under that key: the thumbnail's hash, so the
/// preview reads no second head. A lookup alone, with no making after
/// it, is what the thumbnails' threads do while a develop holds them,
/// since a hit costs a tenth of a millisecond.
///
/// A frame `edited` says shows its edit is looked up first under its
/// develop key, and the camera's entry stands in on a miss: the
/// picture made from the edit, when one is kept, is the frame's
/// thumbnail at every size.
#[allow(clippy::type_complexity)]
pub(super) fn thumb_lookup(
    cache: &ThumbCache,
    frame: &Frame,
    size: u32,
    previews: Option<&crate::previews::Previews>,
    edited: Option<&crate::edited_pictures::Edited>,
) -> (Option<(u64, u64)>, Option<(String, Tag)>, Option<Thumb>) {
    let path = frame.path.as_path();
    let on = cache
        .lock()
        .expect("thumbnail cache")
        .as_ref()
        .is_some_and(|c| c.cap() > 0);
    let before = if on { file_stat(path) } else { None };
    let key = before.and_then(|stat| match greycard_library::hash_file(path) {
        Ok(hash) => Some((hash, thumb_tag(path, stat))),
        Err(e) => {
            tracing::debug!("thumbnail {}: not hashed: {e}", path.display());
            None
        }
    });
    let from_edit = key.as_ref().and_then(|(hash, _)| {
        let edited = edited?;
        edited.lookup(hash, size, edited.shows(frame))
    });
    let hit = from_edit.or_else(|| {
        key.as_ref()
            .and_then(|(hash, tag)| {
                cache
                    .lock()
                    .expect("thumbnail cache")
                    .as_mut()
                    .and_then(|c| c.get(hash, size, *tag))
            })
            .map(|t| cut_to_camera(t, path, edited, false))
    });
    if let (Some(previews), Some(stat), Some((hash, _))) = (previews, before, key.as_ref()) {
        previews.note(path, stat, hash);
    }
    (before, key, hit)
}

/// A file's thumbnail from the cache when it holds one for the file's
/// content, else made by `make` (the pool's is [`thumbnail`]) and kept,
/// the lookup noted with `previews`. The lookup costs a stat and the
/// content hash, a read of the file's first 64 KB, and nothing else of
/// the file; a cache that cannot be read or written is a miss and a
/// thumbnail made, never an error of its own. Says whether it came
/// from the cache. The file is stat'd before it is hashed and again
/// after the picture is made, and a picture made while the file was
/// changing — its length or its time moved — is shown but not kept: it
/// may be of a file half written, and kept under the key the finished
/// file will have.
pub(super) fn cached_thumbnail_noting(
    cache: &ThumbCache,
    previews: Option<&crate::previews::Previews>,
    edited: Option<&crate::edited_pictures::Edited>,
    frame: &Frame,
    size: u32,
    make: impl FnOnce(&std::path::Path, u32) -> anyhow::Result<(u32, u32, Vec<u8>)>,
) -> anyhow::Result<(Thumb, bool)> {
    let path = frame.path.as_path();
    let (before, key, hit) = thumb_lookup(cache, frame, size, previews, edited);
    if let Some(thumb) = hit {
        return Ok((thumb, true));
    }
    let (width, height, rgb) = make(path, size)?;
    let thumb = Thumb { width, height, rgb };
    let shown = cut_to_camera(thumb.clone(), path, edited, true);
    if let Some((hash, tag)) = key {
        if file_stat(path) != before {
            tracing::debug!(
                "thumbnail {}: the file changed while it was made; not cached",
                path.display()
            );
        } else if let Some(c) = cache.lock().expect("thumbnail cache").as_mut()
            && let Err(e) = c.put(&hash, size, tag, &thumb)
        {
            tracing::debug!("thumbnail {}: not cached: {e}", path.display());
        }
    }
    Ok((shown, false))
}

/// The camera's picture of the file at `path` cut to the camera's
/// frame, where the camera framed less of it than the raw keeps (an
/// aspect chosen on the body): what a view of a frame with no edit
/// shows, as its develop is framed. The cache keeps the camera's
/// picture whole; the cut is made as it is handed out, so a frame
/// cropped by hand to the whole sensor still has its whole picture.
/// The frame is the one the index's row told, or, with `read` (the
/// file was read for the picture), read from the file's head once; a
/// picture from the cache alone of a frame whose row is not read yet
/// stays whole until the row is, and is asked for again then. A
/// picture not of the sensor's shape (one the camera cut already)
/// stays as it is (`CameraFrame::cut_shown`).
pub fn cut_to_camera(
    thumb: Thumb,
    path: &std::path::Path,
    edited: Option<&crate::edited_pictures::Edited>,
    read: bool,
) -> Thumb {
    if edited.is_none() {
        return thumb;
    }
    let camera = if read {
        crate::edited_pictures::camera_of(path)
    } else {
        crate::edited_pictures::camera_known(path)
    };
    let Some(frame) = camera else {
        return thumb;
    };
    let (width, height, rgb) = frame.cut_shown(thumb.width, thumb.height, &thumb.rgb, 3);
    Thumb { width, height, rgb }
}

/// A small sRGB rendering of a file, `size` on its long edge: the
/// camera's own preview JPEG when the file has one, box downscaled
/// and turned as the camera says; else a bilinear demosaic, box
/// downscaled.
pub fn thumbnail(path: &std::path::Path, size: u32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let size = size.max(1) as usize;
    if is_picture_path(path) {
        // The picture itself, taken to sRGB by its embedded profile.
        // The tag and the profile are the picture module's to read,
        // since that is the one the develop reads them by and this
        // thumbnail has to agree with it.
        let orientation = greycard_core::picture::orientation_path(path)?;
        let mut decoder = image::ImageReader::open(path)?
            .with_guessed_format()?
            .into_decoder()?;
        let icc = image::ImageDecoder::icc_profile(&mut decoder)
            .ok()
            .flatten();
        let to_srgb = greycard_core::picture::ToSrgb::of_profile(icc.as_deref())?;
        let decoded = image::DynamicImage::from_decoder(decoder)?;
        return Ok(picture_thumbnail(&decoded, &to_srgb, orientation, size));
    }
    // Neither read maps the file: the pool walks every frame of a
    // folder, a share's among them, and a mapped file whose volume
    // goes away is a SIGBUS. Of a file with several previews, one at
    // least `PREVIEW_OVER` times the size: the downscale is by a whole
    // factor, which from a preview nearer the size leaves the
    // thumbnail well short of it.
    let preview = greycard_core::decode::preview_path_at_least(
        path,
        (size as u32).saturating_mul(PREVIEW_OVER),
    );
    if let Ok(Some((preview, orientation))) = preview {
        return Ok(thumbnail_of_preview(&preview, orientation, size));
    }
    let frame = greycard_core::decode::decode_path_unmapped(path)?;
    let settings = DevelopSettings {
        demosaic: DemosaicMethod::Bilinear,
        chromatic_aberration: None,
        ..Default::default()
    };
    let image = develop(&frame, &settings)?.image;
    // The long edge, as the preview path takes it: dividing the
    // width alone gives a portrait frame half as much again as was
    // asked for, and the memory with it.
    let factor = image.width.max(image.height).div_ceil(size).max(1);
    let (w, h) = (image.width / factor, image.height / factor);
    let to_srgb = greycard_core::color::WORKING_SPACE
        .to_space_matrix(&RgbSpace::SRGB, greycard_core::color::CAT)
        .expect("working space to sRGB")
        .rows
        .map(|row| row.map(|v| v as f32));
    let mut rgb = Vec::with_capacity(w * h * 3);
    for ty in 0..h {
        for tx in 0..w {
            let mut sum = [0.0f32; 3];
            for y in ty * factor..(ty + 1) * factor {
                for x in tx * factor..(tx + 1) * factor {
                    let i = (y * image.width + x) * 3;
                    for (s, v) in sum.iter_mut().zip(&image.data[i..i + 3]) {
                        *s += v.min(1.0);
                    }
                }
            }
            let n = (factor * factor) as f32;
            let lin = sum.map(|s| s / n);
            for row in to_srgb {
                let v = (row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]).clamp(0.0, 1.0);
                rgb.push((srgb_encode(v) * 255.0).round() as u8);
            }
        }
    }
    Ok((w as u32, h as u32, rgb))
}

/// The long edge a thumbnail is made at for the filmstrip, whose
/// items are 178 wide. The grid asks for more when its cells are
/// larger.
pub const THUMB_WIDTH: u32 = 170;

/// The camera's preview, box downscaled to `size` on its long edge
/// and turned by the engine's own orientation so it agrees with the
/// developed picture.
fn thumbnail_of_preview(
    preview: &image::RgbImage,
    orientation: greycard_core::raw::Orientation,
    size: usize,
) -> (u32, u32, Vec<u8>) {
    let (w, h) = (preview.width() as usize, preview.height() as usize);
    let small = box_down(preview.as_raw(), w, h, size, 255.0);
    turned_rgb8(small, orientation)
}

/// A picture file's thumbnail, its samples taken to sRGB as `to_srgb`
/// says. Samples sRGB's already are the camera preview's way, and come
/// out as they did before profiles were read. The others are taken to
/// sRGB after the downscale, a few thousand pixels rather than the
/// whole picture: the box averages encoded samples, as it does a
/// camera's sRGB preview, and the average, still a float, goes through
/// the file's curve and the matrix. A 16-bit file is averaged at its
/// own depth so a curve steep at the foot is not read through 8 bits;
/// a float file's samples are linear, as the develop takes them, and
/// are averaged so.
fn picture_thumbnail(
    decoded: &image::DynamicImage,
    to_srgb: &greycard_core::picture::ToSrgb,
    orientation: greycard_core::raw::Orientation,
    size: usize,
) -> (u32, u32, Vec<u8>) {
    use greycard_core::picture::{is_float, is_sixteen};
    let float = is_float(decoded);
    if to_srgb.is_srgb() && !float {
        return thumbnail_of_preview(&decoded.to_rgb8(), orientation, size);
    }
    let (w, h) = (decoded.width() as usize, decoded.height() as usize);
    let mut small = if float {
        box_down(decoded.to_rgb32f().as_raw(), w, h, size, 1.0)
    } else if is_sixteen(decoded) {
        box_down(decoded.to_rgb16().as_raw(), w, h, size, 65535.0)
    } else {
        box_down(decoded.to_rgb8().as_raw(), w, h, size, 255.0)
    };
    for p in small.data.as_chunks_mut::<3>().0 {
        *p = if float {
            to_srgb.linear(*p)
        } else {
            to_srgb.encoded(*p)
        };
    }
    turned_rgb8(small, orientation)
}

/// Interleaved RGB samples of a `pw` by `ph` picture box downscaled by
/// a whole factor to `size` on the long side, each average over `max`.
fn box_down<T: Summed>(px: &[T], pw: usize, ph: usize, size: usize, max: f32) -> WorkingImage {
    // The long side, as the develop's turned picture may be portrait.
    let factor = pw.max(ph).div_ceil(size).max(1);
    let (w, h) = ((pw / factor).max(1), (ph / factor).max(1));
    let mut data = Vec::with_capacity(w * h * 3);
    for ty in 0..h {
        for tx in 0..w {
            let mut sum = [T::Sum::default(); 3];
            for y in ty * factor..(ty + 1) * factor {
                for x in tx * factor..(tx + 1) * factor {
                    let i = (y * pw + x) * 3;
                    for (s, v) in sum.iter_mut().zip(&px[i..i + 3]) {
                        *s += v.widen();
                    }
                }
            }
            let n = (factor * factor) as f32;
            data.extend(sum.map(|s| T::mean(s, n) / max));
        }
    }
    WorkingImage {
        width: w,
        height: h,
        data,
    }
}

/// A sample [`box_down`] sums: an integer exactly, in an integer, as
/// the camera previews' 8-bit samples always were, and a float in a
/// double.
trait Summed: Copy {
    type Sum: Copy + Default + std::ops::AddAssign;
    fn widen(self) -> Self::Sum;
    /// The sum over `n` samples, their mean.
    fn mean(sum: Self::Sum, n: f32) -> f32;
}

impl Summed for u8 {
    type Sum = u32;
    fn widen(self) -> u32 {
        self.into()
    }
    fn mean(sum: u32, n: f32) -> f32 {
        sum as f32 / n
    }
}

impl Summed for u16 {
    type Sum = u64;
    fn widen(self) -> u64 {
        self.into()
    }
    fn mean(sum: u64, n: f32) -> f32 {
        sum as f32 / n
    }
}

impl Summed for f32 {
    type Sum = f64;
    fn widen(self) -> f64 {
        self.into()
    }
    fn mean(sum: f64, n: f32) -> f32 {
        sum as f32 / n
    }
}

/// Encoded samples, 0 to 1, turned and brought to 8 bits.
fn turned_rgb8(
    small: WorkingImage,
    orientation: greycard_core::raw::Orientation,
) -> (u32, u32, Vec<u8>) {
    let turned = greycard_core::develop::orient(small, orientation);
    let rgb = turned
        .data
        .iter()
        .map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect();
    (turned.width as u32, turned.height as u32, rgb)
}

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame the camera framed at 3:2 has its camera picture handed
    /// out cut to the band, as the grid, the strip, and the Navigator
    /// draw it; a frame it did not frame, and a lookup with no edited
    /// pictures to say whose, as made.
    #[test]
    fn the_cameras_picture_is_handed_out_cut_to_its_frame() {
        use greycard_core::raw::Orientation;
        let thumb = Thumb {
            width: 40,
            height: 30,
            rgb: vec![9; 40 * 30 * 3],
        };
        let edited = crate::edited_pictures::Edited::new(Arc::new(Mutex::new(None)));
        let (framed, whole) = (
            std::path::Path::new("/nowhere/framed-thumb.orf"),
            std::path::Path::new("/nowhere/whole-thumb.orf"),
        );
        crate::edited_pictures::note_camera(
            framed,
            Some(greycard_core::testing::three_two_frame(Orientation::Normal)),
        );
        crate::edited_pictures::note_camera(whole, None);
        let cut = cut_to_camera(thumb.clone(), framed, Some(&edited), false);
        assert_eq!((cut.width, cut.height), (40, 26));
        assert_eq!(cut.rgb.len(), 40 * 26 * 3);
        let kept = cut_to_camera(thumb.clone(), whole, Some(&edited), false);
        assert_eq!((kept.width, kept.height), (40, 30));
        let unasked = cut_to_camera(thumb.clone(), framed, None, false);
        assert_eq!((unasked.width, unasked.height), (40, 30));
        // A picture from the cache alone of a frame not known yet
        // stays whole, and nothing of the file is read for it.
        let dir = crate::testing::scratch_dir("thumb-cut-unread");
        let unknown = dir.join("unknown.ORF");
        std::fs::write(
            &unknown,
            greycard_core::testing::olympus_head(1, [8, 8, 5184, 3888], [0, 216, 5183, 3671]),
        )
        .unwrap();
        let whole_still = cut_to_camera(thumb.clone(), &unknown, Some(&edited), false);
        assert_eq!((whole_still.width, whole_still.height), (40, 30));
        assert_eq!(crate::edited_pictures::camera_known(&unknown), None);
        // Made from the file, which was read: its head is read too.
        let made = cut_to_camera(thumb, &unknown, Some(&edited), true);
        assert_eq!((made.width, made.height), (40, 26));
        crate::testing::remove_dir_retry(&dir);
        // A picture the camera cut already is not cut again.
        let shot = Thumb {
            width: 45,
            height: 30,
            rgb: vec![9; 45 * 30 * 3],
        };
        let same = cut_to_camera(shot, framed, Some(&edited), false);
        assert_eq!((same.width, same.height), (45, 30));
    }

    use greycard_core::color::{CAT, WORKING_SPACE, srgb_decode};
    use greycard_core::raw::Orientation;
    use greycard_render::export::Space;
    use image::ImageEncoder;

    /// A saturated green, a red, and a grey, in sRGB: inside its gamut,
    /// so a file in any wider space holds them.
    const PATCHES: [[u8; 3]; 3] = [[40, 190, 50], [205, 35, 30], [118, 118, 118]];

    /// A patch's side in the fixtures: whole 4:2:0 JPEG blocks, so a
    /// JPEG's chroma does not bleed into the next patch's middle, and
    /// a TIFF large enough that the `image` crate reads its profile
    /// (its decoder refuses a tag whose values outweigh the picture's
    /// bytes).
    const SIDE: u32 = 128;

    /// The thumbnail's long edge for the fixtures: a patch is four
    /// pixels of it.
    const SMALL: u32 = 12;

    /// An sRGB color as the exporter encodes it for `space`: to the
    /// working space, the space's matrix, sRGB's curve, and the
    /// space's own over it (`Space::recode`).
    fn encoded_for(space: Space, srgb: [u8; 3]) -> [f32; 3] {
        let lin = srgb.map(|v| srgb_decode(v as f32 / 255.0));
        let to_working = RgbSpace::SRGB
            .to_space_matrix(&WORKING_SPACE, CAT)
            .unwrap()
            .rows
            .map(|r| r.map(|v| v as f32));
        let mul =
            |m: [[f32; 3]; 3], v: [f32; 3]| m.map(|r| r[0] * v[0] + r[1] * v[1] + r[2] * v[2]);
        mul(space.matrix(), mul(to_working, lin))
            .map(|v| space.recode(srgb_encode(v.clamp(0.0, 1.0))))
    }

    /// The patches side by side, encoded for `space`, at `max`.
    fn patches(space: Space, max: f32) -> Vec<f32> {
        let mut px = Vec::new();
        for _ in 0..SIDE {
            for p in PATCHES {
                for _ in 0..SIDE {
                    px.extend(encoded_for(space, p).map(|v| (v * max).round()));
                }
            }
        }
        px
    }

    /// The patches written as a picture file the way the exporter
    /// writes one: its profile embedded, a PNG or JPEG of 8 bits or a
    /// TIFF of 16. `None` writes no profile.
    fn write_patches(path: &std::path::Path, space: Space, icc: Option<Vec<u8>>) {
        let (w, h) = (SIDE * 3, SIDE);
        let ext = path.extension().unwrap().to_str().unwrap();
        if ext == "tif" {
            let px: Vec<u16> = patches(space, 65535.0).iter().map(|v| *v as u16).collect();
            let file = std::fs::File::create(path).unwrap();
            greycard_core::exif::write_rgb16_tiff(file, w, h, &px, icc.as_deref(), None).unwrap();
            return;
        }
        let px: Vec<u8> = patches(space, 255.0).iter().map(|v| *v as u8).collect();
        let mut bytes = Vec::new();
        if ext == "png" {
            let mut enc = image::codecs::png::PngEncoder::new(&mut bytes);
            if let Some(icc) = icc {
                enc.set_icc_profile(icc).unwrap();
            }
            enc.write_image(&px, w, h, image::ExtendedColorType::Rgb8)
                .unwrap();
        } else {
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 100);
            if let Some(icc) = icc {
                enc.set_icc_profile(icc).unwrap();
            }
            enc.write_image(&px, w, h, image::ExtendedColorType::Rgb8)
                .unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    /// The middle of each patch in an 8-bit sRGB picture `w` wide.
    fn middles(w: u32, h: u32, rgb: &[u8]) -> [[u8; 3]; 3] {
        let patch = w / 3;
        std::array::from_fn(|k| {
            let i = ((h / 2 * w) + patch * k as u32 + patch / 2) as usize * 3;
            [rgb[i], rgb[i + 1], rgb[i + 2]]
        })
    }

    /// What the develop's own reading of the file makes of each patch's
    /// middle: `decode_picture` into the working space, then to sRGB.
    fn as_developed(path: &std::path::Path) -> [[u8; 3]; 3] {
        let pic = greycard_core::picture::decode_picture_path(path).unwrap();
        let to_srgb = WORKING_SPACE
            .to_space_matrix(&RgbSpace::SRGB, CAT)
            .unwrap()
            .rows
            .map(|r| r.map(|v| v as f32));
        let (w, h) = (pic.image.width, pic.image.height);
        let mut rgb = Vec::with_capacity(w * h * 3);
        for p in pic.image.data.as_chunks::<3>().0 {
            for r in to_srgb {
                let v = r[0] * p[0] + r[1] * p[1] + r[2] * p[2];
                rgb.push((srgb_encode(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
            }
        }
        middles(w as u32, h as u32, &rgb)
    }

    fn assert_near(got: [[u8; 3]; 3], want: [[u8; 3]; 3], within: i32, what: &str) {
        for (g, w) in got.iter().zip(&want) {
            for (a, b) in g.iter().zip(w) {
                assert!(
                    (*a as i32 - *b as i32).abs() <= within,
                    "{what}: {got:?} against {want:?}"
                );
            }
        }
    }

    /// A picture exported in Rec.2020, Adobe RGB, or Display P3 has a
    /// thumbnail of the colors it holds: those of the same colors
    /// exported in sRGB, and those the develop reads from it, not its
    /// samples shown as if they were sRGB's (issue 209). A PNG and a
    /// JPEG of 8 bits, and a TIFF of 16.
    #[test]
    fn a_pictures_thumbnail_is_taken_through_its_profile() {
        let dir = crate::testing::scratch_dir("thumb-profile");
        let reference = dir.join("srgb.png");
        write_patches(&reference, Space::Srgb, Some(Space::Srgb.icc().unwrap()));
        let (w, h, rgb) = thumbnail(&reference, SMALL).unwrap();
        let srgb = middles(w, h, &rgb);
        assert_near(srgb, PATCHES, 0, "the sRGB file");
        // How near each comes to sRGB's: its 8-bit steps, Adobe RGB's
        // pure power coarse at the foot where the green's red is, and
        // a JPEG's loss. The develop's reading of the same file is met
        // to a level whatever the file.
        let files = [
            ("rec2020.png", Space::Rec2020, 1),
            ("adobe.png", Space::AdobeRgb, 3),
            ("p3.png", Space::DisplayP3, 1),
            ("rec2020.tif", Space::Rec2020, 1),
            ("adobe.tif", Space::AdobeRgb, 1),
            // Rec.2020's matrix to sRGB magnifies the JPEG's chroma loss
            // in the green's red; the profile unread, it is 79 out.
            ("rec2020.jpg", Space::Rec2020, 8),
        ];
        for (name, space, within) in files {
            let path = dir.join(name);
            write_patches(&path, space, Some(space.icc().unwrap()));
            let (w, h, rgb) = thumbnail(&path, SMALL).unwrap();
            assert_eq!((w, h), (SMALL, SMALL / 3));
            let got = middles(w, h, &rgb);
            assert_near(got, srgb, within, &format!("{name} against sRGB's"));
            assert_near(got, as_developed(&path), 1, &format!("{name} as developed"));
        }
        // A TIFF of floats and no profile: linear sRGB, as the develop
        // takes it, not its samples as if encoded.
        let floats = dir.join("linear.tif");
        let px: Vec<f32> = patches(Space::Srgb, 255.0)
            .iter()
            .map(|v| srgb_decode(v / 255.0))
            .collect();
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::codecs::tiff::TiffEncoder::new(&mut bytes)
            .write_image(
                &px.iter().flat_map(|v| v.to_ne_bytes()).collect::<Vec<u8>>(),
                SIDE * 3,
                SIDE,
                image::ExtendedColorType::Rgb32F,
            )
            .unwrap();
        std::fs::write(&floats, bytes.into_inner()).unwrap();
        let (w, h, rgb) = thumbnail(&floats, SMALL).unwrap();
        let got = middles(w, h, &rgb);
        assert_near(got, srgb, 1, "linear.tif against sRGB's");
        assert_near(got, as_developed(&floats), 1, "linear.tif as developed");
        crate::testing::remove_dir_retry(&dir);
    }

    /// The downscale as it was before profiles were read, for the
    /// pictures that must come out as they did.
    fn thumbnail_before(path: &std::path::Path, size: usize) -> (u32, u32, Vec<u8>) {
        let preview = image::open(path).unwrap().to_rgb8();
        let (pw, ph) = (preview.width() as usize, preview.height() as usize);
        let factor = pw.max(ph).div_ceil(size).max(1);
        let (w, h) = ((pw / factor).max(1), (ph / factor).max(1));
        let px = preview.as_raw();
        let mut rgb = Vec::new();
        for ty in 0..h {
            for tx in 0..w {
                let mut sum = [0u32; 3];
                for y in ty * factor..(ty + 1) * factor {
                    for x in tx * factor..(tx + 1) * factor {
                        let i = (y * pw + x) * 3;
                        for (s, v) in sum.iter_mut().zip(&px[i..i + 3]) {
                            *s += *v as u32;
                        }
                    }
                }
                let n = (factor * factor) as f32;
                rgb.extend(sum.map(|s| ((s as f32 / n / 255.0) * 255.0).round() as u8));
            }
        }
        (w as u32, h as u32, rgb)
    }

    /// A picture with no profile, and one whose profile is sRGB's as the
    /// exporter writes it, are thumbnailed byte for byte as before: the
    /// profile is read and found to be sRGB's, and nothing more is done.
    #[test]
    fn an_untagged_or_srgb_pictures_thumbnail_is_as_before() {
        let srgb = Space::Srgb.icc().unwrap();
        assert!(
            greycard_core::picture::ToSrgb::of_profile(Some(&srgb))
                .unwrap()
                .is_srgb()
        );
        assert!(
            !greycard_core::picture::ToSrgb::of_profile(Some(&Space::DisplayP3.icc().unwrap()))
                .unwrap()
                .is_srgb()
        );
        let dir = crate::testing::scratch_dir("thumb-srgb");
        // A ramp with some texture, a downscale that averages.
        let (w, h) = (90u32, 60u32);
        let ramp = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([
                (x * 255 / w) as u8,
                (y * 255 / h) as u8,
                ((x * 7 + y * 13) % 256) as u8,
            ])
        });
        let files: [(&str, Option<Vec<u8>>); 4] = [
            ("plain.png", None),
            ("plain.jpg", None),
            ("tagged.png", Some(srgb.clone())),
            ("tagged.jpg", Some(srgb.clone())),
        ];
        for (name, icc) in files {
            let path = dir.join(name);
            let mut bytes = Vec::new();
            if name.ends_with("png") {
                let mut enc = image::codecs::png::PngEncoder::new(&mut bytes);
                if let Some(icc) = icc {
                    enc.set_icc_profile(icc).unwrap();
                }
                enc.write_image(ramp.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                    .unwrap();
            } else {
                let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90);
                if let Some(icc) = icc {
                    enc.set_icc_profile(icc).unwrap();
                }
                enc.write_image(ramp.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                    .unwrap();
            }
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(
                thumbnail(&path, 20).unwrap(),
                thumbnail_before(&path, 20),
                "{name}"
            );
        }
        // A TIFF of 16 bits, tagged sRGB: through 8 bits as before.
        let tiff = dir.join("tagged.tif");
        write_patches(&tiff, Space::Srgb, Some(srgb));
        assert_eq!(
            thumbnail(&tiff, SMALL).unwrap(),
            thumbnail_before(&tiff, SMALL as usize)
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// The culling loupe's, the lightbox's, and the local preview's
    /// picture of a picture file is taken through its profile too.
    #[test]
    fn a_pictures_camera_picture_is_taken_through_its_profile() {
        let dir = crate::testing::scratch_dir("camera-profile");
        let reference = dir.join("srgb.png");
        write_patches(&reference, Space::Srgb, Some(Space::Srgb.icc().unwrap()));
        let (plain, _) = crate::camera::camera_picture(&reference).unwrap();
        let srgb = middles(plain.width(), plain.height(), plain.as_raw());
        assert_near(srgb, PATCHES, 0, "the sRGB file");
        for (name, space) in [
            ("rec2020.png", Space::Rec2020),
            ("p3.jpg", Space::DisplayP3),
            ("adobe.tif", Space::AdobeRgb),
        ] {
            let path = dir.join(name);
            write_patches(&path, space, Some(space.icc().unwrap()));
            let (got, orientation) = crate::camera::camera_picture(&path).unwrap();
            assert_eq!(orientation, Orientation::Normal);
            let got = middles(got.width(), got.height(), got.as_raw());
            assert_near(got, srgb, 3, &format!("{name} against sRGB's"));
            assert_near(got, as_developed(&path), 1, &format!("{name} as developed"));
        }
        crate::testing::remove_dir_retry(&dir);
    }
}
