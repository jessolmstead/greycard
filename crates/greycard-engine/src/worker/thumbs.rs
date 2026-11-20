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

/// What a cache entry for a file of this stat must have been made
/// under: this recipe and the file's modification time. The hash reads
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
pub fn thumb_tag(stat: (u64, u64)) -> Tag {
    Tag {
        recipe: THUMB_RECIPE,
        stamp: stat.1,
    }
}

/// What a file's thumbnails are kept under, its content hash and the
/// stamp of [`thumb_tag`], for taking them out of the cache when the
/// file is deleted: read before it goes, since neither can be after.
pub fn thumb_key(path: &std::path::Path) -> Option<(String, u64)> {
    let stat = file_stat(path)?;
    let hash = greycard_library::hash_file(path).ok()?;
    Some((hash, thumb_tag(stat).stamp))
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
        Ok(hash) => Some((hash, thumb_tag(stat))),
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
        // The picture itself, as the file encodes it: near enough to
        // sRGB for a thumbnail whatever its profile. The tag is the
        // picture module's to read, since that is the one the
        // develop turns by and this thumbnail has to agree with it.
        let orientation = greycard_core::picture::orientation_path(path)?;
        let decoder = image::ImageReader::open(path)?
            .with_guessed_format()?
            .into_decoder()?;
        let rgb = image::DynamicImage::from_decoder(decoder)?.to_rgb8();
        return Ok(thumbnail_of_preview(&rgb, orientation, size));
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
    let (pw, ph) = (preview.width() as usize, preview.height() as usize);
    // The long side, as the develop's turned picture may be portrait.
    let factor = pw.max(ph).div_ceil(size).max(1);
    let (w, h) = ((pw / factor).max(1), (ph / factor).max(1));
    let mut data = Vec::with_capacity(w * h * 3);
    let px = preview.as_raw();
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
            data.extend(sum.map(|s| s as f32 / n / 255.0));
        }
    }
    let turned = greycard_core::develop::orient(
        WorkingImage {
            width: w,
            height: h,
            data,
        },
        orientation,
    );
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
}
