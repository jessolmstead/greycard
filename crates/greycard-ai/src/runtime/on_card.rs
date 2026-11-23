//! A learned denoiser off the card it lost. A learned denoiser's run on
//! a small, slow card took the whole adapter down, the window's device
//! with it, and again after the restart, which moved only the engine's
//! ops. A model is blamed only on evidence: its run was under way on
//! the card when the window's device was lost ([`on_card_now`], read by
//! the window's callback), or the run itself failed on a lost device
//! ([`Held::run`]). It is then written to a record of its own beside
//! the provider record, `off_card.json` in the model store, for this
//! adapter and driver and this build (its commit where it has one,
//! [`set_build_label`]), and while any denoiser tier is there for the
//! card this launch runs on, every tier is refused at its load
//! ([`denoisers_off_here`]): not on WebGPU, and not on the CPU after
//! it, which §646 cut on purpose. Nothing runs in its place: the frame
//! is developed with no denoise, as on X-Trans.
//!
//! A file of its own, not a field of the provider record, so that a
//! load on another adapter or driver, or an older greycard, which write
//! that record's one slot for the file and provider, can never wipe
//! this card's entry. Removing the file is the way back by hand.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::{Provider, Remembered, record};

/// A session on the card, named as the off-card record names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnCard {
    store_root: PathBuf,
    entry: Off,
}

/// One model file off one card, driver, and build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Off {
    /// The registry's id for the model.
    model: String,
    /// The file's published sha256 (or two joined, as the provider
    /// record keeps them).
    hash: String,
    /// The WebGPU adapter's name, backend, and driver.
    adapter: String,
    /// [`build_key`].
    build: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct OffFile {
    entries: Vec<Off>,
}

/// Where the off-card record lives for a store rooted at `store_root`.
pub fn off_card_path(store_root: &Path) -> PathBuf {
    store_root.join("off_card.json")
}

/// The record's entries, none where there is no file. A file that will
/// not parse reads as none, and says so in the log with its path: the
/// next write keeps only what it writes, so its other cards' entries go.
fn read_off(store_root: &Path) -> Vec<Off> {
    let path = off_card_path(store_root);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match serde_json::from_str::<OffFile>(&text) {
        Ok(file) => file.entries,
        Err(e) => {
            log::warn!(
                "{} could not be read ({e}); taken as empty, and its entries for other cards are dropped at its next write",
                path.display()
            );
            Vec::new()
        }
    }
}

/// Written beside and renamed, as the provider record is; best effort,
/// its landing checked by reading it back ([`OnCard::put_off`]).
fn write_off(store_root: &Path, entries: &[Off]) {
    let out = off_card_path(store_root);
    if std::fs::create_dir_all(store_root).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_string_pretty(&OffFile {
        entries: entries.to_vec(),
    }) else {
        return;
    };
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = store_root.join(format!("off_card.json.{}.{n}.partial", std::process::id()));
    if std::fs::write(&tmp, text).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if std::fs::rename(&tmp, &out).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The build the binary says it is, `0.11.0 (91f86f9)` from a checkout:
/// set once by the editor and the CLI as they start.
static BUILD_LABEL: OnceLock<String> = OnceLock::new();

/// The binary's version and commit (`greycard_version::KEY`, the label
/// without its count of commits since the tag), so a source build at
/// another commit tries the card again, and a tag fetched later does
/// not. Unset, or a build with no commit (a release, a tarball), the
/// key falls back to the version.
pub fn set_build_label(label: &str) {
    let _ = BUILD_LABEL.set(label.to_string());
}

/// What names this build in the off-card record: the provider record's
/// fingerprint (providers, version, ONNX Runtime's build) and the
/// binary's label where it was given.
fn build_key(providers: &[Provider]) -> String {
    let fingerprint = record::build_fingerprint(providers);
    match BUILD_LABEL.get() {
        Some(label) => format!("{fingerprint} {label}"),
        None => fingerprint,
    }
}

/// Whether this process is a test's: its off-card checks then ask no
/// adapter, load no ONNX Runtime, and read no user's store, and the card
/// and store are what a test names on its thread ([`set_test_here`],
/// [`set_test_store`]). Off in every binary, whatever its features, so
/// a binary always runs the real path; a test process turns it on
/// ([`off_card_for_tests`]). This crate's own tests start with it on.
static TESTS: AtomicBool = AtomicBool::new(cfg!(test));

/// This process is a test's ([`TESTS`]): for a helper every test of a
/// crate built on this one passes through, never for a binary.
#[cfg(any(test, feature = "testing"))]
pub fn off_card_for_tests() {
    TESTS.store(true, Ordering::SeqCst);
}

fn tests() -> bool {
    TESTS.load(Ordering::SeqCst)
}

/// Whether [`off_card_for_tests`] has run in this process: for a test
/// crate to check its own opt-out.
#[cfg(any(test, feature = "testing"))]
pub fn off_card_is_for_tests() -> bool {
    tests()
}

/// The WebGPU adapter this launch runs on and this build's key, or none
/// where WebGPU is not among `providers`, ONNX Runtime cannot be had,
/// or the adapter cannot be named. In a test process ([`TESTS`]), what
/// the test set on its thread, and with nothing set, none.
fn here(providers: &[Provider]) -> Option<(String, String)> {
    #[cfg(any(test, feature = "testing"))]
    HERE_ASKED.with(|n| n.set(n.get() + 1));
    if tests() {
        return test_here();
    }
    if !providers.contains(&Provider::WebGpu) || super::init().is_err() {
        return None;
    }
    let adapter = Provider::WebGpu.adapter_identity()?;
    Some((adapter, build_key(providers)))
}

/// In a test process with no card named on this thread: nothing is off,
/// and no record is read.
fn nothing_named() -> bool {
    tests() && test_here().is_none()
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static TEST_HERE: std::cell::RefCell<Option<(String, String)>> =
        const { std::cell::RefCell::new(None) };
    static TEST_STORE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
    /// How many times [`here`] was asked on this thread: in a binary,
    /// each ask loads ONNX Runtime and names the adapter.
    static HERE_ASKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many times this thread asked for the card this launch runs on,
/// which in a binary loads ONNX Runtime and names the adapter: for a
/// test that a thread, the window's, never does.
#[cfg(any(test, feature = "testing"))]
pub fn here_asked_on_this_thread() -> u64 {
    HERE_ASKED.with(|n| n.get())
}

/// The store is read again at the next ask of [`denoisers_off_card`],
/// as at a process's start: for a test of the first read.
#[cfg(any(test, feature = "testing"))]
pub fn read_store_again_for_tests() {
    *STORE_READ.lock().unwrap_or_else(|e| e.into_inner()) = false;
}

#[cfg(any(test, feature = "testing"))]
fn test_here() -> Option<(String, String)> {
    TEST_HERE.with(|h| h.borrow().clone())
}

#[cfg(not(any(test, feature = "testing")))]
fn test_here() -> Option<(String, String)> {
    None
}

#[cfg(any(test, feature = "testing"))]
fn test_store() -> Option<PathBuf> {
    TEST_STORE.with(|s| s.borrow().clone())
}

#[cfg(not(any(test, feature = "testing")))]
fn test_store() -> Option<PathBuf> {
    None
}

/// The card and build this thread's off-card checks take as this
/// launch's, in a test process; `None`, nothing is off and nothing is
/// read.
#[cfg(any(test, feature = "testing"))]
pub fn set_test_here(here: Option<(&str, &str)>) {
    TEST_HERE.with(|h| *h.borrow_mut() = here.map(|(a, b)| (a.to_string(), b.to_string())));
}

/// The model store this thread's first ask of [`denoisers_off_card`]
/// reads, in a test process, in the user's store's place; `None`, none
/// is read.
#[cfg(any(test, feature = "testing"))]
pub fn set_test_store(root: Option<&Path>) {
    TEST_STORE.with(|s| *s.borrow_mut() = root.map(Path::to_path_buf));
}

impl OnCard {
    /// For a session `remember` names, settled on `provider` of
    /// `providers`: a denoiser tier on WebGPU, its adapter named. `None`
    /// otherwise.
    pub(crate) fn of(
        remember: Option<Remembered<'_>>,
        provider: Provider,
        providers: &[Provider],
    ) -> Option<Self> {
        let r = remember.filter(|r| provider == Provider::WebGpu && is_denoiser(r.model))?;
        let (adapter, build) = here(providers)?;
        Some(Self {
            store_root: r.store_root.to_path_buf(),
            entry: Off {
                model: r.model.to_string(),
                hash: r.hash.to_string(),
                adapter,
                build,
            },
        })
    }

    /// One whose adapter and build the caller names itself: a test's,
    /// which asks no hardware.
    pub fn new(store_root: &Path, model: &str, hash: &str, adapter: &str, build: &str) -> Self {
        Self {
            store_root: store_root.to_path_buf(),
            entry: Off {
                model: model.to_string(),
                hash: hash.to_string(),
                adapter: adapter.to_string(),
                build: build.to_string(),
            },
        }
    }

    /// The model's registry id.
    pub fn model(&self) -> &str {
        &self.entry.model
    }

    /// Put on the off-card record, and read back: true only when the
    /// record now holds it. A store that cannot be written (read only,
    /// a full disk, a rename refused) says false. Entries for other
    /// cards, drivers, and builds are kept.
    fn put_off(&self) -> bool {
        let mut entries = read_off(&self.store_root);
        if !entries.contains(&self.entry) {
            entries.push(self.entry.clone());
            write_off(&self.store_root, &entries);
        }
        let landed = read_off(&self.store_root).contains(&self.entry);
        let path = off_card_path(&self.store_root);
        if landed {
            log::warn!(
                "{}: the card was lost while it ran; the learned denoiser is not run on this card, driver, and build again (remove {} to try)",
                self.entry.model,
                path.display()
            );
        } else {
            log::warn!(
                "{}: the card was lost while it ran, and {} could not be written",
                self.entry.model,
                path.display()
            );
        }
        landed
    }
}

/// The runs on the card now, and the models this process has put off
/// it.
pub struct Held {
    runs: Mutex<Vec<(u64, OnCard)>>,
    next: AtomicU64,
    off: Mutex<Vec<String>>,
}

impl Default for Held {
    fn default() -> Self {
        Self::new()
    }
}

/// Every run held on the card, any [`Held`]'s: a test's way to see that
/// a run was held, since the run ends before it can look.
static RUNS_HELD: AtomicU64 = AtomicU64::new(0);

/// How many runs have been held on the card in this process.
pub fn runs_held() -> u64 {
    RUNS_HELD.load(Ordering::SeqCst)
}

impl Held {
    pub const fn new() -> Self {
        Self {
            runs: Mutex::new(Vec::new()),
            next: AtomicU64::new(0),
            off: Mutex::new(Vec::new()),
        }
    }

    /// `run`, held as under way on the card while it runs when `on`
    /// names its session there. An error that says the device was lost
    /// ([`device_lost`]) puts the model off the card at once, before the
    /// window may hear of the loss (under `--cpu-ops` it hears at its
    /// next frame).
    pub fn run<T, E: std::fmt::Display>(
        &self,
        on: Option<&OnCard>,
        run: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let Some(on) = on else {
            return run();
        };
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.lock_runs().push((id, on.clone()));
        RUNS_HELD.fetch_add(1, Ordering::SeqCst);
        // Let go however the run ends, a panic too.
        struct Ended<'a>(&'a Held, u64);
        impl Drop for Ended<'_> {
            fn drop(&mut self) {
                self.0.lock_runs().retain(|(id, _)| *id != self.1);
            }
        }
        let ended = Ended(self, id);
        let out = run();
        drop(ended);
        if let Err(e) = &out
            && device_lost(&e.to_string())
        {
            self.put_off(std::slice::from_ref(on));
        }
        out
    }

    /// The runs under way on the card now.
    pub fn now(&self) -> Vec<OnCard> {
        self.lock_runs().iter().map(|(_, on)| on.clone()).collect()
    }

    /// Put each of `runs` off the card ([`OnCard::put_off`]); the models
    /// this process has put off it, these and any before, each once,
    /// those whose record did not land left out.
    pub fn put_off(&self, runs: &[OnCard]) -> Vec<String> {
        let landed: Vec<String> = runs
            .iter()
            .filter(|on| on.put_off())
            .map(|on| on.entry.model.clone())
            .collect();
        let mut off = self.off.lock().unwrap_or_else(|e| e.into_inner());
        for model in landed {
            if is_denoiser(&model) {
                DENOISERS_OFF.store(true, Ordering::SeqCst);
            }
            if !off.contains(&model) {
                off.push(model);
            }
        }
        off.clone()
    }

    /// Try again: the denoisers this process put off the card are let
    /// go, so a later loss with none of them under way is not taken for
    /// theirs.
    fn let_go_of_denoisers(&self) {
        self.off
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|m| !is_denoiser(m));
    }

    fn lock_runs(&self) -> std::sync::MutexGuard<'_, Vec<(u64, OnCard)>> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The process's runs on the card.
pub static HELD: Held = Held::new();

/// The runs under way on the card now ([`HELD`]): what the window's
/// device-lost callback reads, in memory only.
pub fn on_card_now() -> Vec<OnCard> {
    HELD.now()
}

/// [`Held::put_off`] on [`HELD`]: what the quit does with the runs the
/// callback read, off the callback.
pub fn put_off_card(runs: &[OnCard]) -> Vec<String> {
    HELD.put_off(runs)
}

/// Whether a learned denoiser is off this card: read from the record at
/// start ([`read_denoisers_off_card`]), found at a load
/// ([`denoisers_off_here`]), or set when one is put off it in this
/// process. Every tier rests when any is on record: a card that lost
/// under one is not trusted with another.
static DENOISERS_OFF: AtomicBool = AtomicBool::new(false);

/// How many times Try again has cleared [`DENOISERS_OFF`], held while a
/// read sets it or a press clears it: a read begun before a press does
/// not set it after.
static TRIED_AGAIN: Mutex<u64> = Mutex::new(0);

fn tried_again() -> std::sync::MutexGuard<'static, u64> {
    TRIED_AGAIN.lock().unwrap_or_else(|e| e.into_inner())
}

/// [`DENOISERS_OFF`] set by a read begun at `since` ([`TRIED_AGAIN`]'s
/// count then), unless Try again came between; whether it was.
fn set_off_since(since: u64) -> bool {
    let count = tried_again();
    let set = *count == since;
    if set {
        DENOISERS_OFF.store(true, Ordering::SeqCst);
    }
    set
}

/// Whether the store has been read for [`denoisers_off_card`].
static STORE_READ: Mutex<bool> = Mutex::new(false);

/// Whether the learned denoiser is off this card ([`DENOISERS_OFF`]).
/// The user's store is read for it the first time it is asked, so a
/// caller that read nothing at its start (the CLI's merge, a merge made
/// again) has the record too. In a test process, the store a test named
/// on its thread, and none when it named none.
pub fn denoisers_off_card() -> bool {
    read_store_once(|| {
        if tests() {
            test_store()
        } else {
            crate::store::Store::user()
                .ok()
                .map(|s| s.root().to_path_buf())
        }
    });
    DENOISERS_OFF.load(Ordering::SeqCst)
}

/// [`DENOISERS_OFF`] as it stands, the store never read for it: for the
/// window's thread, which must not load ONNX Runtime or ask the adapter
/// (the first read of a record that holds a denoiser does both). The
/// worker reads the record before its first develop and the models
/// probe reads it on its own thread; the window shows the answer again
/// after each.
pub fn denoisers_off_card_known() -> bool {
    DENOISERS_OFF.load(Ordering::SeqCst)
}

/// The store `root` gives read once a process for the record
/// ([`read_denoisers_off_card`]). None given, it is read at a later ask
/// in a test process; a binary with no store does not ask again.
fn read_store_once(root: impl FnOnce() -> Option<PathBuf>) {
    let mut read = STORE_READ.lock().unwrap_or_else(|e| e.into_inner());
    if *read {
        return;
    }
    match root() {
        Some(root) => {
            *read = true;
            read_denoisers_off_card(&root, || {
                static PROVIDERS: OnceLock<Vec<Provider>> = OnceLock::new();
                PROVIDERS.get_or_init(Provider::available)
            });
        }
        None => *read = !tests(),
    }
}

/// Read the off-card record under `store_root` for the card this launch
/// runs on and this build, and keep the answer ([`denoisers_off_card`]).
/// `providers` is asked, and ONNX Runtime loaded for the build's name,
/// only when the record holds a denoiser at all. True when one is off
/// and kept: a Try again pressed while it read wins.
pub fn read_denoisers_off_card(
    store_root: &Path,
    providers: impl FnOnce() -> &'static [Provider],
) -> bool {
    if nothing_named() {
        return false;
    }
    let since = *tried_again();
    read_denoisers_off_since(store_root, || here(providers()), since)
}

/// [`read_denoisers_off_card`] for the card `here` names, begun at
/// `since` ([`TRIED_AGAIN`]).
fn read_denoisers_off_since(
    store_root: &Path,
    here: impl FnOnce() -> Option<(String, String)>,
    since: u64,
) -> bool {
    let entries = read_off(store_root);
    if !entries.iter().any(|e| is_denoiser(&e.model)) {
        return false;
    }
    let off = here().is_some_and(|(a, b)| denoisers_off_in(&entries, &a, &b));
    let kept = off && set_off_since(since);
    if kept {
        log::info!(
            "the learned denoiser is on record as off this card ({})",
            off_card_path(store_root).display()
        );
    }
    kept
}

/// Whether any learned denoiser tier is on the off-card record under
/// `store_root` for the WebGPU card this launch would run on, for this
/// build of `providers`: the gate every tier's load passes
/// (`Denoiser::from_store`). Kept when true.
pub fn denoisers_off_here(store_root: &Path, providers: &[Provider]) -> bool {
    if nothing_named() {
        return false;
    }
    let since = *tried_again();
    let entries = read_off(store_root);
    if !entries.iter().any(|e| is_denoiser(&e.model)) {
        return false;
    }
    denoisers_off_at_since(store_root, here(providers), since)
}

/// [`denoisers_off_here`] for an adapter and build named (`None`: not
/// known, not off).
#[cfg(test)]
pub(crate) fn denoisers_off_at(store_root: &Path, here: Option<(String, String)>) -> bool {
    let since = *tried_again();
    denoisers_off_at_since(store_root, here, since)
}

fn denoisers_off_at_since(store_root: &Path, here: Option<(String, String)>, since: u64) -> bool {
    let off = here.is_some_and(|(a, b)| denoisers_off_in(&read_off(store_root), &a, &b));
    if off {
        set_off_since(since);
    }
    off
}

/// Try the card again (the Noise section's Try again): this card's and
/// this build's entries taken off the record under `store_root`, every
/// other card's, driver's, and build's kept, the learned denoiser let
/// back on in this process, and the denoisers it put off the card let
/// go ([`HELD`]). A loss after it is put on record again as any is. The
/// entries taken off.
pub fn forget_off_card(store_root: &Path, providers: &[Provider]) -> usize {
    if nothing_named() {
        let_denoisers_back_on();
        return 0;
    }
    forget_off_card_at(store_root, here(providers))
}

/// The learned denoiser let back on in this process, the record left
/// as it is: for a window with no model store to take it off.
pub fn let_denoisers_back_on() {
    let mut count = tried_again();
    *count += 1;
    DENOISERS_OFF.store(false, Ordering::SeqCst);
    HELD.let_go_of_denoisers();
}

/// [`forget_off_card`] for an adapter and build named (`None`: not
/// known, nothing taken off).
fn forget_off_card_at(store_root: &Path, here: Option<(String, String)>) -> usize {
    // Held while the file is written and the flag cleared: a read begun
    // before waits, then finds the count moved and sets nothing.
    let mut count = tried_again();
    let mut gone = 0;
    if let Some((adapter, build)) = here {
        let mut entries = read_off(store_root);
        let before = entries.len();
        entries.retain(|e| !(e.adapter == adapter && e.build == build));
        gone = before - entries.len();
        if gone > 0 {
            write_off(store_root, &entries);
            log::info!(
                "the learned denoiser tried on this card again: {gone} entries taken off {}",
                off_card_path(store_root).display()
            );
        }
    }
    *count += 1;
    DENOISERS_OFF.store(false, Ordering::SeqCst);
    drop(count);
    HELD.let_go_of_denoisers();
    gone
}

/// Whether `model` is a denoiser tier's id, on either mosaic: a card
/// lost under one rests them all.
fn is_denoiser(model: &str) -> bool {
    crate::registry::model(model).is_some_and(|m| crate::registry::tier_of(m).is_some())
}

/// Whether `entries` hold any denoiser tier off the card for an adapter
/// and build named.
fn denoisers_off_in(entries: &[Off], adapter: &str, build: &str) -> bool {
    entries
        .iter()
        .any(|e| is_denoiser(&e.model) && e.adapter == adapter && e.build == build)
}

/// What the runtimes say of a lost device, as their binaries on disk
/// read: Dawn's `device_lost` (as in `VK_ERROR_DEVICE_LOST`), its
/// "[device] is lost.", and its "Device lost"; and D3D's removal codes
/// by name. Matched in lower case.
const LOST_WORDS: [&str; 6] = [
    "device_lost",
    "is lost.",
    "device lost",
    "dxgi_error_device_removed",
    "dxgi_error_device_hung",
    "dxgi_error_device_reset",
];

/// Whether an error from a run says its device was lost
/// ([`LOST_WORDS`]).
fn device_lost(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    LOST_WORDS.iter().any(|w| t.contains(w))
}

/// Held by every test that sets the process's flag, so one that reads
/// it falling and rising sees its own.
#[cfg(test)]
pub(crate) fn flag_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{DENOISE_BALANCED, DENOISE_BEST, DENOISE_FAST};

    const ADAPTER: &str = "Test GPU|Vulkan|1.2.3";
    const BUILD: &str = "WebGPU,CPU greycard 0.1.0 (abc1234)";

    fn root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-on-card-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        greycard_core::testing::remove_dir_retry(&dir);
        dir
    }

    fn here() -> Option<(String, String)> {
        Some((ADAPTER.to_string(), BUILD.to_string()))
    }

    fn on(root: &Path, model: &str, adapter: &str, build: &str) -> OnCard {
        OnCard::new(root, model, &format!("{model}-hash"), adapter, build)
    }

    /// A run under way when the device is lost is put off the card, on
    /// record and read back; a run that has ended is not.
    #[test]
    fn a_run_under_way_at_a_loss_is_put_off_the_card() {
        let _flag = flag_lock();
        let root = root("under-way");
        let held = Held::new();
        let best = on(&root, DENOISE_BEST.id, ADAPTER, BUILD);
        let fast = on(&root, DENOISE_FAST.id, ADAPTER, BUILD);
        held.run(Some(&fast), || Ok::<_, String>(())).unwrap();
        assert!(held.now().is_empty());
        let at_loss = held
            .run(Some(&best), || Ok::<_, String>(held.now()))
            .unwrap();
        assert_eq!(at_loss, vec![best.clone()]);
        assert!(held.now().is_empty(), "let go when it ended");
        assert!(!denoisers_off_at(&root, here()));
        assert_eq!(held.put_off(&at_loss), vec![DENOISE_BEST.id.to_string()]);
        assert!(denoisers_off_at(&root, here()));
        // A loss with nothing under way puts nothing off, and says what
        // this process has put off so far.
        assert_eq!(held.put_off(&[]), vec![DENOISE_BEST.id.to_string()]);
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// A record of any tier puts the learned denoiser off the card (the
    /// tiers asked against it are `denoise.rs`'s nine pairs).
    #[test]
    fn a_record_of_any_tier_puts_the_denoiser_off() {
        let _flag = flag_lock();
        for recorded in [DENOISE_FAST.id, DENOISE_BALANCED.id, DENOISE_BEST.id] {
            let root = root("each");
            assert!(
                Held::new()
                    .put_off(&[on(&root, recorded, ADAPTER, BUILD)])
                    .contains(&recorded.to_string())
            );
            assert!(denoisers_off_at(&root, here()), "{recorded} on record");
            greycard_core::testing::remove_dir_retry(&root);
        }
    }

    /// A record for one card stays when another card or another driver
    /// runs the tier, or is lost too: two GPUs, a driver rolled back.
    /// Another adapter, driver, or build is not off; a model that is
    /// not a denoiser rests nothing.
    #[test]
    fn a_record_is_kept_per_card_driver_and_build() {
        let _flag = flag_lock();
        let root = root("per-card");
        let held = Held::new();
        held.put_off(&[on(&root, DENOISE_FAST.id, ADAPTER, BUILD)]);
        // The other GPU, and the same one on a newer driver, lost too.
        let other = "Other GPU|Vulkan|9.9.9";
        let newer = "Test GPU|Vulkan|1.2.4";
        held.put_off(&[on(&root, DENOISE_FAST.id, other, BUILD)]);
        held.put_off(&[on(&root, DENOISE_FAST.id, newer, BUILD)]);
        // A load on either, which writes the provider record's one slot
        // for the file, leaves this record alone.
        let mut providers = vec![record::test_entry(
            DENOISE_FAST.id,
            "fast-hash",
            Provider::WebGpu,
            other,
            BUILD,
            record::Outcome::Worked,
        )];
        record::upsert(
            &mut providers,
            record::test_entry(
                DENOISE_FAST.id,
                "fast-hash",
                Provider::WebGpu,
                newer,
                BUILD,
                record::Outcome::Worked,
            ),
        );
        record::write(&root, &providers);
        assert_eq!(read_off(&root).len(), 3);
        // Back on the first card, the old driver: still off.
        assert!(denoisers_off_at(&root, here()));
        let at = |a: &str, b: &str| denoisers_off_at(&root, Some((a.into(), b.into())));
        assert!(at(other, BUILD));
        assert!(!at("Third GPU|Vulkan|1.0", BUILD));
        assert!(!at("Test GPU|Vulkan|1.2.5", BUILD));
        assert!(!at(ADAPTER, "WebGPU,CPU greycard 0.1.0 (def5678)"));
        assert!(!at(ADAPTER, "WebGPU,CPU greycard 0.1.0"));
        assert!(!denoisers_off_at(&root, None));
        let sky = root.join("sky");
        held.put_off(&[on(&sky, "sky", ADAPTER, BUILD)]);
        assert!(!denoisers_off_at(&sky, here()));
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// A run that fails on a lost device is put off the card at once;
    /// one that fails otherwise, or a panic in it, is not, and is let go.
    #[test]
    fn a_run_failing_on_a_lost_device_is_put_off_at_once() {
        let _flag = flag_lock();
        let root = root("run-error");
        let held = Held::new();
        let best = on(&root, DENOISE_BEST.id, ADAPTER, BUILD);
        assert!(
            held.run(Some(&best), || Err::<(), _>("a shape of 3x3"))
                .is_err()
        );
        assert!(!denoisers_off_at(&root, here()));
        let lost = held.run(Some(&best), || {
            Err::<(), _>("Failed to download data from buffer: Device lost")
        });
        assert!(lost.is_err());
        assert!(denoisers_off_at(&root, here()));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            held.run(Some(&best), || -> Result<(), String> {
                panic!("in the run")
            })
        }));
        assert!(panicked.is_err());
        assert!(held.now().is_empty());
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// A store that cannot be written (its folder a file) puts nothing
    /// off: the record did not land.
    #[test]
    fn a_store_that_cannot_be_written_puts_nothing_off() {
        let base = root("unwritable");
        std::fs::create_dir_all(&base).unwrap();
        let file = base.join("not-a-folder");
        std::fs::write(&file, b"x").unwrap();
        let held = Held::new();
        let best = on(&file.join("store"), DENOISE_BEST.id, ADAPTER, BUILD);
        assert!(held.put_off(&[best]).is_empty());
        greycard_core::testing::remove_dir_retry(&base);
    }

    /// Each of the runtimes' words for a lost device, and none of an
    /// ordinary failure's.
    #[test]
    fn a_lost_device_is_read_from_the_runtimes_words() {
        for lost in [
            "vkQueueSubmit failed with VK_ERROR_DEVICE_LOST",
            "[Device \"greycard\"] is lost.",
            "Failed to download data from buffer: Device lost",
            "D3D12 device was removed: DXGI_ERROR_DEVICE_REMOVED",
            "GetDeviceRemovedReason: DXGI_ERROR_DEVICE_HUNG",
            "DXGI_ERROR_DEVICE_RESET",
        ] {
            assert!(device_lost(lost), "{lost}");
        }
        for ordinary in [
            "Load model from /m/denoise-v20.onnx failed: No such file or directory",
            "[ONNXRuntimeError] : 10 : INVALID_GRAPH : This is an invalid model",
            "Failed to allocate memory for requested buffer of size 1073741824: out of memory",
            "Load model from /home/u/lost+found/lost.onnx failed: no such file",
            "the model answered with the wrong shape",
        ] {
            assert!(!device_lost(ordinary), "{ordinary}");
        }
    }

    /// A record written later in this process is seen by the flag every
    /// caller reads, not only what it held when first asked.
    #[test]
    fn a_record_written_later_is_seen_by_the_flag() {
        let _flag = flag_lock();
        let root = root("later");
        forget_off_card_at(&root, None);
        assert!(!denoisers_off_card(), "nothing on record yet");
        Held::new().put_off(&[on(&root, DENOISE_BALANCED.id, ADAPTER, BUILD)]);
        assert!(denoisers_off_card(), "the write is seen");
        forget_off_card_at(&root, here());
        assert!(!denoisers_off_card());
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// Try again takes this card's and this build's entries off, keeps
    /// every other card's, driver's, and build's, lets the denoiser back
    /// on in this process, and a loss after it is put on record again.
    #[test]
    fn try_again_takes_off_this_cards_entry_alone() {
        let _flag = flag_lock();
        let root = root("try-again");
        let held = Held::new();
        let other = "Other GPU|Vulkan|9.9.9";
        let newer = "Test GPU|Vulkan|1.2.4";
        held.put_off(&[
            on(&root, DENOISE_BEST.id, ADAPTER, BUILD),
            on(&root, DENOISE_FAST.id, ADAPTER, BUILD),
            on(&root, DENOISE_BEST.id, other, BUILD),
            on(&root, DENOISE_BEST.id, newer, BUILD),
            on(&root, DENOISE_BEST.id, ADAPTER, "another build"),
        ]);
        assert!(denoisers_off_card());
        assert_eq!(forget_off_card_at(&root, here()), 2);
        assert!(!denoisers_off_card(), "let back on in this process");
        assert!(!denoisers_off_at(&root, here()));
        let left: Vec<(String, String)> = read_off(&root)
            .into_iter()
            .map(|e| (e.adapter, e.build))
            .collect();
        assert_eq!(
            left,
            vec![
                (other.to_string(), BUILD.to_string()),
                (newer.to_string(), BUILD.to_string()),
                (ADAPTER.to_string(), "another build".to_string()),
            ]
        );
        // Lost again: on record again, by the same path.
        let best = on(&root, DENOISE_BEST.id, ADAPTER, BUILD);
        let at_loss = held
            .run(Some(&best), || Ok::<_, String>(held.now()))
            .unwrap();
        held.put_off(&at_loss);
        assert!(denoisers_off_at(&root, here()));
        assert!(denoisers_off_card());
        forget_off_card_at(&root, here());
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// A record that will not parse reads as none (and says so in the
    /// log); the next write keeps what it writes.
    #[test]
    fn a_record_that_will_not_parse_reads_as_none() {
        let root = root("corrupt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(off_card_path(&root), b"{ not json").unwrap();
        assert!(read_off(&root).is_empty());
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// In a test, with nothing set, nothing is off and no adapter or
    /// store is asked; what a test sets is this thread's.
    #[test]
    fn a_test_reads_no_record_unless_it_names_the_card() {
        assert_eq!(super::here(&[Provider::WebGpu, Provider::Cpu]), None);
        set_test_here(Some((ADAPTER, BUILD)));
        assert_eq!(
            super::here(&[]),
            Some((ADAPTER.to_string(), BUILD.to_string()))
        );
        set_test_here(None);
        assert_eq!(super::here(&[Provider::WebGpu]), None);
    }

    /// The first ask of the flag reads the store (a private one here, the
    /// user's in a binary) for this card's record, once.
    #[test]
    fn the_first_ask_reads_the_store_for_the_record() {
        let _flag = flag_lock();
        let root = root("first-ask");
        let_denoisers_back_on();
        *STORE_READ.lock().unwrap() = false;
        write_off(&root, &[on(&root, DENOISE_FAST.id, ADAPTER, BUILD).entry]);
        set_test_here(Some((ADAPTER, BUILD)));
        set_test_store(Some(&root));
        assert!(denoisers_off_card(), "read from the store named");
        assert!(*STORE_READ.lock().unwrap());
        // Read once: taken off by hand, the flag stands till Try again.
        std::fs::remove_file(off_card_path(&root)).unwrap();
        assert!(denoisers_off_card());
        let_denoisers_back_on();
        set_test_store(None);
        set_test_here(None);
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// The flag as known reads no store and asks for no card, which in a
    /// binary loads ONNX Runtime: the window's thread asks it so. The
    /// first ask of the flag itself asks for the card, once.
    #[test]
    fn the_flag_as_known_asks_for_no_card() {
        let _flag = flag_lock();
        let root = root("known");
        let_denoisers_back_on();
        *STORE_READ.lock().unwrap() = false;
        write_off(&root, &[on(&root, DENOISE_FAST.id, ADAPTER, BUILD).entry]);
        set_test_here(Some((ADAPTER, BUILD)));
        set_test_store(Some(&root));
        let before = here_asked_on_this_thread();
        assert!(!denoisers_off_card_known(), "not read yet");
        assert_eq!(here_asked_on_this_thread(), before);
        assert!(!*STORE_READ.lock().unwrap(), "the store is not read");
        assert!(denoisers_off_card());
        assert_eq!(here_asked_on_this_thread(), before + 1);
        assert!(denoisers_off_card_known(), "known once read");
        assert_eq!(here_asked_on_this_thread(), before + 1);
        let_denoisers_back_on();
        set_test_store(None);
        set_test_here(None);
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// A read that began before Try again sets nothing after it: the
    /// press, between the read's look at the file and its answer, wins.
    #[test]
    fn a_read_begun_before_try_again_sets_nothing_after() {
        let _flag = flag_lock();
        let root = root("race");
        let_denoisers_back_on();
        write_off(&root, &[on(&root, DENOISE_BEST.id, ADAPTER, BUILD).entry]);
        let since = *tried_again();
        let kept = read_denoisers_off_since(
            &root,
            || {
                forget_off_card_at(&root, here());
                here()
            },
            since,
        );
        assert!(!kept);
        assert!(!DENOISERS_OFF.load(Ordering::SeqCst));
        // Begun after it, a read sets it as ever.
        write_off(&root, &[on(&root, DENOISE_BEST.id, ADAPTER, BUILD).entry]);
        let since = *tried_again();
        assert!(read_denoisers_off_since(&root, here, since));
        let_denoisers_back_on();
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// Try again lets go of the denoisers this process put off the card:
    /// a later loss with none under way is not taken for theirs.
    #[test]
    fn try_again_lets_go_of_what_this_process_put_off() {
        let _flag = flag_lock();
        let root = root("let-go");
        let best = on(&root, DENOISE_BEST.id, ADAPTER, BUILD);
        assert!(HELD.put_off(&[best]).contains(&DENOISE_BEST.id.to_string()));
        forget_off_card_at(&root, here());
        assert!(HELD.put_off(&[]).is_empty(), "nothing put off now");
        // Try again with no store to take the record off lets go too.
        let best = on(&root, DENOISE_BEST.id, ADAPTER, BUILD);
        assert!(!HELD.put_off(&[best]).is_empty());
        let_denoisers_back_on();
        assert!(HELD.put_off(&[]).is_empty(), "nothing put off now");
        assert!(!DENOISERS_OFF.load(Ordering::SeqCst));
        let_denoisers_back_on();
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// A load's gate that began before Try again refuses that load (it
    /// read the record) but sets nothing after the press.
    #[test]
    fn a_load_begun_before_try_again_sets_nothing_after() {
        let _flag = flag_lock();
        let root = root("load-race");
        let_denoisers_back_on();
        write_off(&root, &[on(&root, DENOISE_BEST.id, ADAPTER, BUILD).entry]);
        let since = *tried_again();
        // The press, on a store of its own: this one's record stays.
        forget_off_card_at(&root.join("other"), here());
        assert!(denoisers_off_at_since(&root, here(), since));
        assert!(!DENOISERS_OFF.load(Ordering::SeqCst));
        // Begun after it, the gate keeps its answer as ever.
        let since = *tried_again();
        assert!(denoisers_off_at_since(&root, here(), since));
        assert!(DENOISERS_OFF.load(Ordering::SeqCst));
        let_denoisers_back_on();
        greycard_core::testing::remove_dir_retry(&root);
    }
}
