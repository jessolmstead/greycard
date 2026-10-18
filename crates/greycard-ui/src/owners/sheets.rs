//! The sheets without a struct of their own: [`Sheets`].

use crate::*;

/// The sheets that have no struct of their own: deleting frames and
/// versions, removing and renaming a look, the model and lens offers,
/// saving a preset, and syncing and pasting settings with the
/// clipboard. A sheet with a struct of its own (import, Lightroom,
/// RawTherapee, camera match, archive) stays on [`crate::State`].
pub(crate) struct Sheets {
    /// The delete sheet's question while it is up: what it would
    /// delete and from where. Read again at the confirm.
    pub(crate) delete_asked: Option<panel::delete::Asked>,
    /// The delete sheet's question over versions while it is up, in
    /// place of `delete_asked`: a version goes by a save, not to the
    /// trash.
    pub(crate) version_delete: Option<panel::versions::Asked>,
    /// The folders whose files the system trash has refused this
    /// session: the sheet offers the permanent delete beside the
    /// trash for these, as a choice.
    pub(crate) trash_refused: std::collections::HashSet<PathBuf>,
    /// The remove-a-look sheet's question while it is up, how many
    /// have been asked (so a late count is dropped), and a removal out
    /// on its thread.
    pub(crate) look_remove: Option<panel::look_remove::Asked>,
    pub(crate) look_remove_generation: u64,
    pub(crate) look_removing: bool,
    /// The rename-a-look sheet's question while it is up, and how many
    /// have been asked, so a late count is dropped.
    pub(crate) look_rename: Option<panel::look_rename::Asked>,
    pub(crate) look_rename_generation: u64,
    /// A rename out on its thread, until it lands.
    pub(crate) look_renaming: Option<panel::look_rename::Renaming>,
    /// A delete is out on its thread, with the frames it has planned:
    /// another is refused until it lands, and so is Move rejects, and
    /// no sidecar of those frames is written meanwhile
    /// (`panel::delete::held`).
    pub(crate) deleting: Option<std::collections::HashSet<PathBuf>>,
    /// What the sheet offers to fetch, and whether something is coming.
    pub(crate) fetch: Option<Fetch>,
    pub(crate) fetching: bool,
    /// Models declined this session, not to be asked for again.
    pub(crate) declined: Vec<&'static str>,
    /// The Subject model declined this session as a Whole person's
    /// edge: not offered for it again.
    pub(crate) edge_declined: bool,
    /// A Whole person was just asked for from the People menu: the
    /// Subject model its edge runs may be offered once, beside it.
    /// Never for one loaded from a sidecar or pasted.
    pub(crate) edge_offer_due: bool,
    /// A Whole person has been made with Subject's edge this session,
    /// so the worker has the Subject model loaded: the first such run
    /// says on the status line that it loads it.
    pub(crate) edge_loaded: bool,
    /// Models whose fetch failed this session: a Subject turns from
    /// its WebGPU file to the original when that one fails.
    pub(crate) fetch_failed: Vec<&'static str>,
    /// The lens profiles' first-launch offer: whether it was ever
    /// answered Not now (`settings.lenses_declined`), and whether it
    /// has been made this launch.
    pub(crate) lenses_declined: bool,
    pub(crate) lenses_asked: bool,
    /// The save sheet's checks of sections, parts and masks, and the
    /// rows the sheet shows of them (`carry`).
    pub(crate) preset_checks: carry::Checks,
    pub(crate) preset_rows: carry::Shown,
    /// The sync sheet's, the same way; a paste's too.
    pub(crate) sync_checks: carry::Checks,
    pub(crate) sync_rows: carry::Shown,
    /// The frame and the targets the sync sheet was opened on, so an
    /// Apply can refuse a selection that moved under it.
    pub(crate) sync_asked: Option<(usize, Vec<usize>)>,
    /// The same for a paste: the frame on screen and the frames the
    /// sheet named, so an Apply can refuse a selection that moved.
    pub(crate) paste_asked: Option<(usize, Vec<usize>)>,
    /// The checks the last sync or paste laid over the selection,
    /// for the session: a paste's sheet opens with them.
    pub(crate) sync_last: Option<carry::Checks>,
    /// The settings clipboard: an edit copied from a frame, to paste
    /// over the selection (`clipboard`). The session's, never the
    /// system's.
    pub(crate) clipboard: Option<clipboard::Clipboard>,
}
