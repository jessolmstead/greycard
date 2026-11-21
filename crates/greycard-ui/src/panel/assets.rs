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
    // What the profile has, for the switches; all three where there is
    // no profile, whose switches stand down on that alone.
    let carries = if chosen.is_from_file() {
        match report {
            LensReport::Found { carries, .. } => Some(*carries),
            _ => None,
        }
    } else {
        options
            .iter()
            .find(|o| o.choice == *chosen)
            .map(|o| o.carries)
    };
    let has = carries.unwrap_or(greycard_engine::lens::Carries::ALL);
    app.set_lens_has_distortion(has.distortion);
    app.set_lens_has_ca(has.chromatic_aberration);
    app.set_lens_has_vignetting(has.vignetting);
    app.set_lens_available(carries.is_some());
    app.set_lens_source(lens_source(report, chosen).unwrap_or_default().into());
    if !chosen.is_from_file() {
        app.set_lens_profile(label.into());
        return;
    }
    app.set_lens_profile(
        match report {
            LensReport::Unread => String::new(),
            LensReport::NoDatabase => tr!("no lens profiles on this machine"),
            LensReport::NoLensName => tr!("the file names no lens"),
            LensReport::Unknown(name) => tr!("no profile for {name}", name = name),
            LensReport::Found {
                lens,
                camera: true,
                smaller_sensor: false,
                ..
            } => lens.clone(),
            LensReport::Found {
                lens,
                camera: true,
                smaller_sensor: true,
                ..
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

/// Where the lens line's profile comes from, after its name: the
/// database for the file's own lens found or one of its lenses picked,
/// an Adobe profile for one of those picked. None where the line names
/// no profile.
fn lens_source(report: &LensReport, chosen: &greycard_edit::LensChoice) -> Option<String> {
    use greycard_edit::LensChoice;
    match chosen {
        LensChoice::FromFile => matches!(report, LensReport::Found { .. }).then(lensfun),
        LensChoice::Database(_) => Some(lensfun()),
        LensChoice::Lcp(_) => Some(
            // Lens section, after the lens name: profile from an Adobe .lcp file
            tr!("Adobe"),
        ),
    }
}

/// The lens database's name, as a source.
fn lensfun() -> String {
    // Lens section, after the lens name: profile from the lensfun database
    tr!("lensfun")
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

/// Whether the profile the edit names is known to have no tone
/// curve: the file's own, or a DCP in the list that carries none. One
/// not in the list (the list still being read, or the file not there)
/// is not known to, and its switch is left as the edit has it.
fn chosen_lacks_curve(st: &State) -> bool {
    let greycard_edit::camera::ProfileChoice::Named(name) = &st.develop.edit.camera.profile else {
        return true;
    };
    st.develop
        .profiles
        .iter()
        .any(|e| &e.name == name && !e.tone_curve)
}

/// What the CAMERA PROFILE section lists for the open file: the
/// file's own calibrations first, then the profiles in the profile
/// directory that were made for this camera, one row a profile however
/// many files hold it, then the chosen one when it is neither. With
/// every camera shown (the switch at the list's foot), every profile,
/// grouped by camera (`greycard_edit::camera::grouped`). The switch is
/// a view of the list: the edit and its picture are the same either
/// way, and a profile the edit names is listed either way.
pub(crate) fn show_profiles(st: &State, edit: &greycard_edit::camera::Camera, app: &App) {
    use greycard_edit::camera;
    let chosen = &edit.profile;
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
        let (shown, n) = filtered(st, body, chosen);
        hidden = n;
        // Two different profiles can go by one name on the same rows:
        // two files made for one body, or for the body under each of
        // its names. The file name beside each tells them apart.
        let shown: Vec<&camera::Entry> = shown.iter().collect();
        let labels = camera::told_apart(&shown);
        rows.extend(
            shown
                .iter()
                .zip(labels)
                .map(|(e, label)| profile_row(&e.name, label, false)),
        );
    }
    // A profile the edit names and the directory does not have is
    // still shown, and chosen, so the panel says what the edit says;
    // while the first listing is still being read, by its name alone,
    // since it is not known to be missing.
    let pending = profiles_pending(st);
    if !embedded && !st.develop.profiles.iter().any(|e| e.name == chosen) {
        rows.push(match pending {
            true => profile_row(chosen, chosen.to_string(), false),
            false => profile_row(chosen, tr!("{name} (missing)", name = chosen), false),
        });
    }
    app.set_camera_profile_rows(ModelRc::new(VecModel::from(rows)));
    app.set_camera_profiles_any(!st.develop.profiles.is_empty());
    app.set_camera_profile(chosen.into());
    // The tone curve's switch, for a profile that carries one. For one
    // not in the list it is shown while the list is still being read,
    // and as the edit has it for a file not there, so a step of the
    // edit does not take the switch off (`read_edit` reads it only
    // where shown).
    app.set_camera_profile_has_curve(
        !embedded
            && match st.develop.profiles.iter().find(|e| e.name == chosen) {
                Some(e) => e.tone_curve,
                // Not read yet: shown, so it does not hide once off.
                None if pending => true,
                None => edit.tone_curve,
            },
    );
    app.set_camera_profile_note(
        match (st.develop.profiles.len(), hidden) {
            (0, _) if pending => String::new(),
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
        match pending {
            true => String::new(),
            false => greycard_edit::camera::warning(
                &st.develop.profiles,
                body_of(st),
                chosen_name.as_str(),
            ),
        }
        .into(),
    );
}

/// What the CAMERA PROFILE list keeps between showings: which listing
/// of the directory it holds, and the camera filter's last answer.
#[derive(Default)]
pub(crate) struct ProfilesKept {
    /// The generation of the listing taken last
    /// (`greycard_edit_io::profiles::Listing`), 0 for none.
    from: u64,
    /// The start's read of the directory is under way, or over and its
    /// listing not yet shown: nothing is known missing yet.
    pub(crate) awaiting: bool,
    /// The filter's answer for the list as it stands, kept until the
    /// list, the camera, or the choice changes.
    filtered: std::cell::RefCell<Option<Filtered>>,
    /// The cameras whose hidden count the log has said for the list
    /// as it stands.
    logged: std::cell::RefCell<std::collections::HashSet<Vec<String>>>,
}

/// The camera filter's answer for one camera and choice.
struct Filtered {
    names: Vec<String>,
    chosen: String,
    shown: Vec<greycard_edit::camera::Entry>,
    hidden: usize,
}

/// The start's listing is still to be shown: nothing is known missing
/// yet.
fn profiles_pending(st: &State) -> bool {
    st.develop.profiles_kept.awaiting
}

#[cfg(test)]
thread_local! {
    /// The times the camera filter ran, and the times the log said a
    /// hidden count, on this thread.
    pub(crate) static PROFILE_FILTERS: std::cell::Cell<(usize, usize)> =
        const { std::cell::Cell::new((0, 0)) };
    /// The times the directory was looked at for a change.
    pub(crate) static PROFILE_LOOKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The files the last look named, and whether it walked by force.
    pub(crate) static PROFILE_LOOKED: std::cell::RefCell<(bool, Vec<PathBuf>)> =
        const { std::cell::RefCell::new((false, Vec::new())) };
}

/// The panel's list replaced: what the filter and the log kept for
/// the last one goes with it.
pub(crate) fn set_profiles(st: &mut State, entries: Vec<greycard_edit::camera::Entry>) {
    st.develop.profiles = entries;
    st.develop.profile_tone_for = None;
    let kept = &mut st.develop.profiles_kept;
    *kept.filtered.get_mut() = None;
    kept.logged.get_mut().clear();
}

/// A test's own list, in place of the directory's: taken as the
/// listing held now, so a look that finds nothing new leaves it.
#[cfg(test)]
pub(crate) fn hand_profiles(st: &mut State, entries: Vec<greycard_edit::camera::Entry>) {
    let held = greycard_edit_io::profiles::refresh(false, &[]);
    st.develop.profiles_kept.from = held.map_or(0, |l| l.generation);
    set_profiles(st, entries);
}

/// The profiles shown for the open camera, and how many the filter
/// hid: one row a profile (a file that is the same profile as another
/// is left out, and the one the edit names stands for its pair), those
/// made for this camera and the one the edit names. Worked out once
/// for each list, camera, and choice: the panel is shown again on
/// every step of an edit, and a list of thousands is not filtered
/// again for each.
fn filtered(
    st: &State,
    body: greycard_edit::camera::Body,
    chosen: &str,
) -> (Vec<greycard_edit::camera::Entry>, usize) {
    let kept = &st.develop.profiles_kept;
    let names = body.names();
    if let Some(f) = kept.filtered.borrow().as_ref()
        && f.names == names
        && f.chosen == chosen
    {
        return (f.shown.clone(), f.hidden);
    }
    #[cfg(test)]
    PROFILE_FILTERS.with(|n| n.set((n.get().0 + 1, n.get().1)));
    let mut shown = Vec::new();
    let mut hidden = 0;
    for e in greycard_edit::camera::listed(&st.develop.profiles, chosen) {
        if e.fits(body) || e.name == chosen {
            shown.push(e.clone());
        } else {
            hidden += 1;
            // The name each side goes by, so a profile hidden by a
            // spelling the maker and the decoder disagree on can be
            // told from one made for another camera.
            tracing::debug!(
                "camera profile {}: made for {:?}, not listed for {:?}",
                e.name,
                e.unique_camera_model.as_deref().unwrap_or("no camera"),
                names
            );
        }
    }
    // The count, once for each camera and list, so a profile a
    // spelling hides (an alias the body does not know) can be looked
    // for in a default log; each one's names are at debug. With no
    // frame open there is no camera to say it for.
    if hidden > 0 && !body.model.is_empty() && kept.logged.borrow_mut().insert(names.clone()) {
        #[cfg(test)]
        PROFILE_FILTERS.with(|n| n.set((n.get().0, n.get().1 + 1)));
        tracing::info!(
            "camera profiles: {hidden} not listed for {names:?}, made for other cameras"
        );
    }
    *kept.filtered.borrow_mut() = Some(Filtered {
        names,
        chosen: chosen.to_string(),
        shown: shown.clone(),
        hidden,
    });
    (shown, hidden)
}

/// Take the directory's listing when it is newer than the one the
/// panel holds; true when it was. Never walks the directory.
fn take_listing(st: &mut State) -> bool {
    let Some(listing) = greycard_edit_io::profiles::held() else {
        return false;
    };
    if listing.generation == st.develop.profiles_kept.from {
        return false;
    }
    st.develop.profiles_kept.from = listing.generation;
    // A listing taken is shown by whoever took it, so the start's is
    // no longer awaited.
    st.develop.profiles_kept.awaiting = false;
    set_profiles(st, listing.entries.to_vec());
    true
}

/// A listing came in off the window's thread: shown, if it is newer.
fn listing_came(st: &mut State, app: &App) {
    if take_listing(st) {
        show_profiles(st, &st.develop.edit.camera, app);
        // A profile put there by hand is the edits' that name it.
        crate::edited::looks_moved(st);
    }
}

/// What the next look at the directory is to do: walk it whatever it
/// finds, or not; and the listed files to look at besides its folders.
#[derive(Default, Debug, PartialEq)]
struct Wanted {
    force: bool,
    files: Vec<PathBuf>,
}

/// The looks asked off the window's thread. One runs at a time: a look
/// asked while one runs is merged into the next, so a share that hangs
/// holds one thread, not one an open.
#[derive(Default)]
struct LookQueue {
    running: bool,
    wanted: Option<Wanted>,
}

impl LookQueue {
    /// A look asked: merged into the one waiting, `force` if either is
    /// and the files of both. True when no look runs, for the caller to
    /// start the thread that runs it.
    fn ask(&mut self, force: bool, files: Vec<PathBuf>) -> bool {
        let wanted = self.wanted.get_or_insert_with(Wanted::default);
        wanted.force |= force;
        wanted.files.extend(files);
        wanted.files.sort();
        wanted.files.dedup();
        !std::mem::replace(&mut self.running, true)
    }

    /// The running thread's next look, or `None` for it to end: the
    /// slot is freed in the same step, so a look asked just after starts
    /// a thread of its own rather than waiting on one that is going.
    fn next(&mut self) -> Option<Wanted> {
        let next = self.wanted.take();
        if next.is_none() {
            self.running = false;
        }
        next
    }
}

static LOOKING: std::sync::Mutex<LookQueue> = std::sync::Mutex::new(LookQueue {
    running: false,
    wanted: None,
});

/// Run the looks `queue` holds, one `pass` each, until none is left. A
/// pass that panics frees the slot on the way out, so the next look
/// asked starts a thread again.
fn run_looks(queue: &std::sync::Mutex<LookQueue>, mut pass: impl FnMut(Wanted)) {
    struct Panicked<'a>(&'a std::sync::Mutex<LookQueue>);
    impl Drop for Panicked<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0.lock().unwrap_or_else(|e| e.into_inner()).running = false;
            }
        }
    }
    let _freed = Panicked(queue);
    loop {
        // Its own statement: the queue is let go before the pass, which
        // can take a while on a slow share, so a look asked meanwhile
        // (from the window's thread) never waits on it. A guard in a
        // `while let` would be held through the loop's body.
        let next = queue.lock().unwrap_or_else(|e| e.into_inner()).next();
        let Some(wanted) = next else {
            return;
        };
        pass(wanted);
    }
}

/// Look at the directory for a change (`greycard_edit_io::profiles::
/// refresh`): its folders' clocks and `files`' lengths and clocks,
/// walking it again when one moved or `force` is set. Off the window's
/// thread ([`LookQueue`]), each answer coming back through the event
/// loop to be shown if it is newer, so the window waits on no disk. A
/// test looks at once, and takes the answer itself.
fn look(app: &App, force: bool, files: Vec<PathBuf>) {
    if cfg!(test) {
        #[cfg(test)]
        {
            PROFILE_LOOKS.with(|n| n.set(n.get() + 1));
            PROFILE_LOOKED.with(|l| *l.borrow_mut() = (force, files.clone()));
        }
        greycard_edit_io::profiles::refresh(force, &files);
        return;
    }
    if !LOOKING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .ask(force, files)
    {
        return;
    }
    let weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("profiles".into())
        .spawn(move || {
            run_looks(&LOOKING, |wanted| {
                greycard_edit_io::profiles::refresh(wanted.force, &wanted.files);
                let _ = weak.upgrade_in_event_loop(|app| {
                    if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                        listing_came(&mut state.borrow_mut(), &app);
                    }
                });
            });
        });
    if let Err(e) = spawned {
        LOOKING.lock().unwrap_or_else(|e| e.into_inner()).running = false;
        tracing::warn!("camera profiles not looked at: {e}");
    }
}

/// [`look`], with the profiles on show named, and its answer taken
/// at once in a test.
fn look_at_profiles(st: &mut State, app: &App, force: bool) {
    look(app, force, shown_files(st));
    if cfg!(test) {
        listing_came(st, app);
    }
}

/// The files of the profiles on show for the open camera, and of the
/// one the edit names: a profile rewritten in place moves no folder's
/// clock, and these are the ones whose header the panel shows.
fn shown_files(st: &State) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = st
        .develop
        .profiles_kept
        .filtered
        .borrow()
        .as_ref()
        .map(|f| f.shown.iter().map(|e| e.path.clone()).collect())
        .unwrap_or_default();
    let chosen = st.develop.edit.camera.profile.name();
    files.extend(
        st.develop
            .profiles
            .iter()
            .filter(|e| e.name == chosen)
            .map(|e| e.path.clone()),
    );
    files
}

/// The start's read of the directory, on a thread nothing waits for:
/// what it lists is shown when it is done. True when the thread runs,
/// for the window to await its listing ([`ProfilesKept::awaiting`]).
pub(crate) fn warm_profiles(app: &App) -> bool {
    /// The read is over, however it ended, a panic included: the window
    /// is told, and shows the listing, or stops waiting for one.
    struct Over(slint::Weak<App>);
    impl Drop for Over {
        fn drop(&mut self) {
            let _ = self.0.upgrade_in_event_loop(|app| {
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    read_over(&mut state.borrow_mut(), &app);
                }
            });
        }
    }
    let over = Over(app.as_weak());
    let spawned = std::thread::Builder::new()
        .name("profiles".into())
        .spawn(move || {
            let _over = over;
            greycard_edit_io::profiles::warm();
        });
    if let Err(e) = spawned {
        tracing::warn!("camera profiles not read ahead: {e}");
        return false;
    }
    true
}

/// The start's read is over: its listing shown if it is newer, and the
/// list shown again either way, a profile not there now said missing.
fn read_over(st: &mut State, app: &App) {
    st.develop.profiles_kept.awaiting = false;
    let came = take_listing(st);
    show_profiles(st, &st.develop.edit.camera, app);
    if came {
        crate::edited::looks_moved(st);
    }
}

/// A frame opened: the listing held is taken if it is newer, never
/// walked for. [`profiles_shown`] follows once the list is shown.
pub(crate) fn profiles_opened(st: &mut State) {
    take_listing(st);
}

/// The list shown for an opened frame: the directory looked at for a
/// change off the window's thread, the profiles on show by their
/// files, so a profile added or rewritten since is shown when the look
/// comes back.
pub(crate) fn profiles_shown(st: &mut State, app: &App) {
    look_at_profiles(st, app, false);
}

/// The profile directory for a sync, a paste, or a preset, which fit
/// a named profile to each frame's body: the listing held, the
/// directory looked at for a change off the window's thread as on an
/// open. Before the start's read is done there is none held, and this
/// waits for it.
pub(crate) fn profiles_for_sync(app: &App) -> Vec<greycard_edit::camera::Entry> {
    match greycard_edit_io::profiles::held() {
        Some(listing) => {
            look(app, false, Vec::new());
            listing.entries.to_vec()
        }
        None => greycard_edit_io::profiles::list(),
    }
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
    // Under the profile's own tone curve a look of ours is skipped
    // (`look::over_profile_curve`): its row is dimmed as one with no
    // table for the curve is, and no line asks for a refit.
    let profile_curve = app.get_camera_tone_curve() && app.get_camera_profile_has_curve();
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
                off: off
                    || (profile_curve
                        && greycard_edit::look::fitted(&st.develop.looks, &name, curve)),
                name: name.into(),
                label: label.into(),
                note: curves.into(),
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
    let (mismatch, refit) = if profile_curve {
        (String::new(), String::new())
    } else {
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
        }
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
    // The CAMERA PROFILE section opened: the directory walked again on
    // a thread, each header whose file changed read again, so a profile
    // put there or rewritten while the editor runs is listed as it is
    // now. Every open looks too (`profiles_shown`), more cheaply.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_camera_profiles_wanted(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            look_at_profiles(&mut state.borrow_mut(), &app, true);
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
            // The tone curve's switch is off for a profile with no curve
            // (or one not in the list): kept, it would be written and
            // change the develop key without changing a pixel, and come
            // back unasked with the next profile that has one.
            if chosen_lacks_curve(&st) {
                st.develop.edit.camera.tone_curve = false;
            }
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
            show_profiles(&st, &st.develop.edit.camera, &app);
            app.set_camera_tone_curve(st.develop.edit.camera.tone_curve);
            develop_soon(&mut st, state.clone(), worker.clone(), app.as_weak());
            // The profile picked by its file, so a header rewritten in
            // place since is read again and its switch follows.
            look_at_profiles(&mut st, &app, false);
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
            show_profiles(&st, &st.develop.edit.camera, &app);
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
            show_profiles(&st, &st.develop.edit.camera, &app);
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
    // logic with fake bodies; this closure just resolves the preset,
    // and `sync::preset_pressed` the profile directory's listing
    // (`profiles_for_sync`).
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
                        "*.lrtemplate".into(),
                        "*.LRTEMPLATE".into(),
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
mod tests;
