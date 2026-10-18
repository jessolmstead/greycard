//! The exports, their queue and the archive copies' writes: [`Work`].

use crate::*;

/// The work out on other threads that the window follows: the exports,
/// their queue and the settings they keep, and the writes to the
/// archives' copies.
pub(crate) struct Work {
    /// A set being exported, until its last frame is done with: held
    /// to cancel it and to know its outcomes from a stale set's.
    pub(crate) exporting: Option<Arc<queue::Set>>,
    /// A folder chooser for an export is up, or the overwrite sheet:
    /// Export waits for the answer.
    pub(crate) export_choosing: bool,
    /// How many looks at an export's folder have begun: an answer of
    /// an earlier one (a mount that kept it waiting, overtaken by
    /// another Export or a Choose) is dropped. A look holds nothing
    /// up, so a folder that never answers never stops Export.
    pub(crate) export_checks: u64,
    /// The folder the latest look is at, while it has not answered: a
    /// Choose that overtakes it says the export was not started.
    pub(crate) export_looking: Option<PathBuf>,
    /// The line a set of one frame ends on, made as its frame is done:
    /// the file's name, a rename and what the edit went without, which
    /// a set's count does not say.
    pub(crate) export_one_line: Option<(Arc<queue::Set>, String)>,
    /// A set that would write over files that are there, held while
    /// the overwrite sheet asks; `export_choosing` is up meanwhile.
    pub(crate) export_pending: Option<crate::panel::deliver::Pending>,
    /// The sets waiting to be exported, oldest first, as
    /// `export-queue.json` keeps them.
    pub(crate) export_queue: Vec<export_queue::Entry>,
    /// Where the queue is written whenever it changes; none for a
    /// snapshot or a batch run, which leave the user's files alone.
    pub(crate) export_queue_file: Option<PathBuf>,
    /// The queue's head entry being exported, while the queue runs.
    pub(crate) queue_run: Option<crate::panel::export_queue::Run>,
    /// How many looks for an entry's files the queue has begun.
    pub(crate) queue_probes: u64,
    /// The export presets, as the settings file keeps them.
    pub(crate) export_presets: Vec<sheet::ExportPreset>,
    /// The folder an export's chooser last answered with, where the
    /// next export goes and the next chooser opens; the settings file
    /// keeps it across runs.
    pub(crate) export_last_folder: Option<PathBuf>,
    /// The writes behind the saves to the archives' copies (§233):
    /// see [`sync`].
    pub(crate) sync: sync::Sync,
}
