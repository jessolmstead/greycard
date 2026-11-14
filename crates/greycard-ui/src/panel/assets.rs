use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::tr;
use greycard_edit::{Edit, Preset, preset};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::panel::browser::file_name;
use crate::panel::cull::{control_over_frame, leave_cull};
use crate::panel::edit::{develop_soon, read_edit, schedule_save};
use crate::panel::sync;
use crate::worker::{Job, LensReport, Worker};
use crate::{App, Fetch, LookRow, ProfileRow, STATE, Shared, State, chooser, settings};

/// Whether to offer the lens profiles unprompted: only with no
/// database on the machine, never after a Not now, and once a launch.
/// A user who never opens the LENS section would otherwise never
/// learn that nothing is being corrected.
pub(crate) fn offer_lenses_unprompted(database: bool, declined: bool, asked: bool) -> bool {
    !database && !declined && !asked
}

/// The first-launch offer, when `offer_lenses_unprompted` says so and
/// nothing else has the screen: a sheet already up, a download on
/// its way, or a batch run, which nobody is watching and whose
/// capture a sheet would spoil. Called with no database found; the
/// offer waits for a later picture when something is in the way.
pub(crate) fn offer_lenses_once(st: &mut State, app: &App) {
    if !offer_lenses_unprompted(false, st.sheets.lenses_declined, st.sheets.lenses_asked) {
        return;
    }
    // Any sheet: the window's own list of them, so a sheet added
    // later is covered here without being named.
    let busy = st.sheets.fetch.is_some() || st.sheets.fetching || app.invoke_sheet_open();
    if busy || st.window.batch {
        return;
    }
    st.sheets.lenses_asked = true;
    tracing::info!("no lens profiles on this machine: offering the download");
    offer_lenses(st, app, true);
}

/// The first line of the lens profiles' unprompted offer.
pub(crate) fn lenses_why() -> String {
    tr!("No lens profiles installed; lenses aren't corrected.")
}

/// Open the download sheet for the lens database: `unprompted` when
/// no one asked for it, and the sheet says why it is up.
pub(crate) fn offer_lenses(st: &mut State, app: &App, unprompted: bool) {
    st.sheets.fetch = Some(Fetch::Lenses);
    app.set_fetch_title(tr!("Download the lens profiles?").into());
    let details = tr!(
        "The lensfun database, about {size} KB from {host}.
License: {license} ({url}).
Kept in {kept_in}.",
        size = greycard_lens::store::APPROX_BYTES / 1000,
        host = greycard_lens::store::SOURCES[0]
            .split('/')
            .nth(2)
            // Stands in for the lens database host's name when its URL has none
            .map_or_else(|| tr!("its site"), str::to_string),
        license = greycard_lens::store::LICENSE.0,
        url = greycard_lens::store::LICENSE.1,
        kept_in = greycard_lens::Store::user().map_or_else(
            |_| tr!("no cache directory"),
            |s| s.root().display().to_string()
        ),
    );
    // Unasked, the sheet says first why it is up: the reason is the
    // news, and the particulars follow it.
    let text = if unprompted {
        format!("{}\n\n{details}", lenses_why())
    } else {
        details
    };
    app.set_fetch_text(text.into());
    app.set_fetch_note(
        if unprompted {
            tr!("Their own license. Not now won't ask again; it's also in LENS.")
        } else {
            tr!("Their own license.")
        }
        .into(),
    );
    app.set_fetch_open(true);
}

/// A model's size in whole MB, rounded rather than truncated: BiRefNet
/// lite's rewrite is 113,778,088 bytes, which truncating division
/// reads as 113 and rounding as the 114 the note gives.
fn mb(bytes: u64) -> u64 {
    (bytes + 500_000) / 1_000_000
}

/// The model sheet's note on where the model comes from: one greycard
/// changed and publishes itself says so, and under whose license.
pub(crate) fn model_note(model: &greycard_ai::Model) -> String {
    match model.modified {
        Some(_) => tr!(
            "greycard's faster copy; {license} license.",
            license = model.license.name
        ),
        None => tr!("Its own license."),
    }
}

/// Open the model sheet for `model`. `falls_back` is set when this
/// offer is the Subject rewrite standing in for an original already in
/// the store — one on record as falling back to the CPU on this
/// card — rather than a first Subject offer, which the sheet's opening
/// line says plainly instead of leaving the reader to guess why a
/// second Subject file is being offered at all.
pub(crate) fn offer_model(
    st: &mut State,
    app: &App,
    model: &'static greycard_ai::Model,
    falls_back: bool,
) {
    // Not over another sheet, as the lens profiles' offer: an export
    // that lands under Settings would otherwise stack this over it.
    // Dropped; the next need of the model asks again.
    if app.invoke_sheet_open() {
        return;
    }
    offered(st, app, model, falls_back);
}

/// [`offer_model`], and whether the sheet was opened: not where ONNX
/// Runtime cannot be had. Every model runs on it, and there a download
/// would only fill the disk, so the status line says why instead and
/// the model is taken as declined for the session: nothing offers it
/// again, and a People shape waiting on it is let go.
pub(crate) fn offered(
    st: &mut State,
    app: &App,
    model: &'static greycard_ai::Model,
    falls_back: bool,
) -> bool {
    // Where the runtime can be fetched, it is offered first, and the
    // model after it lands.
    if runtime_first(st, app, AfterRuntime::Model(model, falls_back)) {
        return true;
    }
    if let Err(why) = greycard_ai::runtime::status() {
        if !st.sheets.declined.contains(&model.id) {
            st.sheets.declined.push(model.id);
        }
        if model.id == greycard_ai::SAM3.id {
            crate::panel::parts::model_answered(st, app, false);
        }
        app.set_status(why.to_string().into());
        return false;
    }
    st.sheets.fetch = Some(Fetch::Model(model));
    let note = model_note(model);
    app.set_fetch_note(
        if falls_back {
            tr!(
                "Your Subject model runs on the CPU here; a GPU version is {size} MB. {note}",
                size = mb(model.bytes()),
                note = note
            )
        } else {
            note
        }
        .into(),
    );
    app.set_fetch_title(tr!("Download {name}?", name = model.name).into());
    app.set_fetch_text(
        tr!(
            "{size} MB from {host}.
License: {license} ({url}).
Kept in {kept_in}.",
            size = mb(model.bytes()),
            host = model.files[0]
                .url
                .split('/')
                .nth(2)
                // Stands in for the model host's name when its URL has none
                .map_or_else(|| tr!("the model host"), str::to_string),
            license = model.license.name,
            url = model.license.url,
            kept_in = st.develop.store.as_ref().map_or_else(
                || tr!("no cache directory"),
                |s| s.root().display().to_string()
            ),
        )
        .into(),
    );
    app.set_fetch_open(true);
    true
}

/// What the CPU-only runtime's offer holds up, for when it lands: the
/// offer of a model not in the store yet, the Subject model for a
/// Whole person's edge, or nothing but the masks, the People run, the
/// landscape and the develop already waiting, which are asked again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterRuntime {
    Model(&'static greycard_ai::Model, bool),
    Edge(&'static greycard_ai::Model),
    Nothing,
}

/// The sheet's title for the CPU-only runtime.
pub(crate) fn runtime_title() -> String {
    tr!("Download ONNX Runtime for the CPU?")
}

/// Where the AI features are off until Microsoft's CPU-only runtime is
/// fetched, and it was neither declined nor failed to arrive this
/// session: offer it in place of what `next` asks for. Whether the
/// caller is to hold back, because the sheet opened or waits on
/// another sheet or a download in hand; false where there is no such
/// runtime to fetch, and the caller goes on as without the runtime.
pub(crate) fn runtime_first(st: &mut State, app: &App, next: AfterRuntime) -> bool {
    match greycard_ai::runtime::status() {
        Err(why) => runtime_first_for(st, app, why, next),
        Ok(_) => false,
    }
}

/// [`runtime_first`] with why the AI features are off given.
pub(crate) fn runtime_first_for(
    st: &mut State,
    app: &App,
    why: &'static greycard_ai::runtime::Unavailable,
    next: AfterRuntime,
) -> bool {
    let Some(runtime) = why.fetchable() else {
        return false;
    };
    // A batch run has nobody to answer a sheet: off, as before.
    if st.window.batch
        || st.sheets.declined.contains(&runtime.id)
        || st.sheets.fetch_failed.contains(&runtime.id)
    {
        return false;
    }
    if st.sheets.fetch.is_some() || st.sheets.fetching || app.invoke_sheet_open() {
        return true;
    }
    offer_runtime(st, app, why, next);
    true
}

/// Open the model sheet for the CPU-only runtime `why` names: why it
/// is wanted, what it is, its size, Microsoft's license, and that it
/// runs on the CPU alone.
pub(crate) fn offer_runtime(
    st: &mut State,
    app: &App,
    why: &'static greycard_ai::runtime::Unavailable,
    next: AfterRuntime,
) {
    let Some(runtime) = why.fetchable() else {
        return;
    };
    st.sheets.fetch = Some(Fetch::Runtime(why, next));
    app.set_fetch_title(runtime_title().into());
    let reason = why.reason();
    let mut chars = reason.chars();
    let reason = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect::<String>())
        .unwrap_or_default();
    app.set_fetch_text(
        tr!(
            "{reason}, so the AI features are off.

{name} runs them on the CPU only, slower: {download} MB from {host}, {kept} MB kept.
License: {license} ({url}).
Kept in {kept_in}.",
            reason = reason,
            name = runtime.name.split(',').next().unwrap_or(runtime.name),
            download = mb(runtime.archive.bytes),
            host = runtime
                .archive
                .url
                .split('/')
                .nth(2)
                .map_or_else(|| tr!("its site"), str::to_string),
            kept = mb(runtime.bytes()),
            license = runtime.license.name,
            url = runtime.license.url,
            kept_in = st.develop.store.as_ref().map_or_else(
                || tr!("no cache directory"),
                |s| s.root().display().to_string()
            ),
        )
        .into(),
    );
    app.set_fetch_note(
        tr!("Microsoft's own build, as published. Not now keeps the AI features off this session.")
            .into(),
    );
    app.set_fetch_open(true);
}

/// The CPU-only runtime's offer answered Not now: the AI features stay
/// off this session, and the plain line says so, as it did before the
/// runtime could be fetched. The model the offer held up is taken as
/// declined, and a part or a landscape waiting is let go.
fn runtime_declined(
    st: &mut State,
    app: &App,
    why: &'static greycard_ai::runtime::Unavailable,
    next: AfterRuntime,
) {
    if let Some(runtime) = why.fetchable()
        && !st.sheets.declined.contains(&runtime.id)
    {
        st.sheets.declined.push(runtime.id);
    }
    match next {
        AfterRuntime::Model(model, _) => {
            if !st.sheets.declined.contains(&model.id) {
                st.sheets.declined.push(model.id);
            }
        }
        AfterRuntime::Edge(_) => st.sheets.edge_declined = true,
        AfterRuntime::Nothing => {}
    }
    st.develop.part_pending = None;
    st.develop.landscape_pending = None;
    app.set_status(why.to_string().into());
    // The masks waiting are asked again, and say the same line.
    app.window().request_redraw();
}

/// The status line once the Subject model is declined for a Whole
/// person's edge.
pub(crate) fn edge_declined() -> String {
    tr!("Whole person keeps its coarser edge without the Subject model")
}

/// Open the model sheet for the Subject file `model`, for a Whole
/// person's finer edge: the mask is made meanwhile with the coarser
/// one, and the sheet says so.
pub(crate) fn offer_edge(st: &mut State, app: &App, model: &'static greycard_ai::Model) {
    // The runtime first where it can be fetched; the edge's own offer
    // once it lands.
    if runtime_first(st, app, AfterRuntime::Edge(model)) {
        return;
    }
    // Not offered (no runtime here): no sheet to dress.
    if !offered(st, app, model, false) {
        return;
    }
    st.sheets.fetch = Some(Fetch::Edge(model));
    app.set_fetch_note(
        tr!(
            "For a Whole person's finer edge, hair against the sky; the mask is made \
             meanwhile with a coarser one. {note}",
            note = model_note(model)
        )
        .into(),
    );
}

/// The presets' names on the panel. A removal waiting on its Remove
/// is dropped: the row it named may be another preset's now.
pub(crate) fn show_presets(st: &State, app: &App) {
    app.set_preset_confirm_remove(-1);
    app.set_preset_names(ModelRc::new(VecModel::from(
        st.develop
            .presets
            .iter()
            .map(|e| slint::SharedString::from(e.preset.name.as_str()))
            .collect::<Vec<_>>(),
    )));
    crate::panel::preset_list::show(st, app);
}

/// List the store again and show it.
pub(crate) fn refresh_presets(st: &mut State, app: &App) {
    st.develop.presets = st
        .develop
        .preset_store
        .as_ref()
        .map(|s| s.list())
        .unwrap_or_default();
    crate::panel::preset_list::prune(st);
    show_presets(st, app);
    show_presets_missing(st, app);
}

/// The settings' PRESETS row: which shipped presets the store is
/// without, and so whether Restore is offered.
pub(crate) fn show_presets_missing(st: &State, app: &App) {
    let missing = st
        .develop
        .preset_store
        .as_ref()
        .map(|s| s.missing_shipped())
        .unwrap_or_default();
    app.set_presets_missing(missing.len() as i32);
    app.set_presets_missing_words(
        presets_missing_words(st.develop.preset_store.is_some(), &missing).into(),
    );
}

/// The settings' words for the shipped presets not in the store.
pub(crate) fn presets_missing_words(store: bool, missing: &[String]) -> String {
    if !store {
        return tr!("No configuration directory to keep presets in.");
    }
    match missing {
        [] => tr!("Every default preset is in the list."),
        [one] => tr!("{name} is not in the list.", name = one),
        [rest @ .., last] => tr!(
            "{names} and {last} are not in the list.",
            names = rest.join(", "),
            last = last
        ),
    }
}

/// What one file chosen for import came to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Brought {
    /// In the store under `name`, carrying `carried` ("3 sections"),
    /// with Lightroom's names for what had no place here.
    In {
        name: String,
        carried: String,
        passed: Vec<String>,
    },
    /// Left out, and why, as a sentence.
    Refused(String),
}

/// How an import's report is shown: on the status line when it is one
/// preset in and the line has room for it, else on its sheet, with the
/// refusals in red. Either way once, and nothing of it is kept.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Report {
    Status(String),
    Sheet {
        title: String,
        refusals: Vec<String>,
        lines: Vec<String>,
        note: String,
    },
}

/// The most the status line says of an import before the sheet says it
/// instead: about what a narrow window shows of the line.
const STATUS_ROOM: usize = 80;

/// Bring each file into the store through the importer the command
/// line's `--import` uses, show the list again, and say once what came
/// across.
pub(crate) fn import_presets(st: &mut State, app: &App, paths: &[PathBuf]) {
    let Some(store) = st.develop.preset_store.as_ref() else {
        tracing::warn!("preset import: no configuration directory to save into");
        app.set_status(tr!("no configuration directory to keep presets in").into());
        return;
    };
    // A folder is its preset files, each with the group it gives them
    // (`greycard_edit_io::preset::gather`).
    let brought: Vec<Brought> = greycard_edit_io::preset::gather(paths)
        .iter()
        .map(|(p, folder)| bring(store, p, folder.as_deref()))
        .collect();
    refresh_presets(st, app);
    show_import_report(app, import_report(&brought));
}

/// One file into the store, into `folder`'s group unless it names its
/// own, and what it came to.
fn bring(store: &preset::Store, path: &Path, folder: Option<&str>) -> Brought {
    let file = file_name(path);
    // Never over a preset the list has: its bin is the way to a new
    // copy, and a refusal here says so.
    match greycard_edit_io::preset::import_into_group(store, path, false, folder) {
        Ok((written, imported)) => {
            let preset = &imported.preset;
            tracing::info!(
                "preset {} imported from {} into {}",
                preset.name,
                path.display(),
                written.display()
            );
            Brought::In {
                name: preset.name.clone(),
                carried: crate::panel::sync::carried_words(
                    preset.sections.len(),
                    preset.parts.len(),
                ),
                passed: imported.unmapped,
            }
        }
        Err(e) => {
            tracing::warn!("preset import {}: {e}", path.display());
            Brought::Refused(refusal_words(&file, &e))
        }
    }
}

/// Why a file was left out, as the look sheets say a refusal: a
/// sentence, the file named.
pub(crate) fn refusal_words(file: &str, e: &greycard_edit_io::preset::ImportError) -> String {
    use greycard_edit_io::preset::ImportError;
    match e {
        ImportError::Unreadable(e) => {
            tr!("{file} could not be read: {why}.", file = file, why = e)
        }
        ImportError::NotAPreset(why) => {
            tr!("{file} is not a preset: {why}.", file = file, why = why)
        }
        ImportError::Newer(_) => tr!("{file} is {why}.", file = file, why = e),
        ImportError::Nameless => tr!("{file} was not imported: {why}.", file = file, why = e),
        ImportError::Taken(name) => tr!(
            "{file} was not imported: there is a preset called \"{name}\" already.",
            file = file,
            name = name
        ),
        ImportError::Unsaved(_, e) => tr!(
            "{file} could not be written into the list: {why}.",
            file = file,
            why = preset::plain(e)
        ),
    }
}

/// What an import's report says, and where.
pub(crate) fn import_report(brought: &[Brought]) -> Report {
    let line = |name: &str, carried: &str, passed: &[String]| {
        let mut line = format!("{name} ({carried})");
        if !passed.is_empty() {
            line.push_str(&tr!(
                "; no place here for {names}",
                names = passed.join(", ")
            ));
        }
        line
    };
    if let [
        Brought::In {
            name,
            carried,
            passed,
        },
    ] = brought
    {
        let status = tr!("imported {what}", what = line(name, carried, passed));
        if status.chars().count() <= STATUS_ROOM {
            return Report::Status(status);
        }
    }
    let mut refusals = Vec::new();
    let mut lines = Vec::new();
    let mut names = Vec::new();
    let mut passed_any = false;
    for b in brought {
        match b {
            Brought::In {
                name,
                carried,
                passed,
            } => {
                lines.push(line(name, carried, passed));
                names.push(name.as_str());
                passed_any |= !passed.is_empty();
            }
            Brought::Refused(why) => refusals.push(why.clone()),
        }
    }
    let title = match (names.as_slice(), brought.len()) {
        ([name], 1) => tr!("Imported {name}", name = name),
        ([], 1) => tr!("Not imported"),
        ([], _) => tr!("No presets imported"),
        (_, n) if names.len() == n => tr!("Imported {n} preset" | "Imported {n} presets" % n),
        (_, n) => tr!(
            "Imported {done} of {n} preset" | "Imported {done} of {n} presets" % n,
            done = names.len()
        ),
    };
    let note = if passed_any {
        tr!(
            "What had no place here is named as Lightroom names it; set it by hand where a \
             picture wants it."
        )
    } else {
        String::new()
    };
    Report::Sheet {
        title,
        refusals,
        lines,
        note,
    }
}

/// Put the report up: the status line, or the sheet.
pub(crate) fn show_import_report(app: &App, report: Report) {
    match report {
        Report::Status(status) => app.set_status(status.into()),
        Report::Sheet {
            title,
            refusals,
            lines,
            note,
        } => {
            let model = |v: Vec<String>| {
                ModelRc::new(VecModel::from(
                    v.into_iter()
                        .map(slint::SharedString::from)
                        .collect::<Vec<_>>(),
                ))
            };
            // The title in the status line's own case, for when the
            // sheet is gone: "imported 3 presets", a preset's name as
            // it is.
            let mut status = title.clone();
            if let Some(first) = status.get(..1) {
                status.replace_range(..1, &first.to_lowercase());
            }
            app.set_status(status.into());
            app.set_preset_report_title(title.into());
            app.set_preset_report_refusals(model(refusals));
            app.set_preset_report_lines(model(lines));
            app.set_preset_report_note(note.into());
            app.set_preset_report_open(true);
        }
    }
}

/// The picker's first row: the lens the file names, or none.
fn from_the_file() -> String {
    // Lens picker: the first row, the lens the file names
    tr!("From the file")
}

/// The Maker row's first entry: every maker's lenses.
fn all_makers() -> String {
    tr!("lens makers" => "All")
}

/// [`from_the_file`] and [`all_makers`] in English, as the tests read
/// them.
#[cfg(test)]
pub(crate) const FROM_THE_FILE: &str = "From the file";
#[cfg(test)]
pub(crate) const ALL_MAKERS: &str = "All";

/// The Maker row's entries for `options` under the words `filter`:
/// All, then alphabetically each maker of the body's lenses and the
/// profiles, of a lens on another mount the words list (see
/// [`lens_listed`]), and of the lens `chosen`. The words narrow only
/// the off-mount makers, so a maker picked stays while its lenses do.
pub(crate) fn lens_makers(
    options: &[greycard_engine::lens::LensOption],
    chosen: &greycard_edit::LensChoice,
    filter: &str,
) -> Vec<String> {
    let mut makers: Vec<String> = options
        .iter()
        .filter(|o| !o.off_mount || lens_listed(o, filter) || o.choice == *chosen)
        .map(|o| o.maker.clone())
        .filter(|m| !m.is_empty())
        .collect();
    makers.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
    makers.dedup();
    std::iter::once(all_makers()).chain(makers).collect()
}

/// Whether the words `filter` let an option be listed, maker aside:
/// the body's lenses and the profiles when the words match them (no
/// words match all), a lens on another mount (one on an adapter) only
/// when words are typed and match it, so every mount's lenses do not
/// swamp the body's own. An off-mount lens is matched on its name and
/// maker, never the mounts its label adds: "sony" on a Sony body is
/// not every lens calibrated on Sony A.
fn lens_listed(o: &greycard_engine::lens::LensOption, filter: &str) -> bool {
    use greycard_edit::LensChoice;
    if !o.off_mount {
        return lens_matches(&format!("{} {}", o.label, o.maker), filter);
    }
    let name = match &o.choice {
        LensChoice::Database(name) | LensChoice::Lcp(name) => name.as_str(),
        LensChoice::FromFile => return false,
    };
    !filter.trim().is_empty() && lens_matches(&format!("{name} {}", o.maker), filter)
}

/// Whether a row's `label` holds every word of `filter`, case aside.
fn lens_matches(label: &str, filter: &str) -> bool {
    let label = label.to_lowercase();
    filter
        .split_whitespace()
        .all(|w| label.contains(&w.to_lowercase()))
}

/// What the Lens row shows: the keys and labels of its rows, and which
/// is chosen.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LensRows {
    pub(crate) keys: Vec<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) index: usize,
}

/// The Lens row's rows: From the file, then those of `options` by
/// `maker` (every maker's for none) whose label and maker between them
/// hold every word of `filter`, a lens on another mount only when
/// words are typed (see [`lens_listed`]). The lens `chosen` is always
/// among them, first after From the file when the maker or the filter
/// would leave it out; one not among `options` (a preset or a sidecar
/// from another body or another machine) is listed by its name, after
/// the rest, and has no maker.
pub(crate) fn lens_rows(
    options: &[greycard_engine::lens::LensOption],
    chosen: &greycard_edit::LensChoice,
    maker: Option<&str>,
    filter: &str,
) -> LensRows {
    use greycard_edit::LensChoice;
    let mut keys = vec![String::new()];
    let mut labels = vec![from_the_file()];
    for o in options {
        if maker.is_none_or(|m| m == o.maker) && lens_listed(o, filter) {
            keys.push(o.choice.key());
            labels.push(o.label.clone());
        }
    }
    let key = chosen.key();
    let listed = options.iter().find(|o| o.choice == *chosen);
    let label = match (listed, chosen) {
        (Some(o), _) => o.label.clone(),
        (None, LensChoice::Database(name) | LensChoice::Lcp(name)) => name.clone(),
        (None, LensChoice::FromFile) => from_the_file(),
    };
    let index = keys.iter().position(|k| *k == key).unwrap_or_else(|| {
        let shown = listed.is_none() && maker.is_none() && lens_matches(&label, filter);
        let at = if shown { keys.len() } else { 1 };
        keys.insert(at, key);
        labels.insert(at, label);
        at
    });
    LensRows {
        keys,
        labels,
        index,
    }
}

/// The maker the Maker row has chosen; none for All.
fn shown_maker(app: &App) -> Option<String> {
    let index = usize::try_from(app.get_lens_maker_index()).ok()?;
    let maker = app.get_lens_makers().row_data(index)?;
    (index > 0).then(|| maker.to_string())
}

/// Put the Lens row's rows on the panel for the Maker row and the
/// filter as they stand, `chosen` among them; its label.
pub(crate) fn show_lens_rows(
    options: &[greycard_engine::lens::LensOption],
    chosen: &greycard_edit::LensChoice,
    app: &App,
) -> String {
    let maker = shown_maker(app);
    let rows = lens_rows(options, chosen, maker.as_deref(), &app.get_lens_filter());
    let label = rows.labels[rows.index].clone();
    let model = |v: Vec<String>| {
        ModelRc::new(VecModel::from(
            v.into_iter()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>(),
        ))
    };
    app.set_lens_choice_keys(model(rows.keys));
    app.set_lens_choice_labels(model(rows.labels));
    app.set_lens_choice_index(rows.index as i32);
    label
}

/// Put the Maker row's entries on the panel for the filter as it
/// stands, the maker shown kept while it is among them, All when not:
/// the words bring lenses on other mounts, and their makers, in and out.
fn show_lens_makers(
    options: &[greycard_engine::lens::LensOption],
    chosen: &greycard_edit::LensChoice,
    app: &App,
) {
    let makers = lens_makers(options, chosen, &app.get_lens_filter());
    let shown: Vec<String> = app
        .get_lens_makers()
        .iter()
        .map(|m| m.to_string())
        .collect();
    if shown == makers {
        return;
    }
    let index = shown_maker(app)
        .and_then(|m| makers.iter().position(|n| *n == m))
        .unwrap_or(0);
    app.set_lens_makers(ModelRc::new(VecModel::from(
        makers
            .into_iter()
            .map(slint::SharedString::from)
            .collect::<Vec<_>>(),
    )));
    app.set_lens_maker_index(index as i32);
}

/// Whether the open frame is another than the one the LENS filter and
/// Maker row were last set for; it is theirs from here.
pub(crate) fn another_lens_frame(st: &mut State) -> bool {
    let frame = st.current.and_then(|i| st.key_of(i));
    let another = frame != st.develop.lens_frame;
    st.develop.lens_frame = frame;
    another
}

/// After an open that shows the panel an edit with no develop to
/// follow (an offline frame, culling): the filter and the Maker row
/// as a develop's open sets them, when the frame is another.
pub(crate) fn lens_frame_shown(st: &mut State, app: &App) {
    if another_lens_frame(st) {
        let d = &st.develop;
        show_lens_opened(&d.lens, &d.lens_options, &d.edit.lens.chosen, app, true);
    }
}

/// [`show_lens`] for a frame just opened, `opened` when it is another
/// frame than the last: the filter is cleared, and the Maker row shows
/// the maker of the lens the edit chose.
pub(crate) fn show_lens_opened(
    report: &LensReport,
    options: &[greycard_engine::lens::LensOption],
    chosen: &greycard_edit::LensChoice,
    app: &App,
    opened: bool,
) {
    if opened {
        app.set_lens_filter("".into());
    }
    show_lens_as(report, options, chosen, app, opened);
}

/// What the LENS panel says of the open file: the Maker row, the
/// picker with the lens the edit chose, and under it the profile found
/// for what the file names, or why there is none, or the lens picked.
///
/// The Maker row keeps its maker while the list has it, and turns to
/// the chosen lens's when it would hide it; a lens picked under All
/// leaves it at All. A lens the edit picked that is not among
/// `options` is still listed (see [`lens_rows`]), and its name is
/// shown greyed, as a profile not found is.
pub(crate) fn show_lens(
    report: &LensReport,
    options: &[greycard_engine::lens::LensOption],
    chosen: &greycard_edit::LensChoice,
    app: &App,
) {
    show_lens_as(report, options, chosen, app, false);
}

fn show_lens_as(
    report: &LensReport,
    options: &[greycard_engine::lens::LensOption],
    chosen: &greycard_edit::LensChoice,
    app: &App,
    follow: bool,
) {
    let makers = lens_makers(options, chosen, &app.get_lens_filter());
    let chosen_maker = options
        .iter()
        .find(|o| o.choice == *chosen)
        .map(|o| o.maker.clone())
        .filter(|m| !m.is_empty());
    let mut maker = shown_maker(app).filter(|m| makers.contains(m));
    if let Some(m) = chosen_maker
        && (follow || maker.as_ref().is_some_and(|shown| *shown != m))
    {
        maker = Some(m);
    }
    let index = maker
        .and_then(|m| makers.iter().position(|n| *n == m))
        .unwrap_or(0);
    app.set_lens_makers(ModelRc::new(VecModel::from(
        makers
            .into_iter()
            .map(slint::SharedString::from)
            .collect::<Vec<_>>(),
    )));
    app.set_lens_maker_index(index as i32);
    let label = show_lens_rows(options, chosen, app);
    app.set_lens_choice(chosen.key().into());
    app.set_lens_database(!matches!(report, LensReport::NoDatabase));
    if !chosen.is_from_file() {
        app.set_lens_available(options.iter().any(|o| o.choice == *chosen));
        app.set_lens_profile(label.into());
        return;
    }
    app.set_lens_available(matches!(report, LensReport::Found { .. }));
    app.set_lens_profile(
        match report {
            LensReport::NoDatabase => tr!("no lens profiles on this machine"),
            LensReport::NoLensName => tr!("the file names no lens"),
            LensReport::Unknown(name) => tr!("no profile for {name}", name = name),
            LensReport::Found {
                lens,
                camera: true,
                smaller_sensor: false,
            } => lens.clone(),
            LensReport::Found {
                lens,
                camera: true,
                smaller_sensor: true,
            } => tr!("{lens} (measured on a smaller sensor)", lens = lens),
            LensReport::Found {
                lens,
                camera: false,
                ..
            } => tr!("{lens} (body unknown)", lens = lens),
        }
        .into(),
    );
}

/// The open file's camera, as a profile's fit reads it.
fn body_of(st: &State) -> greycard_edit::camera::Body<'_> {
    let d = &st.develop;
    greycard_edit::camera::Body::new(&d.camera.0, &d.camera.1, &d.other_names)
}

/// A profile's row on the CAMERA PROFILE list.
fn profile_row(name: &str, label: String, indented: bool) -> ProfileRow {
    ProfileRow {
        heading: false,
        name: name.into(),
        label: label.into(),
        note: Default::default(),
        folded: false,
        own: false,
        indented,
    }
}

/// What the CAMERA PROFILE section lists for the open file: the
/// file's own calibrations first, then the profiles in the profile
/// directory that were made for this camera, one row a profile however
/// many files hold it, then the chosen one when it is neither. With
/// every camera shown (the switch at the list's foot), every profile,
/// grouped by camera (`greycard_edit::camera::grouped`). The switch is
/// a view of the list: the edit and its picture are the same either
/// way, and a profile the edit names is listed either way.
pub(crate) fn show_profiles(st: &State, chosen: &greycard_edit::camera::ProfileChoice, app: &App) {
    use greycard_edit::camera;
    let body = body_of(st);
    let embedded = chosen.is_embedded();
    let chosen_name = chosen.name().to_string();
    let chosen = chosen_name.as_str();
    // Camera profile picker: the raw's own embedded profile
    let mut rows = vec![profile_row(camera::EMBEDDED, tr!("Embedded"), false)];
    let mut hidden = 0;
    if app.get_camera_profiles_all() {
        let listing = camera::Listing {
            chosen,
            body,
            open: &st.develop.profile_groups_open,
        };
        rows.extend(
            camera::grouped(&st.develop.profiles, listing)
                .into_iter()
                .map(|r| match r {
                    camera::Row::Heading {
                        key,
                        label,
                        count,
                        folded,
                        own,
                        ..
                    } => ProfileRow {
                        heading: true,
                        name: key.into(),
                        label: label.into(),
                        note: count.to_string().into(),
                        folded,
                        own,
                        indented: false,
                    },
                    camera::Row::Profile {
                        name,
                        label,
                        indented,
                    } => profile_row(&name, label, indented),
                }),
        );
    } else {
        let mut shown = Vec::new();
        // One row a profile: a file that is the same profile as another
        // is left out, and the one the edit names stands for its pair.
        for e in camera::listed(&st.develop.profiles, chosen) {
            if e.fits(body) || e.name == chosen {
                shown.push(e);
            } else {
                hidden += 1;
                // The name each side goes by, so a profile hidden by a
                // spelling the maker and the decoder disagree on can be
                // told from one made for another camera.
                tracing::debug!(
                    "camera profile {}: made for {:?}, not listed for {:?}",
                    e.name,
                    e.unique_camera_model.as_deref().unwrap_or("no camera"),
                    body.names()
                );
            }
        }
        // The count, once a listing, so a profile a spelling hides (an
        // alias the body does not know) can be looked for in a default
        // log; each one's names are at debug.
        if hidden > 0 {
            tracing::info!(
                "camera profiles: {hidden} not listed for {:?}, made for other cameras",
                body.names()
            );
        }
        // Two different profiles can go by one name on the same rows:
        // two files made for one body, or for the body under each of
        // its names. The file name beside each tells them apart.
        let labels = camera::told_apart(&shown);
        rows.extend(
            shown
                .iter()
                .zip(labels)
                .map(|(e, label)| profile_row(&e.name, label, false)),
        );
    }
    // A profile the edit names and the directory does not have is
    // still shown, and chosen, so the panel says what the edit says.
    if !embedded && !st.develop.profiles.iter().any(|e| e.name == chosen) {
        rows.push(profile_row(
            chosen,
            tr!("{name} (missing)", name = chosen),
            false,
        ));
    }
    app.set_camera_profile_rows(ModelRc::new(VecModel::from(rows)));
    app.set_camera_profiles_any(!st.develop.profiles.is_empty());
    app.set_camera_profile(chosen.into());
    app.set_camera_profile_note(
        match (st.develop.profiles.len(), hidden) {
            (0, _) => match greycard_edit_io::profiles::store_dir() {
                Some(dir) => tr!(
                    "No profiles yet: put DCP files in {folder}.",
                    folder = dir.display()
                ),
                None => tr!("No profile directory on this machine."),
            },
            (_, 0) => String::new(),
            (_, n) => {
                tr!("{n} more there, for other cameras." | "{n} more there, for other cameras." % n)
            }
        }
        .into(),
    );
    app.set_camera_profile_warning(
        greycard_edit::camera::warning(&st.develop.profiles, body_of(st), chosen_name.as_str())
            .into(),
    );
}

/// The display curve the panel shows, which is the picture's: the
/// LIGHT section's choice, read as the edit reads it.
fn panel_curve(st: &State, app: &App) -> greycard_edit::DisplayCurve {
    greycard_edit::DisplayCurve::from_name(app.get_display_curve().as_str())
        .unwrap_or(st.develop.edit.display_curve)
}

/// The look directory on the panel: None first, then the tables it
/// holds grouped by the body they were fitted for, the open frame's
/// own first (`look::grouped`), with the chosen one selected and a
/// word under the list.
///
/// A table the edit names and the directory has not got is still
/// shown, and chosen, so the panel says what the edit says — the
/// profile list's rule, and for the same reason: a preset or a
/// sidecar from another machine.
///
/// A table fitted under another display curve than the picture's is
/// off for it (`look::gate`): its row says the curve, and when it is
/// the chosen one the section says so and offers the refit.
pub(crate) fn show_looks(st: &State, chosen: &greycard_edit::look::LookLut, app: &App) {
    let chosen_name = chosen.lut.name().to_string();
    let chosen_name = chosen_name.as_str();
    let curve = panel_curve(st, app);
    // The open frame's body as the index and the camera match name it:
    // the decoder's make and model, joined as the panel joins them.
    let camera = greycard_core::raw::camera_name(&st.develop.camera.0, &st.develop.camera.1);
    let listing = greycard_edit::look::Listing {
        chosen: chosen_name,
        camera: &camera,
        curve,
        open: &st.develop.look_groups_open,
    };
    let rows: Vec<LookRow> = greycard_edit::look::grouped(&st.develop.looks, listing)
        .into_iter()
        .map(|r| match r {
            greycard_edit::look::Row::Heading {
                key,
                label,
                count,
                folded,
                own,
                ..
            } => LookRow {
                heading: true,
                name: key.into(),
                label: label.into(),
                note: count.to_string().into(),
                off: false,
                folded,
                own,
                indented: false,
            },
            greycard_edit::look::Row::Look {
                name,
                label,
                curves,
                off,
                indented,
            } => LookRow {
                heading: false,
                name: name.into(),
                label: label.into(),
                note: curves.into(),
                off,
                folded: false,
                own: false,
                indented,
            },
        })
        .collect();
    // Rows put up again report no leave for the one under the pointer.
    crate::panel::look_preview::forget(st, app);
    app.set_look_rows(ModelRc::new(VecModel::from(rows)));
    // No table for the picture's curve, or one an earlier fit wrote:
    // either way the line over the Refit button, which opens the
    // camera match's sheet on the look's group.
    let (mismatch, refit) =
        match greycard_edit::look::mismatch(&st.develop.looks, chosen_name, curve) {
            Some((text, true)) => (text, tr!("Refit under {curve}...", curve = curve.phrase())),
            Some((text, false)) => (text, String::new()),
            None => greycard_edit::look::stale(
                &st.develop.looks,
                chosen_name,
                curve,
                greycard_match::FIT_VERSION,
            )
            .map(|text| (text, tr!("Refit...")))
            .unwrap_or_default(),
        };
    app.set_look_mismatch(mismatch.into());
    app.set_look_refit(refit.into());
    app.set_look_name(chosen_name.into());
    app.set_look_removable(crate::panel::look_remove::removable(
        &st.develop.looks,
        chosen_name,
    ));
    app.set_look_renamable(crate::panel::look_rename::renamable(
        &st.develop.looks,
        chosen_name,
    ));
    app.set_look_strength(chosen.strength);
    // The chosen look's table for the picture's curve, or its first.
    let shown =
        greycard_edit::look::table_for(&st.develop.looks, chosen_name, curve).or_else(|| {
            greycard_edit::look::tables_of(&st.develop.looks, chosen_name)
                .first()
                .copied()
        });
    let note = match shown {
        // What the chosen table is: its kind, its size, and what it
        // says it was made in when that is not the usual sRGB.
        Some(entry) => entry.described(),
        None if st.develop.looks.is_empty() => match greycard_edit_io::looks::store_dir() {
            Some(dir) => tr!(
                "No looks yet. Add .cube or HaldCLUT .png files to {folder}.",
                folder = dir.display()
            ),
            None => tr!("No look directory on this machine."),
        },
        None => String::new(),
    };
    app.set_look_note(note.into());
    app.set_look_warning(greycard_edit::look::warning(&st.develop.looks, chosen_name).into());
    app.set_look_fitted(fitted_note(st, shown).into());
}

/// "fitted on" a body when the chosen look was fitted by the camera
/// match on another body than the open frame's; nothing when it was
/// fitted on this one, names no body, or no frame is open. The look
/// applies either way: a look is a preference, not a measurement, and
/// is never swapped for this body's own.
fn fitted_note(st: &State, shown: Option<&greycard_edit::look::Entry>) -> String {
    let (make, model) = (st.develop.camera.0.as_str(), st.develop.camera.1.as_str());
    if make.is_empty() && model.is_empty() {
        return String::new();
    }
    let camera = greycard_core::raw::camera_name(make, model);
    shown
        .and_then(|e| e.fitted_note(&camera))
        .unwrap_or_default()
}

/// The checks the save sheet opens with over `edit`: of the sections a
/// preset carries by default, those the edit changed; of one with
/// parts, the parts it changed, or all of them when only the section's
/// switch is changed. The masks listed, none checked.
pub(crate) fn preset_checks(edit: &Edit) -> crate::carry::Checks {
    let plain = Edit::default();
    crate::carry::Checks::from_fn(crate::carry::masks_of(edit), |s, j| {
        if !s.by_default() || s.same(edit, &plain) {
            return false;
        }
        let changed: Vec<bool> = s.parts().map(|p| !p.same(edit, &plain)).collect();
        if changed.iter().any(|c| *c) {
            changed[j]
        } else {
            true
        }
    })
}

/// Put the save sheet's checks on the sheet.
pub(crate) fn show_preset_checks(st: &mut State, app: &App) {
    let st = &mut *st;
    let lines = st.sheets.preset_rows.show(&st.sheets.preset_checks);
    app.set_preset_rows(ModelRc::from(st.sheets.preset_rows.model.clone()));
    app.set_preset_lines(lines);
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    // The LENS panel asks for the profiles.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_fetch_lenses(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.sheets.fetch.is_none() && !st.sheets.fetching {
                offer_lenses(&mut st, &app, false);
            }
        });
    }
    // The Maker row or the filter changed: the Maker row lists the
    // makers the words leave, and the Lens row shows the rows they
    // leave, the chosen lens among them.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_lens_rows_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let chosen = greycard_edit::LensChoice::from_key(app.get_lens_choice().as_str());
            show_lens_makers(&st.develop.lens_options, &chosen, &app);
            show_lens_rows(&st.develop.lens_options, &chosen, &app);
        });
    }
    // The model sheet answered.
    {
        let (state, app_weak, worker) = (state.clone(), app.as_weak(), worker.clone());
        app.on_fetch_answered(move |yes| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            app.set_fetch_open(false);
            let Some(fetch) = st.sheets.fetch.take() else {
                return;
            };
            match (fetch, yes) {
                (Fetch::Model(model) | Fetch::Edge(model), true) => {
                    crate::panel::models::sheet_fetch(&mut st, model);
                }
                (Fetch::Every, yes) => crate::panel::models::answered(&mut st, &app, yes),
                (Fetch::Edge(_), false) => {
                    st.sheets.edge_declined = true;
                    app.set_status(edge_declined().into());
                }
                (Fetch::Model(model), false) if model.id == greycard_ai::SAM3.id => {
                    st.sheets.declined.push(model.id);
                    crate::panel::parts::model_answered(&mut st, &app, false);
                    if st.develop.part_pending.is_none() {
                        app.set_status(
                            tr!("without the People model a People shape masks nothing").into(),
                        );
                    }
                }
                // Declining the generative remove, or its four steps,
                // keeps what the Fill had: the Fill in hand goes on as
                // it was, with its hint.
                (Fetch::Model(model), false)
                    if model.id == greycard_ai::REMOVE.id
                        || model.id == greycard_ai::REMOVE_FAST.id =>
                {
                    st.sheets.declined.push(model.id);
                    if st.develop.retouching.is_some() {
                        crate::panel::deliver::show_hint(
                            &app,
                            &mut st,
                            &crate::panel::retouch::retouch_hint(),
                        );
                    }
                }
                (Fetch::Model(model), false) => {
                    st.sheets.declined.push(model.id);
                    let lost = if model.id == greycard_ai::SUBJECT_WEBGPU.id {
                        // The masks waiting on it turn to the original,
                        // which the next frame offers.
                        app.window().request_redraw();
                        tr!("without the GPU model the original Subject model is offered")
                    } else if model.id == greycard_ai::FILL.id {
                        tr!("without the model a fill is left as it was")
                    } else if greycard_ai::DENOISERS.iter().any(|(_, m)| m.id == model.id) {
                        tr!("without the model the engine's own denoise stands in")
                    } else {
                        tr!("without the model the shape masks nothing")
                    };
                    app.set_status(lost.into());
                    // A Landscape press waiting on the sky's model says
                    // what it cannot do instead.
                    if model.id == greycard_ai::SKY.id {
                        crate::panel::landscape::model_answered(&mut st, &app, false);
                    }
                }
                (Fetch::Runtime(why, next), true) => {
                    if let Some(runtime) = why.fetchable() {
                        st.sheets.fetching = true;
                        st.sheets.runtime_next = Some(next);
                        app.set_status(
                            tr!(
                                "downloading {name}",
                                name = runtime.name.split(',').next().unwrap_or("")
                            )
                            .into(),
                        );
                        worker.send(Job::FetchRuntime { runtime });
                    }
                }
                (Fetch::Runtime(why, next), false) => runtime_declined(&mut st, &app, why, next),
                (Fetch::Lenses, true) => {
                    st.sheets.fetching = true;
                    worker.send(Job::FetchLenses);
                }
                (Fetch::Lenses, false) => {
                    app.set_status(
                        tr!("without the profiles the lens is corrected by hand only").into(),
                    );
                    // Asked once: the offer does not come back on its
                    // own, this run or the next. Not from a batch run,
                    // which leaves the user's settings alone, nor from
                    // a test, which has no business with them.
                    st.sheets.lenses_declined = true;
                    if !st.window.batch && !cfg!(test) {
                        settings::Settings::change(|s| s.lenses_declined = true);
                    }
                }
            }
            // A fetch of every model held while the sheet was up goes
            // on, once nothing else is coming.
            crate::panel::models::pump(&mut st, &app);
        });
    }
    // The CAMERA PROFILE section opened: read the directory again, so
    // a profile put there while the editor is running is listed
    // without reopening the file.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_camera_profiles_wanted(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.develop.profiles = greycard_edit_io::profiles::list();
            show_profiles(&st, &st.develop.edit.camera.profile, &app);
            // A profile put there by hand is the edits' that name it.
            crate::edited::looks_moved(&mut st);
        });
    }
    // A camera profile chosen in the panel: the edit says so, the
    // warning line follows, and the picture is developed again.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_camera_profile_picked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.develop.edit = read_edit(&app, &st.develop.edit, st.develop.target);
            // Another camera's profile is the user's to choose, and is
            // applied as any is; the log says so.
            if let greycard_edit::camera::ProfileChoice::Named(name) =
                &st.develop.edit.camera.profile
                && let Some(e) = st.develop.profiles.iter().find(|e| &e.name == name)
                && !e.fits(body_of(&st))
            {
                tracing::info!(
                    "camera profile {name}: made for {:?}, chosen for {} {}",
                    e.unique_camera_model.as_deref().unwrap_or_default(),
                    st.develop.camera.0,
                    st.develop.camera.1
                );
            }
            show_profiles(&st, &st.develop.edit.camera.profile, &app);
            develop_soon(&mut st, state.clone(), worker.clone(), app.as_weak());
        });
    }
    // A camera's group of the profile list folded or opened: kept for
    // as long as the editor runs, as the look list's are. A click on
    // the group holding the chosen profile, which is open whatever it
    // was set to, changes nothing and is not kept.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_camera_profile_group_toggled(move |key| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let key = key.to_string();
            let listing = greycard_edit::camera::Listing {
                chosen: st.develop.edit.camera.profile.name(),
                body: body_of(&st),
                open: &st.develop.profile_groups_open,
            };
            let shown = greycard_edit::camera::grouped(&st.develop.profiles, listing)
                .into_iter()
                .find_map(|r| match r {
                    greycard_edit::camera::Row::Heading {
                        key: k,
                        folded,
                        locked,
                        ..
                    } if k == key => Some((folded, locked)),
                    _ => None,
                });
            let Some((folded, false)) = shown else {
                return;
            };
            st.develop.profile_groups_open.insert(key, folded);
            show_profiles(&st, &st.develop.edit.camera.profile, &app);
        });
    }
    // Every camera's profiles shown, or only this camera's: the list
    // drawn again and the switch kept in the settings file. The edit
    // and its picture are not touched.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_camera_profiles_all_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let on = app.get_camera_profiles_all();
            crate::panel::prefs::keep(&st, |s| s.profiles_all_cameras = on);
            show_profiles(&st, &st.develop.edit.camera.profile, &app);
        });
    }
    // The LOOK section opened: read the directory again, as the
    // profile directory's is read, and ask for the chosen table
    // again, so a file replaced in place is picked up. Asking is
    // cheap: `greycard_edit_io::looks::load` hands back the same table
    // unless its size or its clock moved, and the renderer re-uploads
    // nothing for a table it already holds.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_looks_wanted(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.develop.looks = greycard_edit_io::looks::list();
            st.develop.look_for = None;
            show_looks(&st, &st.develop.edit.look_lut, &app);
            // A table put there or replaced by hand is the edits' that
            // name it.
            crate::edited::looks_moved(&mut st);
            app.window().request_redraw();
        });
    }
    // A group of the look list folded or opened: remembered as open or
    // folded for as long as the editor runs, whichever frame is open,
    // and the list drawn again. A click on the group holding the
    // chosen look, which is open whatever it was set to, changes
    // nothing and is not kept.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_look_group_toggled(move |key| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let key = key.to_string();
            let camera =
                greycard_core::raw::camera_name(&st.develop.camera.0, &st.develop.camera.1);
            let listing = greycard_edit::look::Listing {
                chosen: st.develop.edit.look_lut.lut.name(),
                camera: &camera,
                curve: panel_curve(&st, &app),
                open: &st.develop.look_groups_open,
            };
            let shown = greycard_edit::look::grouped(&st.develop.looks, listing)
                .into_iter()
                .find_map(|r| match r {
                    greycard_edit::look::Row::Heading {
                        key: k,
                        folded,
                        locked,
                        ..
                    } if k == key => Some((folded, locked)),
                    _ => None,
                });
            let Some((folded, false)) = shown else {
                return;
            };
            st.develop.look_groups_open.insert(key, folded);
            show_looks(&st, &st.develop.edit.look_lut, &app);
        });
    }
    // The display curve changed: which tables are off for the picture
    // changed with it.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_look_curve_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            show_looks(&st, &st.develop.edit.look_lut, &app);
        });
    }
    // The refit the section offers for a table fitted under another
    // curve: the camera match's sheet, over that look's group.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_look_refit_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let name = st.develop.edit.look_lut.lut.name().to_string();
            crate::panel::camera_match::open(&mut st, &app, Some(name));
        });
    }
    // A look chosen in the panel: the edit says so, the note and the
    // warning follow, and the viewport redraws. No develop — a look
    // is the finish, and the picture the engine hands over is the
    // same one. A look previewed from its row is the kept one now.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_look_picked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            crate::panel::look_preview::end(&st, &app);
            // A look reached in culling is the way out of it, as
            // every other look control is.
            if st.culling.cull.is_some() {
                let with = control_over_frame(&mut st, &app);
                leave_cull(&mut st, &app, &worker, with);
                return;
            }
            crate::panel::cull::leave_developed(&mut st, &app);
            st.develop.edit = read_edit(&app, &st.develop.edit, st.develop.target);
            show_looks(&st, &st.develop.edit.look_lut, &app);
            schedule_save(&mut st, app.as_weak());
            app.window().request_redraw();
        });
    }
    // The presets: one laid over the edit as a step, one saved from
    // the edit, one brought in from a file, one removed. What a click
    // does is `sync::apply_preset`, so a test can drive the same
    // logic with fake bodies; this closure just resolves the preset
    // and the profile directory `greycard_edit_io::profiles::list`
    // reads for real.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_preset_applied(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if crate::panel::queue_view::refused(&state.borrow(), "a preset") {
                return;
            }
            // Another row clicked is the question left unanswered.
            app.set_preset_confirm_remove(-1);
            let Some(preset) = state
                .borrow()
                .develop
                .presets
                .get(i as usize)
                .map(|e| e.preset.clone())
            else {
                return;
            };
            sync::preset_pressed(&state, &app, &worker, preset);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_save_open(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            if st.current.is_none() {
                return;
            }
            // On to start: what the edit changed, of the sections a
            // preset carries by default, and of a section with parts
            // the parts it changed.
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            drop(st);
            let mut st = state.borrow_mut();
            st.sheets.preset_checks = preset_checks(&edit);
            show_preset_checks(&mut st, &app);
            app.set_preset_name("".into());
            app.set_preset_open(true);
        });
    }
    // The sheet's checks: a row, all of them, and how the masks land.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_row_toggled(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if let Some(target) = st.sheets.preset_rows.target(i) {
                st.sheets.preset_checks.toggle(target);
                show_preset_checks(&mut st, &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_all(move |on| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.sheets.preset_checks.set_all(on);
            show_preset_checks(&mut st, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_join_chosen(move |join| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.sheets.preset_checks.join = join;
            show_preset_checks(&mut st, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_saved(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let name = app.get_preset_name().trim().to_string();
            if name.is_empty() {
                app.set_status(tr!("a preset needs a name").into());
                return;
            }
            let choice = st.sheets.preset_checks.choice();
            if choice.is_empty() {
                app.set_status(tr!("choose what the preset carries").into());
                return;
            }
            // The demosaic as the open frame runs it, which is what its
            // menu shows.
            let edit = crate::panel::sync::as_source(
                read_edit(&app, &st.develop.edit, st.develop.target),
                st.open_mosaic().kind(),
            );
            // A picture's display curve is no choice to keep.
            let mut preset = Preset::from_choice(&name, &edit, &choice);
            if let Some(path) = st.current.and_then(|c| st.path_at(c)) {
                preset = crate::preset_fit::curve_fitted(&preset, path);
            }
            if preset.is_empty() {
                app.set_status(tr!("choose what the preset carries").into());
                return;
            }
            app.set_preset_open(false);
            match st.develop.preset_store.as_ref().map(|s| s.save(&preset)) {
                Some(Ok(path)) => {
                    app.set_status(
                        tr!(
                            "saved {name} ({carried}) to {path}",
                            name = preset.name,
                            carried = crate::panel::sync::carried_words(
                                preset.sections.len(),
                                preset.parts.len()
                            ),
                            path = path.display()
                        )
                        .into(),
                    );
                }
                Some(Err(e)) => {
                    tracing::warn!("preset {}: not saved: {e}", preset.name);
                    app.set_status(tr!("saving {name}: {why}", name = preset.name, why = e).into());
                }
                None => {
                    tracing::warn!(
                        "preset {}: no configuration directory to save into",
                        preset.name
                    );
                    app.set_status(tr!("no configuration directory to save into").into());
                }
            }
            refresh_presets(&mut st, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_deleted(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(entry) = st.develop.presets.get(i as usize).cloned() else {
                return;
            };
            // Its key while it has one: a favorite goes with it.
            let key = st
                .develop
                .preset_store
                .as_ref()
                .and_then(|s| s.key_of(&entry.path));
            match st.develop.preset_store.as_ref().map(|s| s.remove(&entry)) {
                Some(Ok(())) => {
                    if let Some(key) = &key {
                        crate::panel::preset_list::forget(&mut st, key);
                    }
                    let name = &entry.preset.name;
                    // Promised only when the Restore would bring one
                    // of that name back: the shipped one, which need
                    // not be what the user had under the name.
                    let restorable = st.develop.preset_store.as_ref().is_some_and(|s| {
                        s.missing_shipped()
                            .iter()
                            .any(|m| m.eq_ignore_ascii_case(name))
                    });
                    app.set_status(
                        if restorable {
                            tr!("{name} removed; Settings can restore it", name = name)
                        } else {
                            tr!("{name} removed", name = name)
                        }
                        .into(),
                    );
                }
                Some(Err(e)) => {
                    tracing::warn!("preset {}: not removed: {e}", entry.preset.name);
                    app.set_status(
                        tr!("removing {name}: {why}", name = entry.preset.name, why = e).into(),
                    );
                }
                None => {}
            }
            refresh_presets(&mut st, &app);
        });
    }
    // The settings' Restore: the shipped presets the store is without
    // written back, and nothing of the user's touched.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_presets_restore(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .develop
                .preset_store
                .as_ref()
                .map(|s| s.restore_shipped())
            {
                Some(Ok(names)) if names.is_empty() => {
                    app.set_status(tr!("every default preset is in the list").into());
                }
                Some(Ok(names)) => {
                    tracing::info!("presets restored: {}", names.join(", "));
                    app.set_status(tr!("restored {names}", names = names.join(", ")).into());
                }
                Some(Err(e)) => {
                    tracing::warn!("presets: not restored: {e}");
                    app.set_status(tr!("restoring the default presets: {why}", why = e).into());
                }
                None => {
                    app.set_status(tr!("no configuration directory to restore into").into());
                }
            }
            refresh_presets(&mut st, &app);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_preset_import(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let start = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
            let app_weak = app.as_weak();
            app.set_status(tr!("choosing presets to import...").into());
            // As many as the user picks, Lightroom's and this engine's
            // together. A test opens no chooser: it answers as a cancel.
            chooser::choose_open_many(
                tr!("Import presets"),
                start,
                (
                    // The file chooser's filter name for preset files
                    tr!("Presets"),
                    vec![
                        "*.xmp".into(),
                        "*.XMP".into(),
                        "*.gcp".into(),
                        "*.GCP".into(),
                    ],
                ),
                move |chosen| {
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(app) = app_weak.upgrade() else {
                            return;
                        };
                        // The state through its thread-local, as the
                        // worker's results reach it.
                        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                            return;
                        };
                        let mut st = state.borrow_mut();
                        match chosen {
                            Ok(paths) if paths.is_empty() => {
                                app.set_status(tr!("import canceled").into());
                            }
                            Ok(paths) => import_presets(&mut st, &app, &paths),
                            Err(e) => {
                                tracing::warn!("file chooser: {e:#}");
                                app.set_status(chooser::chooser_failed(&e).into());
                            }
                        }
                    });
                },
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use greycard_edit::preset::Section;

    use super::*;

    #[test]
    fn the_lens_profiles_are_offered_once_and_only_without_them() {
        // Every combination: only no database, never declined and
        // not asked yet this launch offers.
        for database in [false, true] {
            for declined in [false, true] {
                for asked in [false, true] {
                    assert_eq!(
                        offer_lenses_unprompted(database, declined, asked),
                        !database && !declined && !asked,
                        "database {database}, declined {declined}, asked {asked}"
                    );
                }
            }
        }
    }

    /// A model's offer waits for no sheet: under Settings it is
    /// dropped, nothing stacked over it; with the sheet gone it is made
    /// (or, with no runtime here, the line says why).
    #[test]
    fn a_model_is_not_offered_over_another_sheet() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        app.set_settings_open(true);
        offer_model(&mut state.borrow_mut(), &app, &greycard_ai::FILL, false);
        assert!(!app.get_fetch_open());
        assert_eq!(state.borrow().sheets.fetch, None);
        assert_eq!(app.get_status(), "");
        app.set_settings_open(false);
        offer_model(&mut state.borrow_mut(), &app, &greycard_ai::FILL, false);
        assert!(app.get_fetch_open() || !app.get_status().is_empty());
    }

    /// The Subject model offered for a Whole person's edge, declined:
    /// the edge is not offered again this session, a Subject shape
    /// would still be offered its own, and the status line speaks of
    /// the Whole person, not of a shape that masks nothing.
    #[test]
    fn a_declined_edge_offer_is_not_made_again_and_says_so() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        offer_edge(&mut state.borrow_mut(), &app, &greycard_ai::SUBJECT_WEBGPU);
        // Without ONNX Runtime nothing is offered: the line is said
        // and the model taken as declined.
        if let Err(why) = greycard_ai::runtime::status() {
            greycard_ai::runtime::absent_in_test(why);
            assert!(!app.get_fetch_open());
            assert_eq!(state.borrow().sheets.fetch, None);
            assert!(
                state
                    .borrow()
                    .sheets
                    .declined
                    .contains(&greycard_ai::SUBJECT_WEBGPU.id)
            );
            assert_eq!(app.get_status(), why.to_string());
            return;
        }
        assert!(app.get_fetch_open());
        assert_eq!(
            state.borrow().sheets.fetch,
            Some(Fetch::Edge(&greycard_ai::SUBJECT_WEBGPU))
        );
        assert!(app.get_fetch_note().contains("Whole person"));
        app.invoke_fetch_answered(false);
        let st = state.borrow();
        assert!(st.sheets.edge_declined);
        assert!(
            st.sheets.declined.is_empty(),
            "a Subject shape is still offered"
        );
        assert_eq!(app.get_status(), edge_declined());
    }

    /// Where the processor cannot run greycard's ONNX Runtime and the
    /// registry has Microsoft's for the CPU, the model sheet offers
    /// that first, as a model is offered: why, what it is, its size,
    /// its license, and that it runs on the CPU alone and slower. Not
    /// now keeps the plain line, takes the model it held up as
    /// declined, and nothing offers the runtime again this session.
    #[test]
    fn the_cpu_runtime_is_offered_with_its_license_and_not_now_keeps_the_line() {
        use greycard_ai::runtime::Unavailable;
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let runtime = &greycard_ai::registry::ONNXRUNTIME_CPU_LINUX_X64;
        let name = Some("Intel(R) Core(TM) i5-3450 CPU @ 3.10GHz".to_string());
        let missing = vec!["AVX2", "BMI2", "FMA"];
        let why: &'static Unavailable = Box::leak(Box::new(Unavailable::Fetchable {
            missing: missing.clone(),
            name: name.clone(),
            runtime,
        }));
        let sky = AfterRuntime::Model(&greycard_ai::SKY, false);

        // Off for a reason a download does not mend: nothing offered,
        // and the caller goes on as without the runtime.
        let processor: &'static Unavailable =
            Box::leak(Box::new(Unavailable::Processor { missing, name }));
        assert!(!runtime_first_for(
            &mut state.borrow_mut(),
            &app,
            processor,
            sky
        ));
        assert!(!app.get_fetch_open());

        // Another sheet up: held back, and that sheet left alone.
        state.borrow_mut().sheets.fetch = Some(Fetch::Lenses);
        assert!(runtime_first_for(&mut state.borrow_mut(), &app, why, sky));
        assert_eq!(state.borrow().sheets.fetch, Some(Fetch::Lenses));
        assert!(!app.get_fetch_open());
        state.borrow_mut().sheets.fetch = None;

        // Offered, in place of the Sky model's offer.
        assert!(runtime_first_for(&mut state.borrow_mut(), &app, why, sky));
        assert!(app.get_fetch_open());
        assert_eq!(state.borrow().sheets.fetch, Some(Fetch::Runtime(why, sky)));
        assert_eq!(app.get_fetch_title(), runtime_title());
        let text = app.get_fetch_text().to_string();
        for said in [
            "This processor (Intel(R) Core(TM) i5-3450 CPU @ 3.10GHz) has no AVX2, BMI2 or FMA",
            "the AI features are off",
            "ONNX Runtime 1.28.0 by Microsoft runs them on the CPU only, slower",
            "9 MB from github.com, 25 MB kept",
            "License: MIT (https://github.com/microsoft/onnxruntime/blob/v1.28.0/LICENSE)",
            "Kept in ",
        ] {
            assert!(text.contains(said), "{said:?} not in {text:?}");
        }
        assert!(
            app.get_fetch_note()
                .contains("Not now keeps the AI features off")
        );

        // Not now: the line as before, the model held up declined.
        app.invoke_fetch_answered(false);
        assert!(!app.get_fetch_open());
        {
            let st = state.borrow();
            assert_eq!(st.sheets.fetch, None);
            assert!(!st.sheets.fetching);
            assert!(st.sheets.declined.contains(&runtime.id));
            assert!(st.sheets.declined.contains(&greycard_ai::SKY.id));
        }
        assert_eq!(app.get_status(), why.to_string());
        assert!(
            app.get_status()
                .contains(greycard_ai::runtime::FETCH_RUNTIME)
        );
        // Asked once a session.
        assert!(!runtime_first_for(
            &mut state.borrow_mut(),
            &app,
            why,
            AfterRuntime::Nothing
        ));
        assert!(!app.get_fetch_open());
    }

    /// A batch run (a capture, an export with nobody at the window)
    /// never opens the runtime's sheet: the AI features are off, as
    /// without it.
    #[test]
    fn a_batch_run_is_not_offered_the_cpu_runtime() {
        use greycard_ai::runtime::Unavailable;
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let why: &'static Unavailable = Box::leak(Box::new(Unavailable::Fetchable {
            missing: vec!["AVX2"],
            name: None,
            runtime: &greycard_ai::registry::ONNXRUNTIME_CPU_WINDOWS_X64,
        }));
        state.borrow_mut().window.batch = true;
        assert!(!runtime_first_for(
            &mut state.borrow_mut(),
            &app,
            why,
            AfterRuntime::Nothing
        ));
        assert!(!app.get_fetch_open());
        assert_eq!(state.borrow().sheets.fetch, None);
    }

    /// Where greycard ships no ONNX Runtime at all (Windows on ARM),
    /// the same sheet offers Microsoft's, and its why is the platform,
    /// not a processor short of anything.
    #[test]
    fn a_platform_with_no_runtime_of_ours_is_offered_microsofts() {
        use greycard_ai::runtime::Unavailable;
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let runtime = &greycard_ai::registry::ONNXRUNTIME_CPU_WINDOWS_ARM64;
        let why: &'static Unavailable = Box::leak(Box::new(Unavailable::NotShipped {
            runtime: Some(runtime),
        }));
        let sky = AfterRuntime::Model(&greycard_ai::SKY, false);
        assert!(runtime_first_for(&mut state.borrow_mut(), &app, why, sky));
        assert!(app.get_fetch_open());
        assert_eq!(state.borrow().sheets.fetch, Some(Fetch::Runtime(why, sky)));
        let text = app.get_fetch_text().to_string();
        for said in [
            "No ONNX Runtime of greycard's own is made for this platform, so the AI features \
             are off.",
            "ONNX Runtime 1.28.0 by Microsoft runs them on the CPU only, slower",
            "80 MB from github.com, 16 MB kept",
            "License: MIT",
        ] {
            assert!(text.contains(said), "{said:?} not in {text:?}");
        }
        for wrong in ["processor", "AVX2"] {
            assert!(!text.contains(wrong), "{wrong:?} in {text:?}");
        }
        // Not now: the plain line names the fetch command.
        app.invoke_fetch_answered(false);
        assert!(state.borrow().sheets.declined.contains(&runtime.id));
        assert!(
            app.get_status()
                .contains(greycard_ai::runtime::FETCH_RUNTIME)
        );
    }

    #[test]
    fn the_first_launch_offer_opens_the_sheet_once() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);

        // Something else on screen: not now, and not spent either.
        app.set_export_open(true);
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open());
        assert!(!state.borrow().sheets.lenses_asked);
        app.set_export_open(false);
        // The settings sheet too: an offer under it would be answered
        // Not now, for good, by the Escape meant for the settings.
        app.invoke_settings_asked();
        assert!(app.get_settings_open());
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open(), "not under the settings");
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_settings_open());
        assert!(
            !state.borrow().sheets.lenses_declined,
            "Escape closed the settings only"
        );
        assert!(!state.borrow().sheets.lenses_asked);

        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(app.get_fetch_open(), "offered");
        assert_eq!(state.borrow().sheets.fetch, Some(Fetch::Lenses));
        assert!(app.get_fetch_text().starts_with(&lenses_why()));

        // Not now: remembered, and the next picture asks nothing.
        app.invoke_fetch_answered(false);
        assert!(!app.get_fetch_open());
        assert!(state.borrow().sheets.lenses_declined);
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open(), "asked once");

        // A launch that was never declined still asks only once.
        {
            let mut st = state.borrow_mut();
            st.sheets.lenses_declined = false;
            st.sheets.lenses_asked = true;
        }
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open(), "once a launch");

        // The LENS section's own button is not the offer, and asks
        // whatever was answered before.
        app.invoke_fetch_lenses();
        assert!(app.get_fetch_open());
        assert!(!app.get_fetch_text().contains(&lenses_why()));
    }

    #[test]
    fn the_missing_presets_are_named() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            presets_missing_words(true, &[]),
            "Every default preset is in the list."
        );
        assert_eq!(
            presets_missing_words(true, &names(&["Muted Slide"])),
            "Muted Slide is not in the list."
        );
        assert_eq!(
            presets_missing_words(true, &names(&["A", "B", "C"])),
            "A, B and C are not in the list."
        );
        assert!(presets_missing_words(false, &[]).starts_with("No configuration"));
    }

    /// The bin and the Remove as a pointer clicks them: the bin arms
    /// the row and removes nothing, Escape and Cancel disarm it, and
    /// only the Remove removes. Found by sweeping the presets list's
    /// bin column, top down and never past it: the pane scrolls its
    /// sections at their full height now (no more squeezing one to
    /// fit), so a sweep with no floor of its own could wander past
    /// the list and into what is below the section, or even below the
    /// pane — Settings, Report — a click that must never land here.
    ///
    /// Arming the row grows the section by the question and the
    /// Cancel/Remove row, and that growth is `Section`'s 120ms
    /// `animate full`; the mock clock this backend runs on does not
    /// move on its own, so without a nudge the section's clip stays
    /// at its unarmed size and the Remove is clipped away with
    /// everything below it. `mock_elapsed_time` past 120ms is that
    /// nudge, exactly as other pointer tests in this crate use it to
    /// settle an animation before reading where things landed.
    #[test]
    fn the_bin_asks_and_the_remove_removes() {
        let app = crate::testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = crate::testing::retouch_state(&app);
        let d = std::env::temp_dir().join(format!("greycard-ui-bin-{}", std::process::id()));
        crate::testing::remove_dir_retry(&d);
        let store = preset::Store::at(d.clone());
        store.seed();
        state.borrow_mut().develop.preset_store = Some(store);
        refresh_presets(&mut state.borrow_mut(), &app);
        app.set_collapsed_navigator(true);
        let first = state.borrow().develop.presets[0].clone();
        assert_eq!(
            state.borrow().develop.presets.len(),
            3,
            "the three shipped presets"
        );

        // The pane is 240px wide. With the list's frame gone and its
        // rows shorter, the bin is around x=188; the star is to its
        // left. Stay in the bin's column so the sweep does not add a
        // favorite and move the rows before finding the first bin.
        let mut bin = None;
        'sweep: for y in (60..280).step_by(3) {
            for x in (183..195).step_by(3) {
                crate::testing::click(&app, x as f32, y as f32);
                // A section header in the way folds its section:
                // unfolded again, so the layout holds still.
                app.set_collapsed_presets(false);
                app.set_collapsed_navigator(true);
                if app.get_preset_confirm_remove() >= 0 {
                    bin = Some((x as f32, y as f32));
                    break 'sweep;
                }
            }
        }
        let (bx, by) = bin.expect("a bin to click within the presets list");
        assert_eq!(app.get_preset_confirm_remove(), 0, "the first row's");
        assert!(first.path.exists(), "a bin alone removes nothing");

        // Escape is its Cancel.
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        assert!(first.path.exists());

        // Row to row down the same column, from the first bin, to
        // where the list ends: measured, not guessed (three shipped
        // presets, so a third row's bin is the list's last), so the
        // sweep below can start just past it and never land on
        // another row by the time it reaches the Remove. The list
        // itself does not change size when a row arms or disarms, so
        // this part needs no settling.
        crate::testing::click(&app, bx, by);
        assert_eq!(app.get_preset_confirm_remove(), 0);
        let (mut row_step, mut third_row_y) = (None, None);
        for y in (by as i32 + 1)..(by as i32 + 120) {
            crate::testing::click(&app, bx, y as f32);
            match app.get_preset_confirm_remove() {
                1 if row_step.is_none() => row_step = Some(y - by as i32),
                2 => {
                    third_row_y = Some(y);
                    break;
                }
                _ => {}
            }
        }
        let row_step = row_step.expect("a second row's bin below the first") as f32;
        let third_row_y = third_row_y.expect("a third row's bin below the second") as f32;
        // One more row's worth past the third clears the box.
        let list_bottom = third_row_y + row_step;

        // Re-armed on the first row for the removal itself: past
        // 120ms, the section has grown to fit the question and the
        // Cancel/Remove row, so a sweep bounded to a few rows below
        // the list, in Remove's column, lands on the question or the
        // button — never Cancel or the pane's
        // footer, whatever the settled height turns out to be.
        crate::testing::click(&app, bx, by);
        assert_eq!(app.get_preset_confirm_remove(), 0);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(150));
        let mut removed = false;
        'remove: for y in (list_bottom as i32..list_bottom as i32 + 150).step_by(3) {
            for x in (135..180).step_by(6) {
                crate::testing::click(&app, x as f32, y as f32);
                if !first.path.exists() {
                    removed = true;
                    break 'remove;
                }
                if app.get_preset_confirm_remove() < 0 {
                    // Cancel, or a row: arm it again and go on.
                    crate::testing::click(&app, bx, by);
                }
            }
        }
        assert!(removed, "the Remove removes");
        assert_eq!(state.borrow().develop.presets.len(), 2);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        crate::testing::remove_dir_retry(&d);
    }

    /// A preset of parts alone, imported, says what it carries.
    #[test]
    fn an_imported_preset_of_parts_says_its_parts() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let d = crate::testing::scratch_dir("preset-import-parts");
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join("haze.gcp");
        std::fs::write(
            &file,
            r#"{"name": "Haze", "sections": [], "parts": ["detail.dehaze"],
                "edit": {"detail": {"dehaze": 0.3}}}"#,
        )
        .unwrap();
        state.borrow_mut().develop.preset_store = Some(preset::Store::at(d.join("store")));
        import_presets(&mut state.borrow_mut(), &app, std::slice::from_ref(&file));
        assert_eq!(app.get_status(), "imported Haze (1 part)");
        assert!(
            !app.get_preset_report_open(),
            "a line that fits needs no sheet"
        );
        crate::testing::remove_dir_retry(&d);
    }

    /// A Lightroom preset as Lightroom Classic writes one: a name
    /// inside, settings this engine has, and some it has not (the
    /// color noise reduction, the calibration, a mask).
    const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:Version="15.0"
   crs:ProcessVersion="11.0"
   crs:Exposure2012="+0.50"
   crs:Contrast2012="+20"
   crs:Clarity2012="-35"
   crs:ColorNoiseReduction="25"
   crs:RedHue="+12"
   crs:HasSettings="True">
   <crs:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Soft Matte</rdf:li>
    </rdf:Alt>
   </crs:Name>
   <crs:MaskGroupBasedCorrections>
    <rdf:Seq>
     <rdf:li rdf:parseType="Resource"/>
    </rdf:Seq>
   </crs:MaskGroupBasedCorrections>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    /// Several files chosen at once: the Lightroom preset and a
    /// greycard one come into the store and the list, and the report
    /// is a sheet, since two presets are more than the status line
    /// says; what the Lightroom one passed over is named. Chosen
    /// again, with a file that is no preset and one that is gone,
    /// each is refused in its own words and the store is as it was.
    #[test]
    fn presets_imported_in_the_window_come_into_the_list_and_report_once() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let d = crate::testing::scratch_dir("preset-import-window");
        std::fs::create_dir_all(&d).unwrap();
        let xmp = d.join("soft-matte.xmp");
        std::fs::write(&xmp, LIGHTROOM).unwrap();
        let gcp = d.join("grain.gcp");
        std::fs::write(
            &gcp,
            r#"{"name": "Fine Grain", "sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        let store = d.join("store");
        state.borrow_mut().develop.preset_store = Some(preset::Store::at(store.clone()));
        refresh_presets(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_preset_names().row_count(), 0);

        import_presets(&mut state.borrow_mut(), &app, &[xmp.clone(), gcp.clone()]);
        let names: Vec<String> = app
            .get_preset_names()
            .iter()
            .map(|n| n.to_string())
            .collect();
        assert_eq!(names, ["Fine Grain", "Soft Matte"]);
        assert!(store.join("soft-matte.gcp").is_file());
        assert!(store.join("fine-grain.gcp").is_file());
        assert!(app.get_preset_report_open());
        assert_eq!(app.get_preset_report_title(), "Imported 2 presets");
        assert_eq!(app.get_status(), "imported 2 presets");
        assert_eq!(app.get_preset_report_refusals().row_count(), 0);
        let lines: Vec<String> = app
            .get_preset_report_lines()
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert_eq!(
            lines,
            [
                "Soft Matte (2 sections); no place here for ColorNoiseReduction, RedHue, masks",
                "Fine Grain (1 section)",
            ]
        );
        assert!(app.get_preset_report_note().contains("Lightroom"));
        // Shown once: closed, it is not put up again by anything.
        app.set_preset_report_open(false);
        refresh_presets(&mut state.borrow_mut(), &app);
        assert!(!app.get_preset_report_open());

        // Again, with a file that is no preset and one that is gone.
        let before = std::fs::read_to_string(store.join("soft-matte.gcp")).unwrap();
        let junk = d.join("notes.xmp");
        std::fs::write(&junk, "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").unwrap();
        let gone = d.join("gone.gcp");
        import_presets(&mut state.borrow_mut(), &app, &[xmp, junk, gone]);
        assert!(app.get_preset_report_open());
        assert_eq!(app.get_preset_report_title(), "No presets imported");
        let refusals: Vec<String> = app
            .get_preset_report_refusals()
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert_eq!(refusals.len(), 3, "{refusals:?}");
        assert_eq!(
            refusals[0],
            "soft-matte.xmp was not imported: there is a preset called \"Soft Matte\" already."
        );
        assert_eq!(
            refusals[1],
            "notes.xmp is not a preset: no Camera Raw settings in the file."
        );
        assert!(
            refusals[2].starts_with("gone.gcp could not be read: "),
            "{}",
            refusals[2]
        );
        assert_eq!(app.get_preset_report_lines().row_count(), 0);
        assert_eq!(app.get_preset_report_note(), "");
        assert_eq!(app.get_preset_names().row_count(), 2);
        assert_eq!(
            std::fs::read_to_string(store.join("soft-matte.gcp")).unwrap(),
            before
        );
        crate::testing::remove_dir_retry(&d);
    }

    /// The report sheet asks nothing: its OK closes it, and so does a
    /// click outside its card, where one inside it does nothing, and
    /// Escape. Each found where the layout put it, not guessed.
    #[test]
    fn the_import_report_closes_on_ok_outside_or_escape() {
        use i_slint_backend_testing::ElementHandle;
        let app = crate::testing::window(1);
        let open = || {
            show_import_report(
                &app,
                import_report(&[Brought::Refused(
                    "x.gcp is not a preset: it carries nothing.".into(),
                )]),
            );
            assert!(app.get_preset_report_open());
            // Built on the next event: a pointer move at the corner.
            app.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(0.0, 0.0),
                });
        };
        let bounds = |id: &str| {
            let e = ElementHandle::find_by_element_id(&app, id)
                .next()
                .unwrap_or_else(|| panic!("{id} on screen"));
            (e.absolute_position(), e.size())
        };

        open();
        let (at, size) = bounds("PresetReportSheet::ok");
        crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert!(!app.get_preset_report_open(), "OK closes it");

        open();
        let (card, card_size) = bounds("PresetReportSheet::card");
        assert!(card.y > 20.0, "room above the card for a click");
        crate::testing::click(&app, card.x + 12.0, card.y + 12.0);
        assert!(
            app.get_preset_report_open(),
            "a click on the card is not a close"
        );
        crate::testing::click(&app, card.x + card_size.width / 2.0, card.y - 10.0);
        assert!(!app.get_preset_report_open(), "a click outside closes it");

        open();
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_preset_report_open(), "Escape closes it");
    }

    /// One preset in goes on the status line while the line has room;
    /// a longer one, a refusal, or a mix go on the sheet.
    #[test]
    fn an_import_report_takes_the_status_line_only_when_it_fits() {
        let one = |passed: &[&str]| Brought::In {
            name: "Soft Matte".into(),
            carried: "3 sections".into(),
            passed: passed.iter().map(|p| p.to_string()).collect(),
        };
        assert_eq!(
            import_report(&[one(&["masks"])]),
            Report::Status("imported Soft Matte (3 sections); no place here for masks".into())
        );
        let long = &[
            "ColorNoiseReduction",
            "RedHue",
            "GreenSaturation",
            "CameraProfile Camera Portrait",
            "masks",
        ];
        match import_report(&[one(long)]) {
            Report::Sheet {
                title,
                refusals,
                lines,
                note,
            } => {
                assert_eq!(title, "Imported Soft Matte");
                assert!(refusals.is_empty());
                assert_eq!(lines.len(), 1);
                assert!(lines[0].ends_with("Camera Portrait, masks"));
                assert!(!note.is_empty());
            }
            other => panic!("{other:?}"),
        }
        let refused = Brought::Refused("x.gcp is not a preset: it carries nothing.".into());
        match import_report(std::slice::from_ref(&refused)) {
            Report::Sheet {
                title, refusals, ..
            } => {
                assert_eq!(title, "Not imported");
                assert_eq!(refusals, ["x.gcp is not a preset: it carries nothing."]);
            }
            other => panic!("{other:?}"),
        }
        match import_report(&[one(&[]), refused]) {
            Report::Sheet { title, note, .. } => {
                assert_eq!(title, "Imported 1 of 2 presets");
                assert_eq!(note, "", "nothing was passed over");
            }
            other => panic!("{other:?}"),
        }
    }

    /// A bin asks and removes nothing; the Remove that names the
    /// preset removes it. A shipped one removed is then missing in the
    /// settings, and their Restore puts it back.
    #[test]
    fn a_preset_is_removed_only_when_confirmed_and_restored_on_asking() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let d = std::env::temp_dir().join(format!("greycard-ui-presets-{}", std::process::id()));
        crate::testing::remove_dir_retry(&d);
        let store = preset::Store::at(d.clone());
        store.seed();
        state.borrow_mut().develop.preset_store = Some(store);
        refresh_presets(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_presets_missing(), 0);
        let count = || state.borrow().develop.presets.len();
        assert_eq!(count(), 3);
        let first = state.borrow().develop.presets[0].clone();
        assert_eq!(first.preset.name, "Muted Slide");

        // The bin, which only arms the row: nothing is removed, and a
        // list shown again (a preset saved, one imported) drops the
        // question, whose row may be another preset's now.
        assert_eq!(app.get_preset_confirm_remove(), -1);
        app.set_preset_confirm_remove(0);
        assert!(first.path.exists(), "a bin alone removes nothing");
        refresh_presets(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        // So does a preset laid over the edit instead.
        app.set_preset_confirm_remove(0);
        app.invoke_preset_applied(1);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        assert!(first.path.exists());

        // Asked again and confirmed, as the Remove does: gone, the
        // question with it, and the status says where it can be had
        // back.
        app.set_preset_confirm_remove(0);
        app.invoke_preset_deleted(0);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        assert!(!first.path.exists());
        assert_eq!(count(), 2);
        assert_eq!(
            app.get_status(),
            "Muted Slide removed; Settings can restore it"
        );
        assert_eq!(app.get_presets_missing(), 1);
        assert_eq!(
            app.get_presets_missing_words(),
            "Muted Slide is not in the list."
        );

        // Restore: back, as shipped.
        app.invoke_presets_restore();
        assert!(first.path.exists());
        assert_eq!(count(), 3);
        assert_eq!(app.get_presets_missing(), 0);
        assert_eq!(app.get_status(), "restored Muted Slide");

        // A preset of the user's own promises nothing when it goes;
        // nor does a shipped name while another preset still has it.
        let mine = Preset::from_edit("Mine", &Edit::default(), &[Section::Light]);
        let dup = Preset::from_edit("warm negative", &Edit::default(), &[Section::Light]);
        {
            let st = state.borrow();
            let store = st.develop.preset_store.as_ref().unwrap();
            store.save(&mine).unwrap();
            dup.save_to(&d.join("warm-negative-copy.gcp")).unwrap();
        }
        refresh_presets(&mut state.borrow_mut(), &app);
        let at = |name: &str| {
            state
                .borrow()
                .develop
                .presets
                .iter()
                .position(|e| e.preset.name == name)
                .unwrap() as i32
        };
        app.invoke_preset_deleted(at("Mine"));
        assert_eq!(app.get_status(), "Mine removed");
        app.invoke_preset_deleted(at("warm negative"));
        assert_eq!(app.get_status(), "warm negative removed");
        crate::testing::remove_dir_retry(&d);
    }

    #[test]
    fn a_batch_run_is_never_offered_the_lens_profiles() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        state.borrow_mut().window.batch = true;
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open());
    }

    /// A picture opens with its lens matched and shown and the
    /// profile's three corrections off; one switched on goes into the
    /// edit and the sidecar written for it, the others left off.
    #[test]
    fn a_picture_opens_with_its_lens_shown_and_its_corrections_off() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/scratch/picture-lens-off-{}",
            std::process::id()
        ));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let jpeg = dir.join("IMG_0001.JPG");
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, vec![jpeg.clone()]);
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.sidecars[0] = greycard_library::held::from_disk(
                &jpeg,
                Some(greycard_edit::Placement::Beside),
                Default::default(),
            )
            .sidecar;
        }
        app.invoke_select(0);
        let generation = state.borrow().develop.generation;
        let lens = "Canon RF 50mm F1.8 STM";
        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::Opened {
                generation,
                as_shot: (6504.0, 0.0),
                frame: None,
                profile: None,
                lens: LensReport::Found {
                    lens: lens.into(),
                    camera: true,
                    smaller_sensor: false,
                },
                lens_options: Vec::new(),
                kind: crate::worker::SourceKind::Picture {
                    space: "sRGB".into(),
                    bits: 8,
                },
                shot: Default::default(),
                taken: None,
                blend: None,
            },
        );
        assert_eq!(app.get_lens_profile(), lens);
        assert!(app.get_lens_available());
        assert!(app.get_lens_enabled() && app.get_lens_profile_on());
        assert!(!app.get_lens_distortion());
        assert!(!app.get_lens_ca());
        assert!(!app.get_lens_vignetting());

        app.set_lens_distortion(true);
        app.invoke_develop_changed();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ));
        let current = state.borrow().sidecars[0].current.lens.clone();
        assert!(current.distortion);
        assert!(!current.chromatic_aberration && !current.vignetting);
        let saved = greycard_edit::Sidecar::load(&jpeg)
            .unwrap()
            .expect("a sidecar written");
        assert_eq!(saved.current.lens, current);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The LENS picker: "From the file" first and chosen by default,
    /// with the report under it as before; a lens picked is chosen and
    /// named; one picked that this body or machine has not got is
    /// still listed, greyed; and the panel reads the choice back.
    #[test]
    fn the_lens_picker_lists_the_options_and_keeps_the_choice() {
        use greycard_edit::LensChoice;
        use greycard_engine::lens::LensOption;
        let app = crate::testing::window(1);
        let options = vec![
            LensOption {
                choice: LensChoice::Database("Canon RF 50mm F1.8 STM".into()),
                label: "Canon RF 50mm F1.8 STM".into(),
                maker: "Canon".into(),
                off_mount: false,
            },
            LensOption {
                choice: LensChoice::Lcp("hand.lcp".into()),
                label: "Hand 35mm".into(),
                maker: "Hand".into(),
                off_mount: false,
            },
        ];
        let found = LensReport::Found {
            lens: "Canon RF 50mm F1.8 STM".into(),
            camera: true,
            smaller_sensor: false,
        };
        let strings = |m: ModelRc<slint::SharedString>| -> Vec<String> {
            m.iter().map(|s| s.to_string()).collect()
        };
        show_lens(&found, &options, &LensChoice::FromFile, &app);
        assert_eq!(
            strings(app.get_lens_choice_labels()),
            [FROM_THE_FILE, "Canon RF 50mm F1.8 STM", "Hand 35mm"]
        );
        assert_eq!(app.get_lens_choice_index(), 0);
        assert_eq!(app.get_lens_profile(), "Canon RF 50mm F1.8 STM");
        assert!(app.get_lens_available());

        let picked = LensChoice::Lcp("hand.lcp".into());
        show_lens(&LensReport::NoLensName, &options, &picked, &app);
        assert_eq!(app.get_lens_choice_index(), 2);
        assert_eq!(app.get_lens_profile(), "Hand 35mm");
        assert!(app.get_lens_available());
        let edit = read_edit(&app, &greycard_edit::Edit::default(), None);
        assert_eq!(edit.lens.chosen, picked);

        let away = LensChoice::Database("Nikon Z 50mm f/1.8 S".into());
        show_lens(&LensReport::NoLensName, &options, &away, &app);
        let labels = strings(app.get_lens_choice_labels());
        assert_eq!(labels.len(), 4);
        assert_eq!(labels[3], "Nikon Z 50mm f/1.8 S");
        assert_eq!(app.get_lens_choice_index(), 3);
        assert!(!app.get_lens_available());
    }

    /// A picker's options: three makers' lenses and a profile.
    fn maker_options() -> Vec<greycard_engine::lens::LensOption> {
        use greycard_edit::LensChoice;
        use greycard_engine::lens::LensOption;
        let row = |choice, label: &str, maker: &str| LensOption {
            choice,
            label: label.into(),
            maker: maker.into(),
            off_mount: false,
        };
        let db = |name: &str| LensChoice::Database(name.into());
        vec![
            row(
                db("Canon RF 24-70mm F2.8L IS USM"),
                "Canon RF 24-70mm F2.8L IS USM",
                "Canon",
            ),
            row(
                db("Canon RF 50mm F1.8 STM"),
                "Canon RF 50mm F1.8 STM",
                "Canon",
            ),
            row(
                db("Sigma 24-70mm F2.8 DG DN Art"),
                "Sigma 24-70mm F2.8 DG DN Art",
                "Sigma",
            ),
            row(
                db("Samyang AF 35mm F1.4"),
                "Samyang AF 35mm F1.4",
                "Samyang",
            ),
            row(
                LensChoice::Lcp("art.lcp".into()),
                "SIGMA 35mm F1.4 DG HSM Art",
                "Sigma",
            ),
        ]
    }

    /// The Maker row: All, then each maker of a row once,
    /// alphabetically; the Lens row narrowed by maker, by words, and
    /// by both, From the file always first, every row under its own
    /// key whatever is left out, and the chosen lens shown when the
    /// narrowing would leave it out.
    #[test]
    fn the_lens_rows_narrow_by_maker_and_words_and_keep_the_choice() {
        use greycard_edit::LensChoice;
        let options = maker_options();
        let from = LensChoice::FromFile;
        assert_eq!(
            lens_makers(&options, &from, ""),
            [ALL_MAKERS, "Canon", "Samyang", "Sigma"]
        );
        assert_eq!(lens_makers(&[], &from, ""), [ALL_MAKERS]);
        let labels = |r: &LensRows| r.labels.clone();
        let all = lens_rows(&options, &from, None, "");
        assert_eq!(all.labels.len(), 6);
        assert_eq!(all.labels[0], FROM_THE_FILE);
        assert_eq!(all.index, 0);

        let sigma = lens_rows(&options, &from, Some("Sigma"), "");
        assert_eq!(
            labels(&sigma),
            [
                FROM_THE_FILE,
                "Sigma 24-70mm F2.8 DG DN Art",
                "SIGMA 35mm F1.4 DG HSM Art"
            ]
        );
        let zooms = lens_rows(&options, &from, None, "24-70");
        assert_eq!(
            labels(&zooms),
            [
                FROM_THE_FILE,
                "Canon RF 24-70mm F2.8L IS USM",
                "Sigma 24-70mm F2.8 DG DN Art"
            ]
        );
        // Every word, in any order, case aside.
        let art = lens_rows(&options, &from, None, "art  sigma");
        assert_eq!(
            labels(&art),
            [
                FROM_THE_FILE,
                "Sigma 24-70mm F2.8 DG DN Art",
                "SIGMA 35mm F1.4 DG HSM Art"
            ]
        );
        let both = lens_rows(&options, &from, Some("Sigma"), "35");
        assert_eq!(labels(&both), [FROM_THE_FILE, "SIGMA 35mm F1.4 DG HSM Art"]);
        let none = lens_rows(&options, &from, Some("Canon"), "sigma");
        assert_eq!(labels(&none), [FROM_THE_FILE]);

        // The keys are the options' own, row for row, however
        // narrowed: a pick reads the same lens either way.
        for rows in [&all, &sigma, &zooms, &art, &both, &none] {
            assert_eq!(rows.keys.len(), rows.labels.len());
            assert_eq!(rows.keys[0], "");
            for (key, label) in rows.keys.iter().zip(&rows.labels).skip(1) {
                let o = options.iter().find(|o| o.label == *label).unwrap();
                assert_eq!(*key, o.choice.key());
            }
        }

        // The chosen lens left out by the maker or the words: shown
        // all the same, first after From the file, and chosen.
        let fifty = LensChoice::Database("Canon RF 50mm F1.8 STM".into());
        let kept = lens_rows(&options, &fifty, Some("Sigma"), "");
        assert_eq!(
            labels(&kept),
            [
                FROM_THE_FILE,
                "Canon RF 50mm F1.8 STM",
                "Sigma 24-70mm F2.8 DG DN Art",
                "SIGMA 35mm F1.4 DG HSM Art"
            ]
        );
        assert_eq!(kept.index, 1);
        assert_eq!(kept.keys[1], fifty.key());
        let kept = lens_rows(&options, &fifty, None, "24-70");
        assert_eq!(kept.labels[1], "Canon RF 50mm F1.8 STM");
        assert_eq!(kept.index, 1);
        // Not left out: in its own place.
        let shown = lens_rows(&options, &fifty, Some("Canon"), "");
        assert_eq!(shown.index, 2);
        assert_eq!(shown.labels.len(), 3);
        // From the file chosen under words that match nothing: the
        // one row, chosen.
        let bare = lens_rows(&options, &from, None, "nothing");
        assert_eq!(
            (labels(&bare), bare.index),
            (vec![FROM_THE_FILE.to_string()], 0)
        );

        // The words find a lens by its maker too, where its name
        // does not say it.
        let laowa = vec![greycard_engine::lens::LensOption {
            choice: LensChoice::Database("Laowa 15mm f/2 Zero-D".into()),
            label: "Laowa 15mm f/2 Zero-D".into(),
            maker: "Venus".into(),
            off_mount: false,
        }];
        let venus = lens_rows(&laowa, &from, None, "venus 15mm");
        assert_eq!(labels(&venus), [FROM_THE_FILE, "Laowa 15mm f/2 Zero-D"]);
    }

    /// An open with no develop to follow it (an offline frame, or one
    /// in culling) sets the filter and the Maker row as a develop's
    /// does: cleared and following the edit's lens for another frame,
    /// kept for the same one.
    #[test]
    fn an_open_without_a_develop_follows_the_frame_change_rule() {
        use greycard_edit::LensChoice;
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
        let mut st = state.borrow_mut();
        st.develop.lens = LensReport::NoLensName;
        st.develop.lens_options = maker_options();
        st.develop.edit.lens.chosen = LensChoice::Lcp("art.lcp".into());
        let maker = |app: &App| {
            app.get_lens_makers()
                .row_data(app.get_lens_maker_index() as usize)
                .unwrap()
                .to_string()
        };
        st.current = Some(0);
        app.set_lens_filter("24-70".into());
        lens_frame_shown(&mut st, &app);
        assert_eq!(app.get_lens_filter(), "");
        assert_eq!(maker(&app), "Sigma");
        // The same frame again: the words stay.
        app.set_lens_filter("art".into());
        lens_frame_shown(&mut st, &app);
        assert_eq!(app.get_lens_filter(), "art");
        // Another: cleared.
        st.current = Some(1);
        lens_frame_shown(&mut st, &app);
        assert_eq!(app.get_lens_filter(), "");
        // A develop's open of that frame after it keeps them.
        app.set_lens_filter("dg".into());
        assert!(!another_lens_frame(&mut st));
        assert_eq!(app.get_lens_filter(), "dg");
    }

    /// The Maker row on the panel: a frame opened under a picked lens
    /// shows its maker; a lens picked under All leaves All; a maker the
    /// chosen lens is not of turns to the lens's; the filter is cleared
    /// by another frame and kept by the same one; and the Maker row and
    /// the filter, changed, narrow the Lens row with the choice kept.
    #[test]
    fn the_maker_row_follows_the_opened_frame_and_keeps_its_choice() {
        use greycard_edit::LensChoice;
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let options = maker_options();
        state.borrow_mut().develop.lens_options = options.clone();
        let strings = |m: ModelRc<slint::SharedString>| -> Vec<String> {
            m.iter().map(|s| s.to_string()).collect()
        };
        let maker =
            |app: &App| strings(app.get_lens_makers())[app.get_lens_maker_index() as usize].clone();
        let report = LensReport::NoLensName;

        // Opened under the Samyang: Samyang, its row chosen.
        let samyang = LensChoice::Database("Samyang AF 35mm F1.4".into());
        show_lens_opened(&report, &options, &samyang, &app, true);
        assert_eq!(
            strings(app.get_lens_makers()),
            [ALL_MAKERS, "Canon", "Samyang", "Sigma"]
        );
        assert_eq!(maker(&app), "Samyang");
        assert_eq!(
            strings(app.get_lens_choice_labels()),
            [FROM_THE_FILE, "Samyang AF 35mm F1.4"]
        );
        assert_eq!(app.get_lens_choice_index(), 1);
        assert_eq!(app.get_lens_profile(), "Samyang AF 35mm F1.4");

        // All chosen, then a lens picked: All stays.
        app.set_lens_maker_index(0);
        app.invoke_lens_rows_changed();
        assert_eq!(app.get_lens_choice_labels().row_count(), 6);
        let art = LensChoice::Lcp("art.lcp".into());
        show_lens(&report, &options, &art, &app);
        assert_eq!(maker(&app), ALL_MAKERS);
        assert_eq!(app.get_lens_choice_index(), 5);
        assert_eq!(app.get_lens_profile(), "SIGMA 35mm F1.4 DG HSM Art");

        // Canon chosen while the edit's lens is a Sigma (an undo, a
        // paste): the row turns to Sigma, so the choice reads back.
        app.set_lens_maker_index(1);
        show_lens(&report, &options, &art, &app);
        assert_eq!(maker(&app), "Sigma");

        // From the file: the maker stays as it was.
        show_lens(&report, &options, &LensChoice::FromFile, &app);
        assert_eq!(maker(&app), "Sigma");

        // Words typed: the Lens row narrows, the choice kept on it.
        app.set_lens_choice(art.key().into());
        app.set_lens_filter("24-70".into());
        app.invoke_lens_rows_changed();
        assert_eq!(
            strings(app.get_lens_choice_labels()),
            [
                FROM_THE_FILE,
                "SIGMA 35mm F1.4 DG HSM Art",
                "Sigma 24-70mm F2.8 DG DN Art"
            ]
        );
        assert_eq!(app.get_lens_choice_index(), 1);
        assert_eq!(
            strings(app.get_lens_choice_keys()),
            ["", "lcp:art.lcp", "database:Sigma 24-70mm F2.8 DG DN Art"]
        );
        // The panel reads the choice as before.
        let edit = read_edit(&app, &greycard_edit::Edit::default(), None);
        assert_eq!(edit.lens.chosen, art);

        // The same frame opened again keeps the words; another clears
        // them, and its edit's lens's maker is shown.
        show_lens_opened(&report, &options, &art, &app, false);
        assert_eq!(app.get_lens_filter(), "24-70");
        let fifty = LensChoice::Database("Canon RF 50mm F1.8 STM".into());
        show_lens_opened(&report, &options, &fifty, &app, true);
        assert_eq!(app.get_lens_filter(), "");
        assert_eq!(maker(&app), "Canon");
        assert_eq!(
            strings(app.get_lens_choice_labels()),
            [
                FROM_THE_FILE,
                "Canon RF 24-70mm F2.8L IS USM",
                "Canon RF 50mm F1.8 STM"
            ]
        );
        assert_eq!(app.get_lens_choice_index(), 2);

        // Another body's list without that maker: All.
        let fewer: Vec<_> = options
            .iter()
            .filter(|o| o.maker != "Canon")
            .cloned()
            .collect();
        show_lens_opened(&report, &fewer, &LensChoice::FromFile, &app, true);
        assert_eq!(maker(&app), ALL_MAKERS);
        assert_eq!(
            strings(app.get_lens_makers()),
            [ALL_MAKERS, "Samyang", "Sigma"]
        );
    }

    /// A lens on another mount, for one on an adapter: not listed, nor
    /// its maker, with no words typed; with words that match it,
    /// listed after the body's own lenses and before the profiles, its
    /// maker on the Maker row; picked, the edit holds its name alone.
    #[test]
    fn a_lens_off_the_mount_is_listed_when_words_find_it() {
        use greycard_edit::LensChoice;
        use greycard_engine::lens::LensOption;
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let name = "Sigma 35mm f/1.4 DG HSM | A";
        let label = format!("{name} (Canon EF, Nikon F AF)");
        let options = vec![
            LensOption {
                choice: LensChoice::Database("Sony FE 35mm F1.8".into()),
                label: "Sony FE 35mm F1.8".into(),
                maker: "Sony".into(),
                off_mount: false,
            },
            LensOption {
                choice: LensChoice::Database(name.into()),
                label: label.clone(),
                maker: "Sigma".into(),
                off_mount: true,
            },
            LensOption {
                choice: LensChoice::Database("Minolta AF 50mm F1.7".into()),
                label: "Minolta AF 50mm F1.7 (Sony Alpha)".into(),
                maker: "Minolta".into(),
                off_mount: true,
            },
            LensOption {
                choice: LensChoice::Lcp("hand.lcp".into()),
                label: "Hand 35mm".into(),
                maker: "Hand".into(),
                off_mount: false,
            },
        ];
        state.borrow_mut().develop.lens_options = options.clone();
        let strings = |m: ModelRc<slint::SharedString>| -> Vec<String> {
            m.iter().map(|s| s.to_string()).collect()
        };
        let report = LensReport::NoLensName;
        let from = LensChoice::FromFile;
        show_lens_opened(&report, &options, &from, &app, true);
        assert_eq!(
            strings(app.get_lens_choice_labels()),
            [FROM_THE_FILE, "Sony FE 35mm F1.8", "Hand 35mm"]
        );
        assert_eq!(strings(app.get_lens_makers()), [ALL_MAKERS, "Hand", "Sony"]);

        app.set_lens_filter("35".into());
        app.invoke_lens_rows_changed();
        let labels = strings(app.get_lens_choice_labels());
        assert_eq!(
            labels,
            [FROM_THE_FILE, "Sony FE 35mm F1.8", &label, "Hand 35mm"]
        );
        assert_eq!(
            strings(app.get_lens_makers()),
            [ALL_MAKERS, "Hand", "Sigma", "Sony"]
        );
        // Words that do not match it leave it out; and the words are
        // matched on an off-mount lens's name and maker, not on the
        // mounts its label adds.
        assert_eq!(
            lens_rows(&options, &from, None, "sony").labels,
            [FROM_THE_FILE, "Sony FE 35mm F1.8"]
        );
        assert_eq!(
            lens_rows(&options, &from, None, "minolta").labels,
            [FROM_THE_FILE, "Minolta AF 50mm F1.7 (Sony Alpha)"]
        );
        // Under its maker, with the words: listed.
        let sigma = lens_rows(&options, &from, Some("Sigma"), "35");
        assert_eq!(sigma.labels, [FROM_THE_FILE, label.as_str()]);

        // Picked: the key is the name's, and the edit holds the name.
        let keys = strings(app.get_lens_choice_keys());
        app.set_lens_choice(keys[2].clone().into());
        let edit = read_edit(&app, &greycard_edit::Edit::default(), None);
        assert_eq!(edit.lens.chosen, LensChoice::Database(name.into()));
        // Shown chosen with the words cleared: still listed, labeled
        // with its mounts, and available.
        app.set_lens_filter("".into());
        show_lens(&report, &options, &edit.lens.chosen, &app);
        assert_eq!(
            strings(app.get_lens_choice_labels()),
            [FROM_THE_FILE, &label, "Sony FE 35mm F1.8", "Hand 35mm"]
        );
        assert_eq!(app.get_lens_choice_index(), 1);
        assert_eq!(app.get_lens_profile(), label.as_str());
        assert!(app.get_lens_available());

        // Sigma picked on the Maker row while the words find the
        // Sigma, then words that drop it with another lens chosen: the
        // Maker row loses Sigma and falls back to All. The body's own
        // makers stay whatever the words.
        let maker =
            |app: &App| strings(app.get_lens_makers())[app.get_lens_maker_index() as usize].clone();
        show_lens(&report, &options, &from, &app);
        app.set_lens_filter("35".into());
        app.invoke_lens_rows_changed();
        let at = strings(app.get_lens_makers())
            .iter()
            .position(|m| m == "Sigma")
            .unwrap();
        app.set_lens_maker_index(at as i32);
        app.invoke_lens_rows_changed();
        assert_eq!(maker(&app), "Sigma");
        assert_eq!(
            strings(app.get_lens_choice_labels()),
            [FROM_THE_FILE, label.as_str()]
        );
        app.set_lens_filter("sony".into());
        app.invoke_lens_rows_changed();
        assert_eq!(app.get_lens_maker_index(), 0);
        assert_eq!(strings(app.get_lens_makers()), [ALL_MAKERS, "Hand", "Sony"]);
        assert_eq!(
            strings(app.get_lens_choice_labels()),
            [FROM_THE_FILE, "Sony FE 35mm F1.8"]
        );
    }

    /// The CAMERA PROFILE rows for `profiles` on an EOS 850D, also
    /// sold as the Rebel T8i, with `chosen` named by the edit.
    fn profile_rows(
        profiles: Vec<greycard_edit::camera::Entry>,
        chosen: &str,
    ) -> (Vec<String>, Vec<String>, App) {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        {
            let mut st = state.borrow_mut();
            st.develop.camera = ("Canon".into(), "EOS 850D".into());
            st.develop.other_names = vec!["EOS Rebel T8i".into()];
            st.develop.profiles = profiles;
        }
        let choice = greycard_edit::camera::ProfileChoice::from_name(chosen);
        show_profiles(&state.borrow(), &choice, &app);
        let rows: Vec<ProfileRow> = app.get_camera_profile_rows().iter().collect();
        let names = rows.iter().map(|r| r.name.to_string()).collect();
        let labels = rows.iter().map(|r| r.label.to_string()).collect();
        (names, labels, app)
    }

    /// The profile list's rows as a line each: a heading as `[label]`
    /// with `+` folded and `*` own, a profile as its name, `  ` before
    /// one set in.
    fn profile_lines(app: &App) -> Vec<String> {
        app.get_camera_profile_rows()
            .iter()
            .map(|r| match r.heading {
                true => format!(
                    "[{}]{}{}",
                    r.label,
                    if r.folded { "+" } else { "" },
                    if r.own { "*" } else { "" }
                ),
                false => format!("{}{}", if r.indented { "  " } else { "" }, r.name),
            })
            .collect()
    }

    /// A profile in a folder of Adobe's shape.
    fn adobe(folder: &str, title: &str, camera: Option<&str>) -> greycard_edit::camera::Entry {
        greycard_edit::camera::Entry {
            name: format!("Camera/{folder}/{title}"),
            path: std::path::PathBuf::new(),
            title: Some(title.into()),
            unique_camera_model: camera.map(str::to_string),
            copyright: None,
            same_as: None,
        }
    }

    /// Three cameras' profiles in Adobe's shape, and one naming none.
    fn three_cameras() -> Vec<greycard_edit::camera::Entry> {
        let mut entries = vec![
            adobe("Nikon Z 8", "Adobe Standard", Some("NIKON Z 8")),
            adobe("Canon EOS R5", "Adobe Standard", Some("Canon EOS R5")),
            adobe("Canon EOS R5", "Camera Neutral", Some("Canon EOS R5")),
            adobe(
                "Canon EOS Rebel T8i",
                "Adobe Standard",
                Some("Canon EOS Rebel T8i"),
            ),
            adobe("Canon EOS 850D", "Camera Standard", Some("Canon EOS 850D")),
        ];
        entries.push(greycard_edit::camera::Entry {
            name: "Anything".into(),
            title: None,
            unique_camera_model: None,
            ..adobe("x", "Anything", None)
        });
        entries.sort_by_key(|e| e.label().to_lowercase());
        entries
    }

    /// The window with the Develop panel's CAMERA PROFILE list for an
    /// EOS 850D, also sold as the Rebel T8i, over [`three_cameras`].
    fn profile_window() -> (App, Rc<Shared>, Rc<Worker>) {
        let app = crate::testing::window(1);
        let (state, worker) = crate::testing::retouch_state(&app);
        // Tall enough that the section and its switch are in the
        // panel's view, which builds only what it shows.
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 1600.0));
        // Opened first: opening the section reads the (empty) profile
        // directory again.
        app.set_raw_input(true);
        app.set_collapsed_camera(false);
        app.set_panel_tab("Develop".into());
        crate::testing::settle(&app);
        {
            let mut st = state.borrow_mut();
            st.develop.camera = ("Canon".into(), "EOS 850D".into());
            st.develop.other_names = vec!["EOS Rebel T8i".into()];
            st.develop.profiles = three_cameras();
        }
        show_profiles(
            &state.borrow(),
            &state.borrow().develop.edit.camera.profile,
            &app,
        );
        crate::testing::settle(&app);
        (app, state, worker)
    }

    /// A click on the switch at the profile list's foot, the panel
    /// scrolled down to it a step at a time and drawn between steps:
    /// the panel builds only what is in its view.
    fn click_all_cameras(app: &App) {
        use i_slint_backend_testing::ElementHandle;
        app.set_panel_scroll(0.0);
        for _ in 0..40 {
            crate::testing::settle(app);
            let found = ElementHandle::find_by_element_id(app, "Toggle::root").find(|root| {
                root.query_descendants()
                    .match_id("Toggle::lbl")
                    .find_first()
                    .and_then(|l| l.accessible_label())
                    .is_some_and(|l| l == "Show all cameras")
            });
            if let Some(root) = found {
                let (at, size) = crate::testing::toggle_parts(&root).switch;
                if at.y > 300.0 {
                    crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
                    return;
                }
            }
            app.set_panel_scroll(app.get_panel_scroll() - 100.0);
        }
        panic!("no Show all cameras switch");
    }

    /// Off, the list is this camera's, under each of its names, and
    /// the profiles naming no camera; on, every camera's, grouped: this
    /// camera's groups first and open, the rest by name and folded, the
    /// ones naming no camera last under no heading. A group opens and
    /// folds; the switch is the window's, flipped at the list's foot.
    #[test]
    fn show_all_cameras_groups_every_profile_by_camera() {
        let (app, _state, _worker) = profile_window();
        assert!(!app.get_camera_profiles_all(), "off by default");
        assert_eq!(
            profile_lines(&app),
            [
                "embedded",
                "Camera/Canon EOS Rebel T8i/Adobe Standard",
                "Anything",
                "Camera/Canon EOS 850D/Camera Standard",
            ]
        );
        assert!(app.get_camera_profiles_any());
        assert!(!app.get_camera_profile_note().is_empty(), "three hidden");

        click_all_cameras(&app);
        assert!(app.get_camera_profiles_all());
        assert_eq!(
            profile_lines(&app),
            [
                "embedded",
                "[Canon EOS 850D]*",
                "  Camera/Canon EOS 850D/Camera Standard",
                "[Canon EOS Rebel T8i]*",
                "  Camera/Canon EOS Rebel T8i/Adobe Standard",
                "[Canon EOS R5]+",
                "[NIKON Z 8]+",
                "Anything",
            ]
        );
        assert!(app.get_camera_profile_note().is_empty(), "nothing hidden");
        // The labels under a heading are the profiles' own names.
        let labels: Vec<String> = app
            .get_camera_profile_rows()
            .iter()
            .map(|r| r.label.to_string())
            .collect();
        assert_eq!(labels[2], "Camera Standard");

        // A group opened, and folded again.
        app.invoke_camera_profile_group_toggled("canoneosr5".into());
        assert!(profile_lines(&app).contains(&"  Camera/Canon EOS R5/Camera Neutral".to_string()));
        app.invoke_camera_profile_group_toggled("canoneosr5".into());
        assert!(profile_lines(&app).contains(&"[Canon EOS R5]+".to_string()));

        click_all_cameras(&app);
        assert!(!app.get_camera_profiles_all());
        assert_eq!(profile_lines(&app).len(), 4);
    }

    /// Another camera's profile chosen with every camera shown is the
    /// edit's, applied as any profile is; the switch flipped either way
    /// changes nothing in the edit or its picture, and off, the list
    /// still shows the profile the edit names, chosen.
    #[test]
    fn another_cameras_profile_is_chosen_and_stays_shown() {
        let (app, state, _worker) = profile_window();
        let nikon = "Camera/Nikon Z 8/Adobe Standard";
        app.set_camera_profiles_all(true);
        app.invoke_camera_profiles_all_changed();
        app.invoke_camera_profile_group_toggled("nikonz8".into());
        assert!(profile_lines(&app).contains(&format!("  {nikon}")));
        app.set_camera_profile(nikon.into());
        app.invoke_camera_profile_picked();
        let edit = state.borrow().develop.edit.clone();
        assert_eq!(
            edit.camera.profile,
            greycard_edit::camera::ProfileChoice::Named(nikon.into())
        );
        assert!(edit.camera.applies_look_table(), "applied as any DCP is");
        let generation = state.borrow().develop.generation;

        // Off: the chosen profile is still a row, and chosen.
        app.set_camera_profiles_all(false);
        app.invoke_camera_profiles_all_changed();
        assert!(profile_lines(&app).contains(&nikon.to_string()));
        assert_eq!(app.get_camera_profile(), nikon);
        // On again: its group open whatever it was set to.
        app.invoke_camera_profile_group_toggled("nikonz8".into());
        app.set_camera_profiles_all(true);
        app.invoke_camera_profiles_all_changed();
        assert!(profile_lines(&app).contains(&"[NIKON Z 8]".to_string()));
        assert!(profile_lines(&app).contains(&format!("  {nikon}")));
        // Neither flip touched the edit or asked for a develop.
        let st = state.borrow();
        assert_eq!(st.develop.edit, edit);
        assert_eq!(st.develop.generation, generation);
        let read = read_edit(&app, &st.develop.edit, st.develop.target);
        assert_eq!(read.camera, edit.camera);
    }

    /// Every camera of a copy of Adobe's folder: hundreds of headings
    /// in the list, which keeps to twelve rows' height and scrolls,
    /// and is drawn again quickly.
    #[test]
    fn every_camera_of_a_large_folder_keeps_the_list_in_bounds() {
        let (app, state, _worker) = profile_window();
        let mut many: Vec<greycard_edit::camera::Entry> = (0..800)
            .flat_map(|c| {
                let camera = format!("Maker Camera {c:03}");
                [
                    "Adobe Standard",
                    "Camera Neutral",
                    "Camera Vivid",
                    "Camera Flat",
                ]
                .map(|t| adobe(&camera, t, Some(&camera)))
            })
            .collect();
        many.extend(three_cameras());
        state.borrow_mut().develop.profiles = many;
        app.set_camera_profiles_all(true);
        let t = std::time::Instant::now();
        app.invoke_camera_profiles_all_changed();
        crate::testing::settle(&app);
        let shown = t.elapsed();
        assert_eq!(
            app.get_camera_profile_rows().row_count(),
            1 + 2 + 2 + 802 + 1
        );
        let list = i_slint_backend_testing::ElementHandle::find_by_element_id(
            &app,
            "CameraSection::profile-box",
        )
        .next()
        .expect("the list is there");
        // Twelve rows of a control's height or so, not eight hundred.
        assert!(list.size().height < 13.0 * 40.0, "{:?}", list.size());
        // Generous: a debug build on a loaded machine.
        assert!(shown < std::time::Duration::from_secs(5), "{shown:?}");
        println!("800 cameras listed and drawn in {shown:?}");
    }

    /// An Adobe Standard made for `camera`, in a file named `name`.
    fn adobe_standard(
        name: &str,
        camera: &str,
        same_as: Option<&str>,
    ) -> greycard_edit::camera::Entry {
        greycard_edit::camera::Entry {
            name: name.into(),
            path: std::path::PathBuf::new(),
            title: Some("Adobe Standard".into()),
            unique_camera_model: Some(camera.into()),
            copyright: None,
            same_as: same_as.map(str::to_string),
        }
    }

    /// One profile saved under two file names is one row, the file the
    /// edit names standing for the pair; two that differ show their
    /// file names.
    #[test]
    fn a_profile_under_two_names_is_one_row() {
        let rebel = "Canon EOS Rebel T8i";
        let pair = || {
            vec![
                adobe_standard("Canon EOS 850D Adobe Standard", rebel, None),
                adobe_standard(rebel, rebel, Some("Canon EOS 850D Adobe Standard")),
            ]
        };
        let (names, labels, app) = profile_rows(pair(), "embedded");
        assert_eq!(names, ["embedded", "Canon EOS 850D Adobe Standard"]);
        assert_eq!(labels, ["Embedded", "Adobe Standard"]);
        assert!(app.get_camera_profile_note().is_empty(), "nothing hidden");
        // A sidecar naming the other file: that file is the row.
        let (names, labels, app) = profile_rows(pair(), rebel);
        assert_eq!(names, ["embedded", rebel]);
        assert_eq!(labels, ["Embedded", "Adobe Standard"]);
        assert!(app.get_camera_profile_warning().is_empty());
        // Two that differ: both, each with its file name.
        let twins = vec![
            adobe_standard("new", rebel, None),
            adobe_standard("old", rebel, None),
        ];
        let (_, labels, _) = profile_rows(twins, "embedded");
        assert_eq!(
            labels,
            ["Embedded", "Adobe Standard (new)", "Adobe Standard (old)"]
        );
    }

    /// Two different profiles naming the body by its two names both fit
    /// it, so both are listed, and the file names tell them apart.
    #[test]
    fn a_body_s_two_names_profiles_list_with_their_file_names() {
        let profiles = vec![
            adobe_standard("850D", "Canon EOS 850D", None),
            adobe_standard("T8i", "Canon EOS Rebel T8i", None),
            // Another body's is not on these rows, so it is no twin.
            adobe_standard("R5", "Canon EOS R5", None),
        ];
        let (names, labels, _) = profile_rows(profiles, "embedded");
        assert_eq!(names, ["embedded", "850D", "T8i"]);
        assert_eq!(
            labels,
            ["Embedded", "Adobe Standard (850D)", "Adobe Standard (T8i)"]
        );
    }
}
