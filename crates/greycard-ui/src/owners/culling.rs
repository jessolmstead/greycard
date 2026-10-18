//! Culling, the lightbox and the camera's pictures: [`Culling`].

use crate::*;

/// Culling mode and the lightbox, and the camera's pictures they and
/// the develop view stand in with: held, waited for and decoded ahead.
pub(crate) struct Culling {
    /// Culling mode (notes §80), while it is on.
    pub(crate) cull: Option<Cull>,
    /// Whether a culling key moves the selection on to the next
    /// frame: the CULLING section's switch, `settings.cull_move_on`.
    pub(crate) cull_move_on: bool,
    /// Takes the word over the picture for the last culling key down
    /// again; nothing else does, so an arrow pressed after the key
    /// leaves the word up for its second.
    pub(crate) notice_timer: slint::Timer,
    /// The session's rating, flag and label changes: culling's undo.
    pub(crate) tags: tags::History,
    /// The camera's pictures the develop view keeps: what the mode
    /// left behind on its way out, and what a select decoded, with
    /// the textures made from them. They are drawn only while
    /// `placeholder` says a develop is pending.
    pub(crate) hold: Option<Cull>,
    /// A frame selected whose develop has not landed: the camera's
    /// JPEG stands in for it meanwhile.
    pub(crate) placeholder: Option<placeholder::Wait>,
    /// The previews decoded ahead of the arrow, on their own threads.
    pub(crate) prefetch: cull::Prefetcher,
    /// The lightbox, while it is on: its tiles' pictures.
    pub(crate) lightbox: Option<lightbox::Lightbox>,
    /// The lightbox's decode threads, made the first time it opens.
    pub(crate) lightbox_fetch: Option<cull::Prefetcher>,
    /// The compare view's tiles on the view, one model for the life
    /// of the window, as the handles' are.
    pub(crate) compare_tiles: Rc<VecModel<CompareTile>>,
}
