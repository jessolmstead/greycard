//! Helpers the test modules share.

use std::path::PathBuf;
use std::sync::Arc;

use greycard_core::color::Profile;
use greycard_core::lut;

use crate::{DisplayCurve, Edit, Loader};

/// A loader that fails the test when asked for anything: for what
/// names no file.
pub(crate) struct Unasked;

impl Loader for Unasked {
    fn profile(&self, name: &str) -> Option<Arc<Profile>> {
        panic!("asked for the profile {name:?}")
    }

    fn look(&self, name: &str, _: f32, _: DisplayCurve) -> Option<lut::Look> {
        panic!("asked for the look {name:?}")
    }
}

thread_local! {
    /// This thread's clock, for the times a state's moves stamp
    /// (`versions::now`): none, the system's.
    pub(crate) static CLOCK: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    /// This thread's install id (`sync::install`): none, the process's.
    pub(crate) static INSTALL: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
    /// This thread's process (`process_now`): none, the build's.
    pub(crate) static PROCESS: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
    /// This thread's next made id (`versions::random_id`): none, random.
    pub(crate) static IDS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// `f` with this thread's clock at `at`, its install id `install`, and
/// its made ids counted from one, all put back after: what a fixture is
/// written under, so no wall clock, no real id, and nothing at random
/// lands in it.
pub(crate) fn fixed<T>(at: u64, install: &str, f: impl FnOnce() -> T) -> T {
    struct Back;
    impl Drop for Back {
        fn drop(&mut self) {
            CLOCK.set(None);
            INSTALL.set(None);
            IDS.set(None);
        }
    }
    let _back = Back;
    CLOCK.set(Some(at));
    INSTALL.set(Some(install.to_string()));
    IDS.set(Some(1));
    f()
}

/// Whether schema 10 has shipped (0.13.0). Set at the cut, and never
/// back: from then the golden files (`fixtures/*-at-10.*`) are what an
/// earlier build wrote, and their writers refuse. Kept, not deleted,
/// so the next schema's goldens are made the same way.
pub(crate) const SCHEMA_10_CUT: bool = false;

/// The cut is not forgotten: from 0.13.0 on, [`SCHEMA_10_CUT`] is set.
#[test]
fn schema_10_is_cut_by_0_13_0() {
    let mut parts = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    assert!(
        SCHEMA_10_CUT || (major, minor) < (0, 13),
        "0.13.0 ships schema 10: set SCHEMA_10_CUT, so its golden files are never written again"
    );
}

/// The golden writers' guard: see [`SCHEMA_10_CUT`].
pub(crate) fn may_write_golden() -> bool {
    if SCHEMA_10_CUT {
        panic!(
            "schema 10 is cut: its golden files are an earlier build's and are never written again"
        );
    }
    std::env::var_os("GREYCARD_WRITE_GOLDEN").is_some()
}

/// A fresh folder a test can write into, its own by name and pid.
pub(crate) fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-edit-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The default edit at an exposure of `stops`.
pub(crate) fn exposed(stops: f32) -> Edit {
    let mut e = Edit::default();
    e.light.exposure = stops;
    e
}
