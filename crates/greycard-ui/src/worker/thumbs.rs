//! Thumbnails: the cache on disk, and making one when it misses.

use super::*;

/// How [`thumbnail`] makes a picture, as the cache's entries record
/// it in their names: raise it when the picture it makes changes (the
/// downscale, the turn, rawler's choice of preview, or the fallback
/// develop through `DevelopSettings::default()`, whose definition in
/// core names this constant), and every entry made the old way is a
/// miss and is made again. The long edge is in the key already, so a
/// change of the sizes made (`grid::MADE`) needs no bump.
pub const THUMB_RECIPE: u16 = 2;

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
pub(crate) fn file_stat(path: &std::path::Path) -> Option<(u64, u64)> {
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
pub(crate) fn thumb_tag(stat: (u64, u64)) -> Tag {
    Tag {
        recipe: THUMB_RECIPE,
        stamp: stat.1,
    }
}

/// What a file's thumbnails are kept under, its content hash and the
/// stamp of [`thumb_tag`], for taking them out of the cache when the
/// file is deleted: read before it goes, since neither can be after.
pub(crate) fn thumb_key(path: &std::path::Path) -> Option<(String, u64)> {
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
    edited: Option<&crate::edited::Edited>,
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
        key.as_ref().and_then(|(hash, tag)| {
            cache
                .lock()
                .expect("thumbnail cache")
                .as_mut()
                .and_then(|c| c.get(hash, size, *tag))
        })
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
    edited: Option<&crate::edited::Edited>,
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
    Ok((thumb, false))
}

/// A small sRGB rendering of a file, `size` on its long edge: the
/// camera's own preview JPEG when the file has one, box downscaled
/// and turned as the camera says; else a bilinear demosaic, box
/// downscaled.
pub(crate) fn thumbnail(path: &std::path::Path, size: u32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
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
    // goes away is a SIGBUS.
    if let Ok(Some((preview, orientation))) = greycard_core::decode::preview_path(path) {
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
