//! Every model at once: the Settings sheet's Models category, a line a
//! model with its license, its size, and where it stands; Fetch all,
//! which fetches those this machine runs one after another through the
//! fetch the model sheet starts, and skips the rest with why; and the
//! same offered once, unprompted, as a question on the model sheet.
//! Nothing is fetched until it is asked for.
//!
//! What runs here is the engine's to say (`ai::Here`), in the words
//! its own refusals use, probed on a thread of its own: the WebGPU
//! adapter's probe can take a slow driver's time, which the window
//! never waits on. Where the AI features are off for want of the
//! CPU-only runtime, that comes first, and the models after it. A
//! fetch that fails is said on its line and the rest go on. There is
//! no stopping a fetch once it has started, here as on the model
//! sheet: what waits is fetched in turn.

use std::collections::VecDeque;

use greycard_ai::{MODELS, Model, Runtime};
#[cfg(not(test))]
use slint::ComponentHandle;
use slint::{ModelRc, VecModel};

use crate::ai::{Here, NeverHere};
use crate::{App, Fetch, ModelLine, State, settings, tr};

/// The fetch of every model, as it stands.
#[derive(Debug, Default)]
pub(crate) struct FetchAll {
    /// The models still to fetch, in turn.
    pub(crate) waiting: VecDeque<&'static Model>,
    /// The CPU-only runtime to fetch before them, and the one coming.
    pub(crate) runtime: Option<&'static Runtime>,
    pub(crate) runtime_coming: Option<&'static Runtime>,
    /// The model coming, and how far it is, in percent.
    pub(crate) coming: Option<(&'static Model, u8)>,
    /// A model the model sheet's Download is fetching meanwhile, and
    /// how far, so its line says so.
    pub(crate) sheet: Option<(&'static Model, u8)>,
    /// The models whose fetch failed, by id, with the error in brief.
    pub(crate) failed: Vec<(&'static str, String)>,
    /// What this machine runs, once its probe is back; a test sets its
    /// own. Whether the probe is out.
    pub(crate) here: Option<Here>,
    pub(crate) probing: bool,
    /// Counts the probes asked for, so an older answer, out when what
    /// runs here was asked again, is dropped.
    pub(crate) probe_generation: u64,
    /// Whether the offer was ever answered (`settings.models_asked`),
    /// and whether it has been made this launch.
    pub(crate) asked: bool,
    pub(crate) asked_now: bool,
    /// What a test's fetches would have sent the worker, in turn.
    #[cfg(test)]
    pub(crate) sent: Vec<&'static str>,
}

/// Where a model stands for a fetch of every one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stands {
    Have,
    Fetch,
    /// To fetch once the CPU-only runtime has come.
    AfterRuntime,
    /// It would never run here, and why.
    Skip(NeverHere),
}

/// Every registered model and where it stands: in the store, to be
/// fetched (after the CPU-only runtime, where `runtime_ok` and that is
/// what this machine lacks), or skipped with the engine's reason. One
/// in the store is had whatever this machine runs.
pub(crate) fn plan(
    here: &Here,
    runtime_ok: bool,
    have: impl Fn(&Model) -> bool,
) -> Vec<(&'static Model, Stands)> {
    let after = here.off.is_some() && here.runtime.is_some() && runtime_ok;
    let cpu = here.with_runtime();
    MODELS
        .iter()
        .map(|m| {
            let stands = if have(m) {
                Stands::Have
            } else if after {
                cpu.why_not(m).map_or(Stands::AfterRuntime, Stands::Skip)
            } else {
                here.why_not(m).map_or(Stands::Fetch, Stands::Skip)
            };
            (m, stands)
        })
        .collect()
}

/// The plan for this machine, or None while its probe is out.
fn plan_of(st: &mut State) -> Option<Vec<(&'static Model, Stands)>> {
    let here = st.sheets.models.here.clone()?;
    // A runtime whose fetch failed this session is not offered again.
    let runtime_ok = here
        .runtime
        .is_some_and(|r| !st.sheets.fetch_failed.contains(&r.id));
    let store = st.develop.store.clone();
    Some(plan(&here, runtime_ok, |m| {
        store.as_ref().is_some_and(|s| s.have(m))
    }))
}

/// Ask what this machine runs, on a thread of its own, unless it is
/// known or asked already; the lines say so meanwhile, and the answer
/// shows them again and makes the offer, if a picture is up. Never in
/// a test, which sets its own.
pub(crate) fn probe(st: &mut State, app: &App) {
    let all = &mut st.sheets.models;
    if all.here.is_some() || all.probing {
        return;
    }
    all.probing = true;
    all.probe_generation += 1;
    #[cfg(not(test))]
    {
        let generation = all.probe_generation;
        let weak = app.as_weak();
        let store = st.develop.store.as_ref().map(|s| s.root().to_path_buf());
        let spawned = std::thread::Builder::new()
            .name("models probe".into())
            .spawn(move || {
                let here = Here::now();
                // Whether the learned denoiser lost this card on an
                // earlier run, for this driver and build: its tiers rest
                // (the read logs it, with the record's path).
                if let Some(root) = store {
                    greycard_ai::runtime::read_denoisers_off_card(&root, crate::ai::providers);
                }
                let _ = weak.upgrade_in_event_loop(move |app| probed(&app, here, generation));
            });
        if let Err(e) = spawned {
            tracing::warn!("models: no thread to probe the machine on: {e}");
            st.sheets.models.probing = false;
        }
    }
    #[cfg(test)]
    let _ = app;
}

/// The probe's answer: kept, shown, and the offer made if it waited.
/// One from before what runs here was asked again is dropped.
#[cfg(not(test))]
fn probed(app: &App, here: Here, generation: u64) {
    let Some(state) = crate::STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    let mut st = state.borrow_mut();
    if generation != st.sheets.models.probe_generation {
        return;
    }
    tracing::info!(
        "models: this machine {}, WebGPU {}, card {}",
        here.off
            .as_deref()
            .map_or("runs them".to_string(), |w| format!("is off ({w})")),
        if here.webgpu { "on" } else { "off" },
        here.card
            .as_ref()
            .map_or("unknown".to_string(), |(b, n)| format!(
                "{n}, {:.1} GiB",
                *b as f64 / (1u64 << 30) as f64
            )),
    );
    st.sheets.models.probing = false;
    st.sheets.models.here = Some(here);
    show(&mut st, app);
    // The card is known now: the Fill's choices greyed where it can
    // never hold the remove.
    crate::panel::retouch::show_patches(&st, &st.develop.edit, app);
    // And the record read: the learned tiers rest on a card the
    // denoiser lost.
    crate::panel::deliver::show_learned_unavailable(app);
    if st.current.is_some() {
        offer_once(&mut st, app);
    }
}

/// What this machine runs may have changed: the CPU-only runtime came,
/// or would not load. Asked again.
fn forget_here(st: &mut State, app: &App) {
    st.sheets.models.here = None;
    st.sheets.models.probing = false;
    probe(st, app);
}

/// A model's bytes as a fetch of every model brings them: on a Mac, a
/// denoiser tier's fp16 file for Core ML with it.
fn bytes(model: &Model) -> u64 {
    let core_ml = if cfg!(target_os = "macos") && !greycard_ai::denoise::fp32_refused() {
        greycard_ai::registry::core_ml_file(model).map_or(0, |f| f.bytes)
    } else {
        0
    };
    model.bytes() + core_ml
}

/// A size in MB under a GB, in GB to a tenth from there.
pub(crate) fn size_words(bytes: u64) -> String {
    if bytes < 999_500_000 {
        format!("{} MB", (bytes + 500_000) / 1_000_000)
    } else {
        format!("{:.1} GB", bytes as f64 / 1e9)
    }
}

/// The name a line shows: the registry's, up to its first comma, as
/// the status line names a model.
fn short(name: &'static str) -> &'static str {
    name.split(',').next().unwrap_or(name)
}

/// `s` with its first letter a capital.
fn capital(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A fetch's error in brief, for a line of two at the most: past the
/// last colon (the URL and the file go), and cut at 28 characters.
pub(crate) fn brief(message: &str) -> String {
    let tail = message.rsplit(": ").next().unwrap_or(message).trim();
    if tail.chars().count() > 28 {
        let cut: String = tail.chars().take(27).collect();
        format!("{}…", cut.trim_end())
    } else {
        tail.to_string()
    }
}

/// The line's words while the CPU-only runtime comes first.
pub(crate) fn after_runtime() -> String {
    // Models list: a model's state
    tr!("after the runtime")
}

/// A line's words for a model to fetch, or any with no store.
pub(crate) fn not_fetched() -> String {
    // Models list: a model's state
    tr!("not downloaded")
}

/// The lines' words while the machine is probed.
pub(crate) fn checking() -> String {
    // Models list: a model's state
    tr!("checking this machine...")
}

/// What a line says of where its model stands.
fn status(all: &FetchAll, model: &Model, stands: &Stands) -> String {
    for (m, pct) in all.coming.iter().chain(&all.sheet) {
        if m.id == model.id {
            // Models list: a model's state
            return tr!("downloading {pct}%", pct = pct);
        }
    }
    if all.waiting.iter().any(|m| m.id == model.id) {
        return if all.runtime.is_some() || all.runtime_coming.is_some() {
            after_runtime()
        } else {
            // Models list: a model's state
            tr!("queued")
        };
    }
    // A failure is said first: a model whose runtime failed under it
    // is skipped now, and the line says what happened.
    if *stands != Stands::Have
        && let Some((_, why)) = all.failed.iter().find(|(id, _)| *id == model.id)
    {
        // Models list: a model's state
        return tr!("failed: {why}", why = why);
    }
    match stands {
        // Models list: a model's state
        Stands::Have => tr!("in the store"),
        // Models list: a model's state
        Stands::Skip(why) => tr!("skipped: {}", why.short()),
        Stands::AfterRuntime => after_runtime(),
        Stands::Fetch => not_fetched(),
    }
}

/// The CPU-only runtime the plan fetches first, if any.
fn runtime_first(here: &Here, plan: &[(&'static Model, Stands)]) -> Option<&'static Runtime> {
    here.runtime
        .filter(|_| plan.iter().any(|(_, s)| *s == Stands::AfterRuntime))
}

/// The bytes still to fetch of what this machine runs, the runtime's
/// download among them where it comes first.
fn left(here: &Here, plan: &[(&'static Model, Stands)]) -> u64 {
    let models: u64 = plan
        .iter()
        .filter(|(_, s)| matches!(s, Stands::Fetch | Stands::AfterRuntime))
        .map(|(m, _)| bytes(m))
        .sum();
    models + runtime_first(here, plan).map_or(0, |r| r.archive.bytes)
}

/// The note under the Fetch all button.
pub(crate) fn note() -> String {
    tr!("Each under its own license, kept beside it in the store.")
}

/// The note with no store to fetch into.
pub(crate) fn no_store() -> String {
    tr!("No cache directory, so nothing can be downloaded.")
}

/// The Models category as it stands: a line a model, and Fetch all
/// with what is left to fetch beside it, pressed once while a fetch of
/// them is under way, and not while anything else is coming.
pub(crate) fn show(st: &mut State, app: &App) {
    if st.sheets.models.here.is_none() && st.develop.store.is_some() {
        probe(st, app);
    }
    let plan = plan_of(st);
    let all = &st.sheets.models;
    let lines: Vec<ModelLine> = MODELS
        .iter()
        .enumerate()
        .map(|(i, m)| ModelLine {
            name: short(m.name).into(),
            purpose: capital(m.purpose).into(),
            license: m.license.name.into(),
            size: size_words(bytes(m)).into(),
            status: match &plan {
                Some(plan) => status(all, m, &plan[i].1),
                // With no store nothing is probed, and nothing fetched.
                None if st.develop.store.is_none() => not_fetched(),
                None => checking(),
            }
            .into(),
        })
        .collect();
    app.set_models_lines(ModelRc::new(VecModel::from(lines)));
    let left = match (&plan, &all.here) {
        (Some(plan), Some(here)) => left(here, plan),
        _ => 0,
    };
    let running = all.coming.is_some()
        || all.runtime_coming.is_some()
        || all.runtime.is_some()
        || !all.waiting.is_empty();
    app.set_models_fetch_label(
        if left > 0 {
            tr!("Download all ({})", size_words(left))
        } else {
            tr!("Download all")
        }
        .into(),
    );
    let store = st.develop.store.is_some();
    app.set_models_fetch_enabled(store && left > 0 && !running && !st.sheets.fetching);
    app.set_models_note(if store { note() } else { no_store() }.into());
}

/// Fetch every model this machine runs that the store lacks, one at a
/// time, the CPU-only runtime first where that is what it lacks, and
/// log each skipped with why. A model whose fetch failed is tried
/// again. Nothing while the machine is probed.
pub(crate) fn fetch_all(st: &mut State, app: &App) {
    if st.develop.store.is_none() {
        return;
    }
    let (Some(plan), Some(here)) = (plan_of(st), st.sheets.models.here.clone()) else {
        return;
    };
    let all = &mut st.sheets.models;
    if all.runtime_coming.is_none()
        && let Some(runtime) = runtime_first(&here, &plan)
    {
        tracing::info!("models: {} first, the models after it", runtime.name);
        all.runtime = Some(runtime);
    }
    let store = st.develop.store.clone();
    let has = |m: &Model| store.as_ref().is_some_and(|s| s.have(m));
    for (m, stands) in plan {
        let all = &mut st.sheets.models;
        match stands {
            Stands::Skip(why) => tracing::info!("models: {} skipped: {}", m.name, why.words()),
            Stands::Fetch | Stands::AfterRuntime => {
                let held = all.waiting.iter().any(|w| w.id == m.id)
                    || all.coming.is_some_and(|(c, _)| c.id == m.id);
                if !held {
                    all.failed.retain(|(id, _)| *id != m.id);
                    all.waiting.push_back(m);
                    tracing::info!("models: {} queued, {}", m.name, size_words(bytes(m)));
                    // The other Subject file in the store gives way to
                    // this one once it lands (`subject::pick`).
                    let other = [&greycard_ai::SUBJECT, &greycard_ai::SUBJECT_WEBGPU]
                        .into_iter()
                        .find(|o| o.id != m.id && m.id.starts_with("birefnet") && has(o));
                    if let Some(other) = other {
                        tracing::info!(
                            "models: once {} lands, the Subject mask uses it, not {}",
                            m.name,
                            other.name
                        );
                    }
                }
            }
            Stands::Have => {}
        }
    }
    pump(st, app);
}

/// Start what is next, the runtime and then the models waiting, unless
/// something is coming already, ours or another's, or the model sheet
/// is up: one fetch at a time, so a Download pressed meanwhile never
/// fetches a model twice. Asked again as each fetch of any kind lands
/// and as the sheet is answered.
pub(crate) fn pump(st: &mut State, app: &App) {
    let all = &st.sheets.models;
    let free = all.coming.is_none()
        && all.runtime_coming.is_none()
        && !st.sheets.fetching
        && st.sheets.fetch.is_none();
    if free {
        if let Some(runtime) = st.sheets.models.runtime.take() {
            st.sheets.models.runtime_coming = Some(runtime);
            st.sheets.fetching = true;
            st.sheets.runtime_next = Some(crate::panel::assets::AfterRuntime::Nothing);
            app.set_status(tr!("downloading {}", short(runtime.name)).into());
            send_runtime(st, runtime);
        } else {
            while let Some(m) = st.sheets.models.waiting.pop_front() {
                if st.develop.store.as_ref().is_some_and(|s| s.have(m)) {
                    tracing::info!("models: {} is in the store already", m.name);
                    continue;
                }
                st.sheets.models.coming = Some((m, 0));
                st.sheets.fetching = true;
                send(st, m, true);
                break;
            }
        }
    }
    show(st, app);
}

/// The model sheet's Download: the model fetched as it always was, and
/// its line says so while it comes.
pub(crate) fn sheet_fetch(st: &mut State, model: &'static Model) {
    st.sheets.fetching = true;
    st.sheets.models.sheet = Some((model, 0));
    send(st, model, false);
}

#[cfg(not(test))]
fn send(_st: &mut State, model: &'static Model, warm: bool) {
    crate::WORKER.with(|w| {
        if let Some(w) = &*w.borrow() {
            w.send(crate::worker::Job::Fetch { model, warm });
        }
    });
}

#[cfg(not(test))]
fn send_runtime(_st: &mut State, runtime: &'static Runtime) {
    crate::WORKER.with(|w| {
        if let Some(w) = &*w.borrow() {
            w.send(crate::worker::Job::FetchRuntime { runtime });
        }
    });
}

/// A test has no business with the network: the fetch is noted.
#[cfg(test)]
fn send(st: &mut State, model: &'static Model, _warm: bool) {
    st.sheets.models.sent.push(model.id);
}

#[cfg(test)]
fn send_runtime(st: &mut State, runtime: &'static Runtime) {
    st.sheets.models.sent.push(runtime.id);
}

/// A file of the model `name` coming: its line ticks, by the whole
/// model's bytes, the files before this one counted as here.
pub(crate) fn progress(st: &mut State, app: &App, name: &str, file: &str, done: u64, total: u64) {
    let all = &mut st.sheets.models;
    let mut moved = false;
    for slot in [&mut all.coming, &mut all.sheet] {
        let Some((m, was)) = *slot else {
            continue;
        };
        if m.name != name {
            continue;
        }
        let before: u64 = m
            .files
            .iter()
            .take_while(|f| f.name != file)
            .map(|f| f.bytes)
            .sum();
        let this = m
            .files
            .iter()
            .find(|f| f.name == file)
            .map_or(total, |f| f.bytes);
        let whole = m.bytes().max(1);
        let pct = (((before + done.min(this)) * 100 + whole / 2) / whole).min(99) as u8;
        if pct != was {
            *slot = Some((m, pct));
            moved = true;
        }
        break;
    }
    if moved {
        show(st, app);
    }
}

/// A model's fetch landed, `failed` with its error or not: when it was
/// ours its line says so, and the next waiting starts whatever came of
/// it.
pub(crate) fn landed(st: &mut State, app: &App, id: &str, failed: Option<&str>) {
    let all = &mut st.sheets.models;
    if all.sheet.is_some_and(|(m, _)| m.id == id) {
        all.sheet = None;
    }
    if let Some((m, _)) = all.coming
        && m.id == id
    {
        all.coming = None;
        match failed {
            Some(message) => {
                tracing::warn!("models: {} failed, the rest go on: {message}", m.name);
                all.failed.push((m.id, brief(message)));
            }
            None => tracing::info!("models: {} fetched", m.name),
        }
        if all.waiting.is_empty() {
            tracing::info!("models: the fetch of every model is over");
        }
    }
    pump(st, app);
}

/// The CPU-only runtime's fetch landed, loaded (`failed` None) or not,
/// whoever asked for it: what this machine runs is asked again. When
/// ours failed, the models that waited on it cannot run, and each line
/// says so.
pub(crate) fn runtime_landed(st: &mut State, app: &App, id: &str, failed: Option<&str>) {
    let all = &mut st.sheets.models;
    if all.runtime_coming.is_some_and(|r| r.id == id) {
        all.runtime_coming = None;
        if let Some(message) = failed {
            tracing::warn!("models: the runtime did not come, so neither do the models: {message}");
            let why = brief(message);
            while let Some(m) = all.waiting.pop_front() {
                all.failed.push((m.id, why.clone()));
            }
        }
    }
    forget_here(st, app);
    pump(st, app);
}

/// The offer's title.
pub(crate) fn offer_title() -> String {
    tr!("Download every model now?")
}

/// The offer's words: how many and how much, that they also come one
/// by one as wanted, the runtime first where it comes first, and what
/// this machine skips, a sentence a reason. The Subject file this
/// machine does not use is neither counted nor said: the one it does
/// use is fetched in its place.
pub(crate) fn offer_text(here: &Here, plan: &[(&'static Model, Stands)]) -> String {
    let fetched: Vec<&Model> = plan
        .iter()
        .filter(|(_, s)| matches!(s, Stands::Fetch | Stands::AfterRuntime))
        .map(|(m, _)| *m)
        .collect();
    let host = fetched
        .first()
        .and_then(|m| m.files[0].url.split('/').nth(2))
        .map_or_else(|| tr!("the model host"), str::to_string);
    let mut text = tr!(
        "{n} model, {size} from {host}. Or each comes as needed, the first time it's used."
            | "{n} models, {size} from {host}. Or each comes as needed, the first time it's used."
                % fetched.len(),
        size = size_words(left(here, plan)),
        host = host
    );
    if let Some(runtime) = runtime_first(here, plan) {
        text.push_str(&tr!(" {} comes first.", short(runtime.name)));
    }
    let mut reasons: Vec<(String, usize)> = Vec::new();
    for (_, stands) in plan {
        if let Stands::Skip(why) = stands
            && !matches!(why, NeverHere::OtherSubject(_))
        {
            let why = why.short();
            match reasons.iter_mut().find(|(r, _)| *r == why) {
                Some((_, n)) => *n += 1,
                None => reasons.push((why, 1)),
            }
        }
    }
    for (why, n) in reasons {
        text.push_str(&tr!("\n\n{n} skipped here: {why}.", n = n, why = why));
    }
    text
}

/// The offer's note: the licenses of what it fetches, by name, and
/// that it is asked once.
pub(crate) fn offer_note(here: &Here, plan: &[(&'static Model, Stands)]) -> String {
    let mut names: Vec<&str> = runtime_first(here, plan)
        .map(|r| r.license.name)
        .into_iter()
        .collect();
    for (m, stands) in plan {
        if matches!(stands, Stands::Fetch | Stands::AfterRuntime)
            && !names.contains(&m.license.name)
        {
            names.push(m.license.name);
        }
    }
    tr!(
        "Licenses: {}; each full text sits beside its model in the store. Asked once; Download all \
         is also in Settings, Models.",
        names.join(", ")
    )
}

/// The status line once the offer is answered As needed.
pub(crate) fn as_needed() -> String {
    tr!("each model comes the first time it's used")
}

/// Offer every model at once, unprompted, the first time a picture is
/// up: once, ever, answered either way; never in a batch run or one
/// the command line scripts, nor with a sheet up or a download on its
/// way (the offer waits for a later picture), nor with nothing left
/// this machine runs. While the machine is probed it waits for the
/// answer, which makes it. Made before the lens profiles' offer, which
/// then waits its turn. Whether it opened.
pub(crate) fn offer_once(st: &mut State, app: &App) -> bool {
    let all = &st.sheets.models;
    if all.asked
        || all.asked_now
        || st.window.batch
        || st.launch.scripted
        || st.develop.store.is_none()
    {
        return false;
    }
    if st.sheets.fetch.is_some() || st.sheets.fetching || app.invoke_sheet_open() {
        return false;
    }
    if all.coming.is_some() || all.runtime_coming.is_some() || !all.waiting.is_empty() {
        return false;
    }
    let (Some(plan), Some(here)) = (plan_of(st), st.sheets.models.here.clone()) else {
        probe(st, app);
        return false;
    };
    let left = left(&here, &plan);
    if left == 0 {
        return false;
    }
    st.sheets.models.asked_now = true;
    tracing::info!("offering every model at once, {}", size_words(left));
    st.sheets.fetch = Some(Fetch::Every);
    app.set_fetch_title(offer_title().into());
    app.set_fetch_text(offer_text(&here, &plan).into());
    app.set_fetch_note(offer_note(&here, &plan).into());
    // The fetch-every-model ask's two buttons
    app.set_fetch_yes(tr!("Now").into());
    // The fetch-every-model ask's two buttons
    app.set_fetch_no(tr!("As needed").into());
    app.set_fetch_no_default(true);
    app.set_fetch_open(true);
    true
}

/// The offer answered, Now (`yes`) or As needed: the sheet's buttons
/// back to their own words for the next offer, the answer kept so it
/// is not asked again, and on Now the fetch of every model.
pub(crate) fn answered(st: &mut State, app: &App, yes: bool) {
    app.set_fetch_yes(tr!("Download").into());
    app.set_fetch_no(tr!("Not now").into());
    app.set_fetch_no_default(false);
    st.sheets.models.asked = true;
    // Not from a batch run, which leaves the user's settings alone, nor
    // from a test.
    if !st.window.batch && !cfg!(test) {
        settings::Settings::change(|s| s.models_asked = true);
    }
    if yes {
        tracing::info!("every model asked for at the offer");
        fetch_all(st, app);
    } else {
        tracing::info!("every model as needed, at the offer");
        app.set_status(as_needed().into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::deliver::deliver;
    use crate::worker::Outcome;
    use greycard_ai::registry::ONNXRUNTIME_CPU_LINUX_X64;
    use greycard_ai::{REMOVE, REMOVE_FAST, SAM, SKY, SUBJECT, SUBJECT_WEBGPU, Store};
    use slint::ComponentHandle as _;
    use slint::Model as _;

    fn gpu() -> Here {
        Here {
            webgpu: true,
            f16: Some((true, "a card".into())),
            card: Some((16 << 30, "a card".into())),
            ..Here::default()
        }
    }

    fn cpu() -> Here {
        Here::default()
    }

    fn off_again() -> Here {
        Here {
            off: Some("this processor has no AVX2".into()),
            runtime: Some(&ONNXRUNTIME_CPU_LINUX_X64),
            ..cpu()
        }
    }

    /// A store in a scratch folder of its own, with `had` in it: each
    /// file at its published size, sparse, which is all `have` asks.
    fn store(name: &str, had: &[&Model]) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "greycard-models-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Store::at(&dir);
        for m in had {
            put(&store, m);
        }
        store
    }

    fn put(store: &Store, m: &Model) {
        std::fs::create_dir_all(store.dir(m)).unwrap();
        for f in m.files {
            let file = std::fs::File::create(store.path(m, f)).unwrap();
            file.set_len(f.bytes).unwrap();
        }
    }

    fn window(here: Here, store: &Store) -> (App, std::rc::Rc<crate::Shared>) {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        {
            let mut st = state.borrow_mut();
            st.develop.store = Some(store.clone());
            st.sheets.models.here = Some(here);
        }
        (app, state)
    }

    fn lines(app: &App) -> Vec<ModelLine> {
        app.get_models_lines().iter().collect()
    }

    fn line(app: &App, model: &Model) -> ModelLine {
        lines(app)
            .into_iter()
            .zip(MODELS)
            .find(|(_, m)| m.id == model.id)
            .map(|(l, _)| l)
            .unwrap()
    }

    /// What is coming and waiting, in turn.
    fn queued(state: &crate::Shared) -> Vec<&'static str> {
        let st = state.borrow();
        let all = &st.sheets.models;
        all.coming
            .iter()
            .map(|(m, _)| m.id)
            .chain(all.waiting.iter().map(|m| m.id))
            .collect()
    }

    fn sent(state: &crate::Shared) -> Vec<&'static str> {
        state.borrow().sheets.models.sent.clone()
    }

    /// What runs on a machine without WebGPU, in the registry's order.
    fn runnable_on_cpu() -> Vec<&'static str> {
        MODELS
            .iter()
            .filter(|m| ![REMOVE.id, REMOVE_FAST.id, SUBJECT_WEBGPU.id].contains(&m.id))
            .map(|m| m.id)
            .collect()
    }

    #[test]
    fn sizes_are_in_megabytes_then_gigabytes() {
        assert_eq!(size_words(113_778_088), "114 MB");
        assert_eq!(size_words(4_932_834), "5 MB");
        assert_eq!(size_words(1_039_142_186), "1.0 GB");
        assert_eq!(size_words(5_475_314_762), "5.5 GB");
    }

    #[test]
    fn an_error_is_said_in_brief() {
        assert_eq!(
            brief("fetching https://huggingface.co/x/y.onnx: connection refused"),
            "connection refused"
        );
        let long = brief("x: a very long reason that goes on and on past any line's room");
        assert!(long.chars().count() <= 28 && long.ends_with('…'), "{long}");
    }

    /// The category lists every model the registry knows, by its name,
    /// what it is for, its license, its size, and where it stands.
    #[test]
    fn the_section_lists_every_model_with_its_size_and_status() {
        let store = store("lists", &[&SKY]);
        let (app, state) = window(cpu(), &store);
        app.set_settings_category("Models".into());
        app.invoke_settings_asked();
        let shown = lines(&app);
        assert_eq!(shown.len(), MODELS.len());
        for (l, m) in shown.iter().zip(MODELS) {
            assert_eq!(l.name, short(m.name));
            assert_eq!(l.size, size_words(bytes(m)));
            assert_eq!(l.license, m.license.name);
            assert!(!l.purpose.is_empty());
        }
        assert_eq!(
            crate::testing::count_id(&app, "SettingsSheet::models-heading"),
            1
        );
        assert_eq!(line(&app, &SKY).status, "in the store");
        assert_eq!(line(&app, &SAM).status, "not downloaded");
        assert_eq!(
            line(&app, &REMOVE).status,
            "skipped: needs a GPU with WebGPU"
        );
        assert_eq!(
            line(&app, &SUBJECT_WEBGPU).status,
            "skipped: the original runs here"
        );
        assert_eq!(line(&app, &SUBJECT).status, "not downloaded");
        // The total is what is left of what runs here.
        let left: u64 = MODELS
            .iter()
            .filter(|m| runnable_on_cpu().contains(&m.id) && m.id != SKY.id)
            .map(bytes)
            .sum();
        let label = format!("Download all ({})", size_words(left));
        assert_eq!(app.get_models_fetch_label(), label.as_str());
        assert!(app.get_models_fetch_enabled());
        // The button above the list, in reach unscrolled in a common
        // laptop's window; the note at the foot in reach too, wrapped
        // lines and all.
        app.window()
            .set_size(slint::LogicalSize::new(1366.0, 768.0));
        crate::testing::settle(&app);
        let button = "SettingsSheet::models-fetch-button";
        assert_eq!(crate::testing::label_of(&app, button), label);
        let (at, size) = crate::testing::by_id(&app, button);
        assert!(
            at.y >= 0.0 && at.y + size.height <= 768.0,
            "{at:?} {size:?}"
        );
        crate::testing::keeps_its_last_line(
            &app,
            &crate::tr!("Each under its own license, kept beside it in the store."),
            "SettingsSheet::pane",
            2400.0,
        );
        // While the machine is probed, the lines say so and the button
        // waits.
        state.borrow_mut().sheets.models.here = None;
        show(&mut state.borrow_mut(), &app);
        assert!(
            lines(&app)
                .iter()
                .all(|l| l.status == "checking this machine...")
        );
        assert!(!app.get_models_fetch_enabled());
        crate::testing::remove_scratch(state, store.root());
    }

    /// Fetch all queues exactly what runs here, one model sent at a
    /// time, and skips the rest with the engine's reasons; a second
    /// press adds nothing.
    #[test]
    fn fetch_all_enqueues_the_runnable_set_and_skips_the_rest() {
        let store = store("runnable", &[]);
        let (app, state) = window(cpu(), &store);
        app.invoke_models_fetch_all();
        assert_eq!(queued(&state), runnable_on_cpu());
        assert_eq!(sent(&state), vec![runnable_on_cpu()[0]], "one at a time");
        assert!(state.borrow().sheets.fetching);
        assert!(!app.get_models_fetch_enabled(), "pressed once");
        assert_eq!(line(&app, &SUBJECT).status, "downloading 0%");
        assert_eq!(line(&app, &SAM).status, "queued");
        // Pressed again (the button greyed, the callback all the same):
        // nothing twice.
        app.invoke_models_fetch_all();
        assert_eq!(queued(&state), runnable_on_cpu());
        assert_eq!(sent(&state).len(), 1);

        // On a GPU the WebGPU Subject file comes in the original's
        // place, and the remove with it.
        let gpu_plan = plan(&gpu(), false, |_| false);
        let stands = |id: &str| gpu_plan.iter().find(|(m, _)| m.id == id).unwrap().1.clone();
        assert_eq!(stands(SUBJECT_WEBGPU.id), Stands::Fetch);
        assert_eq!(stands(REMOVE.id), Stands::Fetch);
        assert_eq!(stands(REMOVE_FAST.id), Stands::Fetch);
        assert_eq!(
            stands(SUBJECT.id),
            Stands::Skip(NeverHere::OtherSubject(true))
        );
        // A small card is refused the remove as `room` refuses it.
        let small = Here {
            card: Some((4 << 30, "a small card".into())),
            ..gpu()
        };
        let small_plan = plan(&small, false, |_| false);
        assert!(matches!(
            small_plan
                .iter()
                .find(|(m, _)| m.id == REMOVE.id)
                .unwrap()
                .1,
            Stands::Skip(NeverHere::ShortCard(..))
        ));
        // With ONNX Runtime off and nothing to fetch for it, nothing
        // runs, and each says why.
        let off = Here {
            off: Some("this processor has no AVX2".into()),
            ..cpu()
        };
        assert!(
            plan(&off, true, |_| false)
                .iter()
                .all(|(_, s)| matches!(s, Stands::Skip(NeverHere::Off(_))))
        );
        crate::testing::remove_scratch(state, store.root());
    }

    /// Each line ticks as its model comes and lands, and a failure is
    /// said on its line while the rest go on.
    #[test]
    fn a_failed_fetch_leaves_the_others_going() {
        let store = store("failed", &[]);
        let (app, state) = window(cpu(), &store);
        app.invoke_models_fetch_all();
        let first = state.borrow().sheets.models.coming.unwrap().0;
        assert_eq!(first.id, SUBJECT.id);
        deliver(
            &app,
            Outcome::Fetching {
                name: first.name,
                file: first.files[0].name,
                done: first.bytes() / 2,
                total: first.bytes(),
            },
        );
        assert_eq!(line(&app, first).status, "downloading 50%");
        deliver(
            &app,
            Outcome::FetchFailed {
                id: first.id,
                name: first.name,
                message: "fetching https://huggingface.co/x: connection refused".into(),
            },
        );
        assert_eq!(line(&app, first).status, "failed: connection refused");
        assert_eq!(sent(&state).len(), 2, "the next one sent");
        let second = state.borrow().sheets.models.coming.unwrap().0;
        assert_eq!(second.id, SAM.id);
        // The second lands: in the store, and the third is sent.
        put(&store, second);
        deliver(&app, Outcome::Fetched { model: second });
        assert_eq!(line(&app, second).status, "in the store");
        assert_eq!(sent(&state).len(), 3);
        assert_eq!(line(&app, first).status, "failed: connection refused");
        crate::testing::remove_scratch(state, store.root());
    }

    /// A model sheet up holds the queue; a Download from it fetches its
    /// model, said on its line, and the queue goes on once that lands,
    /// past the model now in the store. Not now resumes it as well.
    #[test]
    fn the_sheet_holds_the_queue_and_its_download_is_said() {
        let store = store("sheet", &[]);
        let (app, state) = window(cpu(), &store);
        app.invoke_models_fetch_all();
        let first = state.borrow().sheets.models.coming.unwrap().0;
        // A mask offers SAM meanwhile; then the first lands.
        state.borrow_mut().sheets.fetch = Some(Fetch::Model(&SAM));
        app.set_fetch_open(true);
        put(&store, first);
        deliver(&app, Outcome::Fetched { model: first });
        assert_eq!(sent(&state).len(), 1, "held while the sheet is up");
        assert_eq!(line(&app, &SAM).status, "queued");
        // Download: the sheet's fetch, said on its line.
        app.invoke_fetch_answered(true);
        assert_eq!(sent(&state), vec![first.id, SAM.id]);
        assert_eq!(line(&app, &SAM).status, "downloading 0%");
        deliver(
            &app,
            Outcome::Fetching {
                name: SAM.name,
                file: SAM.files[0].name,
                done: 0,
                total: 1,
            },
        );
        assert!(state.borrow().sheets.models.coming.is_none());
        assert_eq!(sent(&state).len(), 2, "one fetch at a time");
        put(&store, &SAM);
        deliver(&app, Outcome::Fetched { model: &SAM });
        assert_eq!(line(&app, &SAM).status, "in the store");
        let third = runnable_on_cpu()[2];
        assert_eq!(sent(&state), vec![first.id, SAM.id, third], "SAM not twice");

        // Not now resumes the queue too.
        state.borrow_mut().sheets.fetch = Some(Fetch::Model(&SKY));
        app.set_fetch_open(true);
        let coming = state.borrow().sheets.models.coming.unwrap().0;
        put(&store, coming);
        deliver(&app, Outcome::Fetched { model: coming });
        assert_eq!(sent(&state).len(), 3);
        app.invoke_fetch_answered(false);
        assert_eq!(sent(&state).len(), 4, "on again once answered");
        crate::testing::remove_scratch(state, store.root());
    }

    /// A Fetch all queued behind a lens download starts when that
    /// lands, failed or not, and the button waits meanwhile.
    #[test]
    fn a_lens_download_holds_the_queue_only_until_it_lands() {
        let store = store("lenses", &[]);
        for landing in [
            Outcome::LensesFetched,
            Outcome::LensesFetchFailed {
                message: "offline".into(),
            },
        ] {
            let (app, state) = window(cpu(), &store);
            state.borrow_mut().sheets.fetching = true;
            show(&mut state.borrow_mut(), &app);
            assert!(!app.get_models_fetch_enabled());
            fetch_all(&mut state.borrow_mut(), &app);
            assert!(sent(&state).is_empty());
            assert_eq!(line(&app, &SUBJECT).status, "queued");
            deliver(&app, landing);
            assert_eq!(sent(&state), vec![SUBJECT.id]);
            assert_eq!(line(&app, &SUBJECT).status, "downloading 0%");
            drop(app);
            drop(state);
        }
        crate::testing::remove_dir_retry(store.root());
    }

    /// Where the CPU-only runtime is what the machine lacks, it comes
    /// first and the models after; once it is here, what runs is asked
    /// again rather than kept from before. Failing, the models it held
    /// say so.
    #[test]
    fn the_runtime_comes_first_and_here_is_asked_again() {
        let store = store("runtime", &[]);
        let runtime = &ONNXRUNTIME_CPU_LINUX_X64;
        let off = Here {
            off: Some("this processor has no AVX2".into()),
            runtime: Some(runtime),
            ..cpu()
        };
        let (app, state) = window(off.clone(), &store);
        show(&mut state.borrow_mut(), &app);
        assert_eq!(line(&app, &SAM).status, "after the runtime");
        assert_eq!(
            line(&app, &REMOVE).status,
            "skipped: needs a GPU with WebGPU"
        );
        app.invoke_models_fetch_all();
        assert_eq!(sent(&state), vec![runtime.id]);
        assert_eq!(queued(&state), runnable_on_cpu());
        assert_eq!(line(&app, &SUBJECT).status, "after the runtime");
        deliver(&app, Outcome::RuntimeFetched { runtime });
        {
            let st = state.borrow();
            assert!(st.sheets.models.here.is_none(), "asked again");
            assert!(st.sheets.models.probing);
        }
        assert_eq!(sent(&state), vec![runtime.id, SUBJECT.id]);
        state.borrow_mut().sheets.models.here = Some(cpu());
        show(&mut state.borrow_mut(), &app);
        assert_eq!(line(&app, &SUBJECT).status, "downloading 0%");
        assert_eq!(line(&app, &SAM).status, "queued");

        // The runtime would not load: its models say so, and none is
        // sent.
        let (app, state) = window(off, &store);
        app.invoke_models_fetch_all();
        deliver(
            &app,
            Outcome::RuntimeUnusable {
                id: runtime.id,
                message: "the library would not load: GLIBC_2.27 not found".into(),
            },
        );
        assert_eq!(sent(&state), vec![runtime.id]);
        assert!(state.borrow().sheets.models.here.is_none());
        let failed = state.borrow().sheets.models.failed.clone();
        assert_eq!(failed.len(), runnable_on_cpu().len());
        // Its lines say what happened, not only that nothing runs.
        state.borrow_mut().sheets.fetch_failed.push(runtime.id);
        state.borrow_mut().sheets.models.here = Some(off_again());
        show(&mut state.borrow_mut(), &app);
        assert_eq!(line(&app, &SAM).status, "failed: GLIBC_2.27 not found");
        assert_eq!(line(&app, &REMOVE).status, "skipped: no ONNX Runtime here");
        crate::testing::remove_scratch(state, store.root());
    }

    /// The offer is made once, As needed its default; answered, it is
    /// not made again. Now fetches what Fetch all does.
    #[test]
    fn the_first_launch_offer_is_made_once_and_now_fetches_the_set() {
        let store = store("offer", &[]);
        let (app, state) = window(cpu(), &store);
        assert!(offer_once(&mut state.borrow_mut(), &app));
        assert!(app.get_fetch_open());
        assert_eq!(app.get_fetch_title(), "Download every model now?");
        assert_eq!(app.get_fetch_yes(), "Now");
        assert_eq!(app.get_fetch_no(), "As needed");
        assert!(app.get_fetch_no_default(), "As needed is the default");
        let text = app.get_fetch_text().to_string();
        assert!(
            text.contains("2 skipped here: needs a GPU with WebGPU."),
            "{text}"
        );
        assert!(text.contains("huggingface.co"), "{text}");
        let note = app.get_fetch_note().to_string();
        assert!(
            note.starts_with("Licenses: MIT, Apache-2.0, SAM License"),
            "{note}"
        );
        assert!(!note.contains("RAIL"), "no remove on this machine: {note}");
        // Not twice in a launch.
        app.set_fetch_open(false);
        state.borrow_mut().sheets.fetch = None;
        assert!(!offer_once(&mut state.borrow_mut(), &app));

        // Escape is As needed: nothing fetched, and never asked again.
        let (app, state) = window(cpu(), &store);
        assert!(offer_once(&mut state.borrow_mut(), &app));
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_fetch_open());
        {
            let st = state.borrow();
            assert!(st.sheets.models.asked);
            assert!(st.sheets.models.sent.is_empty());
            assert!(!st.sheets.fetching);
        }
        assert_eq!(
            app.get_status(),
            "each model comes the first time it's used"
        );
        assert_eq!(
            app.get_fetch_yes(),
            "Download",
            "the sheet's own words back"
        );
        state.borrow_mut().sheets.models.asked_now = false;
        assert!(!offer_once(&mut state.borrow_mut(), &app));

        // Now fetches the same set as Fetch all.
        let (app, state) = window(cpu(), &store);
        assert!(offer_once(&mut state.borrow_mut(), &app));
        app.invoke_fetch_answered(true);
        let now = queued(&state);
        let (app2, state2) = window(cpu(), &store);
        app2.invoke_models_fetch_all();
        assert_eq!(now, queued(&state2));
        assert!(!now.is_empty());
        drop((app, app2, state2));
        crate::testing::remove_scratch(state, store.root());
    }

    /// Never in a batch run or a scripted one, never with nothing left
    /// that runs here, never with another sheet up, and never before
    /// the machine is known.
    #[test]
    fn the_offer_waits_and_is_not_made_for_nothing() {
        let everything: Vec<&Model> = MODELS.iter().collect();
        let full = store("full", &everything);
        let (app, state) = window(cpu(), &full);
        assert!(!offer_once(&mut state.borrow_mut(), &app));
        assert!(!state.borrow().sheets.models.asked_now);
        let empty = store("busy", &[]);
        let (app, state2) = window(cpu(), &empty);
        let attempt = |st: &crate::Shared| offer_once(&mut st.borrow_mut(), &app);
        state2.borrow_mut().sheets.fetching = true;
        assert!(!attempt(&state2));
        state2.borrow_mut().sheets.fetching = false;
        state2.borrow_mut().launch.scripted = true;
        assert!(!attempt(&state2));
        state2.borrow_mut().launch.scripted = false;
        state2.borrow_mut().window.batch = true;
        assert!(!attempt(&state2));
        state2.borrow_mut().window.batch = false;
        state2.borrow_mut().sheets.models.here = None;
        assert!(!attempt(&state2));
        assert!(state2.borrow().sheets.models.probing, "the probe asked for");
        state2.borrow_mut().sheets.models.here = Some(cpu());
        assert!(attempt(&state2));
        crate::testing::remove_scratch(state, full.root());
        crate::testing::remove_scratch(state2, empty.root());
    }

    /// With no store, nothing is probed and the lines do not wait on
    /// it: not fetched, and the note says why.
    #[test]
    fn with_no_store_the_lines_say_not_fetched() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        show(&mut state.borrow_mut(), &app);
        assert!(!state.borrow().sheets.models.probing);
        assert!(lines(&app).iter().all(|l| l.status == "not downloaded"));
        assert_eq!(
            app.get_models_note(),
            "No cache directory, so nothing can be downloaded."
        );
        assert!(!app.get_models_fetch_enabled());
    }
}
