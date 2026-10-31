//! What the command line asked of this run: [`Launch`].

use crate::*;

/// What the command line asked of this run, and what the run has done
/// of it: the captures and their waits, the timing runs, and the actions
/// taken once the first frame is opened or its picture is up.
pub(crate) struct Launch {
    /// What the command line asked of the first frame opened
    /// (`--preset`, `--develop-temperature`, `--exposure`), applied to
    /// its sidecar when the launch's list lands, before it is opened.
    pub(crate) tweak_at_start: Option<Tweak>,
    /// `--time-sharpen`, `--time-clarity`, `--time-dehaze` and
    /// `--time-texture`: the moves left, when the last was sent, and
    /// the milliseconds each took to its frame.
    pub(crate) time_sharpen: Option<crate::panel::edit::Timing>,
    pub(crate) time_clarity: Option<crate::panel::edit::Timing>,
    pub(crate) time_dehaze: Option<crate::panel::edit::Timing>,
    pub(crate) time_texture: Option<crate::panel::edit::Timing>,
    /// `--time-cull`: the steps left and the milliseconds each took
    /// from the key to the frame that showed the next picture.
    pub(crate) time_cull: Option<(u32, Vec<f64>)>,
    /// `--time-select`: the steps left, and the milliseconds each
    /// step took from the key to the frame that showed the camera's
    /// picture and to the frame that showed the develop.
    pub(crate) time_select: Option<(u32, Vec<f64>, Vec<f64>)>,
    /// `--time-scroll`: the moves left and what each cost.
    pub(crate) time_scroll: Option<panel::browser::ScrollTiming>,
    /// `--cull-develop`: leave the mode once its first picture shows.
    pub(crate) cull_develop: bool,
    /// `--snapshot-placeholder`: step to the next frame once the
    /// first develop is on screen, and let a capture go as soon as
    /// that frame's camera picture is up rather than waiting for its
    /// develop.
    pub(crate) snapshot_placeholder: bool,
    /// `--turn`: quarter turns to put on the selection once the first
    /// picture shows, for a capture.
    pub(crate) turn_at_start: Option<i32>,
    /// `--turn` was asked for and the picture it leads to is not on
    /// screen yet. A capture waits for it, or it would catch the
    /// develop that fired the key rather than the turned frame.
    pub(crate) awaiting_turn: bool,
    /// `--cull-key`: the culling key to press once the first picture
    /// shows, for a capture, which waits until it has been.
    pub(crate) cull_key_at_start: Option<String>,
    /// `--ask-rejects`: open the move-rejects sheet once it does;
    /// `--move-rejects`: and answer it with yes.
    pub(crate) ask_rejects: bool,
    pub(crate) move_rejects: bool,
    /// Open in culling mode, with the compare view at this many
    /// frames, once the first file is chosen (`--cull`).
    pub(crate) cull_at_start: Option<usize>,
    /// Open in Cull drawn developed once the first file is chosen: the
    /// Loupe's mode the last run left, with its Developed switch on.
    pub(crate) developed_at_start: bool,
    /// The command line owns the start (`startup::scripted_start`): a
    /// capture or a timing run. The Loupe's remembered mode is neither
    /// taken nor kept.
    pub(crate) scripted: bool,
    /// `--filter`'s facet terms, chosen when the pass is done.
    pub(crate) facets_wanted: Vec<filter::Wanted>,
    /// A capture waits for those to be chosen.
    pub(crate) awaiting_index: bool,
    /// The last folder, kept while the launch waits for the start
    /// folder's view in the library: opened instead should the index
    /// fail to open.
    pub(crate) last_folder: Option<PathBuf>,
    /// The file to open once the window has its device: the first
    /// develop then runs where every later one does, so a screenshot
    /// or an export shows the same path a session would.
    pub(crate) select_at_start: Option<usize>,
    /// `--develop-temperature` and `--exposure`: laid over the first
    /// file's edit on the panel when it is opened, not over its
    /// sidecar's. See [`panel::startup::Overrides`].
    pub(crate) overrides_at_start: Option<(usize, panel::startup::Overrides)>,
    /// The first file, and its edit as those overrides left it on the
    /// panel, until the panel moves off it or the frame is left: while
    /// the panel shows exactly this, there is nothing of the panel's
    /// to record (`panel::edit::panel_state`).
    pub(crate) overridden: Option<(PathBuf, Edit)>,
    /// The rendering setup has run and opened what it was to open: a
    /// list put in the browser from then on opens its own frame.
    pub(crate) setup_ran: bool,
    /// `--also`: rows put in the set once the first frame is opened.
    pub(crate) also_at_start: Vec<usize>,
    /// A temperature to put on the panel once the file opens, for the
    /// preview check.
    pub(crate) preview_temperature: Option<f32>,
    /// A mask to paint whatever the panel says, for a screenshot.
    pub(crate) show_mask: Option<usize>,
    /// A patch to choose on the first file, for a screenshot.
    pub(crate) show_patch: Option<usize>,
    /// A file has been opened this session (`open_frame`): what the
    /// command line asks of the first file is for that open alone.
    pub(crate) opened: bool,
    /// The open the camera's picture stands in for is the session's
    /// first, so the command line's zoom stands through it rather
    /// than the picture fitting the view (`cull::placeholder_arrived`).
    pub(crate) zoom_stands: bool,
    /// The snapshot has waited its limit for the grid's pictures and
    /// takes the grid as it stands.
    pub(crate) grid_wait_over: bool,
    pub(crate) screenshot: Option<PathBuf>,
    pub(crate) snapshot: Option<PathBuf>,
    /// How far down the panel is scrolled before the snapshot,
    /// logical pixels.
    pub(crate) panel_scroll: Option<f32>,
    /// A sheet or a tool put on screen before the snapshot.
    pub(crate) snapshot_shown: Option<panel::viewport::Shown>,
    /// Something a batch run asked for did not happen: the exit code
    /// says so.
    pub(crate) failed: bool,
}
