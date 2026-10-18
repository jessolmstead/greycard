//! The app handle, the settings file and the session's policies: [`Window`].

use crate::*;

/// The window itself and what this session may do to the user's files:
/// the Slint app handle, the settings file, whether sidecars are
/// written and where, whether anything may be deleted, and whether
/// the run is a batch with nobody at it.
pub(crate) struct Window {
    pub(crate) write_sidecars: bool,
    /// Whether a meta written to the `.gcd` is also written to an
    /// `.xmp` beside the frame: `settings.xmp_sidecars`, or
    /// `--xmp-sidecars`. Reading one is not a choice, so nothing
    /// here guards it.
    pub(crate) xmp_sidecars: bool,
    /// Where a sidecar written goes, beside the frame or under the
    /// hidden folder: `settings.sidecars_in_folder`, or
    /// `--sidecar-folder`. Reading looks in both places either way.
    pub(crate) placement: greycard_edit::Placement,
    /// Whether anything may be deleted from disk: only in a session
    /// someone is at. A snapshot, a screenshot, an export or a timing
    /// run never deletes, whatever it is asked, and a test's state
    /// does not until the test says so.
    pub(crate) deletes_allowed: bool,
    /// Where a preset saved, chosen or deleted is written at once;
    /// none for a snapshot or a batch run, which leave the user's
    /// settings alone.
    pub(crate) settings_file: Option<PathBuf>,
    /// Whether the command line asked for a snapshot, a screenshot or
    /// an export. All three wait for a picture, and nobody is reading
    /// the status line, so a failure has to end the run itself.
    pub(crate) batch: bool,
    /// The window, for a job's end landing on its thread from a place
    /// that has only the state in hand.
    pub(crate) app: slint::Weak<App>,
}
