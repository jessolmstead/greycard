//! The app handle, the settings file and the session's policies: [`Window`].

use std::path::PathBuf;

use crate::App;

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
    /// Whether a raw's new edit starts with the lens section on:
    /// `settings.lens_on_new`, as the Settings sheet's switch flips
    /// it. A sidecar already written keeps its own.
    pub(crate) lens_on_new: bool,
    /// The author and copyright every export carries, over the
    /// camera's: `settings.author` and `settings.copyright`, as the
    /// Settings sheet's fields last took them. Empty for none.
    pub(crate) author: String,
    pub(crate) copyright: String,
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

impl Window {
    /// Where a sidecar this session writes goes, none with sidecars
    /// off: what a read that may take a Lightroom develop is told.
    pub(crate) fn sidecars(&self) -> Option<greycard_edit::Placement> {
        self.write_sidecars.then_some(self.placement)
    }

    /// The author and copyright the next export writes.
    pub(crate) fn rights(&self) -> greycard_render::export::Rights {
        greycard_render::export::Rights::new(&self.author, &self.copyright)
    }

    /// What a raw with no sidecar starts with, by the setting.
    pub(crate) fn fresh(&self) -> greycard_edit_io::on_disk::Fresh {
        greycard_edit_io::on_disk::Fresh {
            lens: self.lens_on_new,
        }
    }
}
