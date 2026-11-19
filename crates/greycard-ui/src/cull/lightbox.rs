//! What a lightbox tile's picture is, on culling's decode threads: the
//! picture of the frame's edit, a wait for one, or the camera's, the
//! loupe's own decode. The window's side is the binary's `lightbox`.

use std::sync::Arc;

use crate::cull::{self, Loaded, Preview, Source, Want};
use crate::edited_pictures::{BIG, Edited, Shows};
use crate::grid;
use greycard_library::thumbs::Thumb;

/// The sizes of the edit's pictures kept, in the order a tile wanting
/// `size` tries them: the least that covers it first, larger ones after,
/// then the smaller ones from the largest down, so a tile takes a picture
/// too small only when nothing larger is kept.
pub fn kept_order(size: u32) -> Vec<u32> {
    let mut kept: Vec<u32> = grid::MADE.iter().copied().chain([BIG]).collect();
    kept.sort_unstable();
    let (small, large): (Vec<u32>, Vec<u32>) = kept.into_iter().partition(|&s| s < size);
    large.into_iter().chain(small.into_iter().rev()).collect()
}

/// What a tile shows.
#[derive(Debug)]
pub(crate) enum Pick {
    /// The picture of the frame's edit, at the tile's size or the
    /// nearest kept.
    Edit(Thumb),
    /// The frame has an edit and no picture of it is kept yet: nothing,
    /// until one is.
    Wait,
    /// The frame has no edit: the camera's JPEG.
    Camera,
}

/// What a tile wanting `size` shows of the frame of content `hash`,
/// which `shows` says shows its edit or not. Only the cache is read.
pub(crate) fn pick(edited: Option<&Edited>, hash: Option<&str>, shows: Shows, size: u32) -> Pick {
    if shows == Shows::Camera {
        return Pick::Camera;
    }
    let (Some(edited), Some(hash)) = (edited, hash) else {
        return Pick::Wait;
    };
    kept_order(size)
        .into_iter()
        .find_map(|s| edited.lookup(hash, s, shows))
        .map_or(Pick::Wait, |thumb| {
            Pick::Edit(crate::edited_pictures::shrunk(&thumb, size))
        })
}

/// One tile's picture, on a decode thread: see [`pick`]. The camera's
/// is the culling loupe's own decode at the tile's size.
// Its link names this module's private items, for a reader here.
#[allow(rustdoc::private_intra_doc_links)]
pub fn fetch(
    want: &Want,
    previews: Option<&Arc<crate::previews::Previews>>,
    deliver: &dyn Fn(Loaded),
) {
    let edited = previews.and_then(|p| p.edited());
    let (hash, shows) = match &want.from {
        Source::Preview { hash, .. } => (
            Some(hash.clone()),
            edited.map_or(Shows::Camera, |e| e.shows_offline(&want.frame())),
        ),
        Source::File => {
            let shows = edited.map_or(Shows::Camera, |e| e.shows(&want.frame()));
            let hash = (shows != Shows::Camera)
                .then(|| greycard_library::hash_file(&want.path).ok())
                .flatten();
            (hash, shows)
        }
    };
    match pick(edited, hash.as_deref(), shows, want.size) {
        Pick::Edit(thumb) => deliver(Loaded::Ok {
            file: want.file,
            frame: want.frame(),
            size: want.size,
            preview: Arc::new(Preview::edited(thumb)),
        }),
        Pick::Wait => deliver(Loaded::NoPreview {
            file: want.file,
            frame: want.frame(),
        }),
        Pick::Camera => cull::fetch(
            &Want {
                lightbox: false,
                make_preview: false,
                ..want.clone()
            },
            previews,
            deliver,
        ),
    }
}
