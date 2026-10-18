use crate::panel::browser::file_name;
use crate::panel::cull::{control_over_frame, leave_cull};
use crate::panel::edit::{develop_soon, read_edit, schedule_save};
use crate::panel::sync;
use crate::*;

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
pub(crate) const LENSES_WHY: &str = "No lens profiles installed; lenses aren't corrected.";

/// Open the download sheet for the lens database: `unprompted` when
/// no one asked for it, and the sheet says why it is up.
pub(crate) fn offer_lenses(st: &mut State, app: &App, unprompted: bool) {
    st.sheets.fetch = Some(Fetch::Lenses);
    app.set_fetch_title("Download the lens profiles?".into());
    let details = format!(
        "The lensfun database, about {} KB from {}.
License: {} ({}).
Kept in {}.",
        greycard_lens::store::APPROX_BYTES / 1000,
        greycard_lens::store::SOURCES[0]
            .split('/')
            .nth(2)
            .unwrap_or("its site"),
        greycard_lens::store::LICENSE.0,
        greycard_lens::store::LICENSE.1,
        greycard_lens::Store::user().map_or("no cache directory".to_string(), |s| s
            .root()
            .display()
            .to_string()),
    );
    // Unasked, the sheet says first why it is up: the reason is the
    // news, and the particulars follow it.
    let text = if unprompted {
        format!("{LENSES_WHY}\n\n{details}")
    } else {
        details
    };
    app.set_fetch_text(text.into());
    let note = "Their own license.";
    app.set_fetch_note(
        if unprompted {
            format!("{note} Not now won't ask again; it's also in LENS.")
        } else {
            note.to_string()
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
        Some(_) => format!("greycard's faster copy; {} license.", model.license.name),
        None => "Its own license.".to_string(),
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
            format!(
                "Your Subject model runs on the CPU here; a GPU version is {} MB. {note}",
                mb(model.bytes()),
            )
        } else {
            note
        }
        .into(),
    );
    app.set_fetch_title(format!("Download {}?", model.name).into());
    app.set_fetch_text(
        format!(
            "{} MB from {}.
License: {} ({}).
Kept in {}.",
            mb(model.bytes()),
            model.files[0]
                .url
                .split('/')
                .nth(2)
                .unwrap_or("the model host"),
            model.license.name,
            model.license.url,
            st.develop
                .store
                .as_ref()
                .map_or("no cache directory".to_string(), |s| s
                    .root()
                    .display()
                    .to_string()),
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
pub(crate) const RUNTIME_TITLE: &str = "Download ONNX Runtime for the CPU?";

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
    app.set_fetch_title(RUNTIME_TITLE.into());
    let reason = why.reason();
    let mut chars = reason.chars();
    let reason = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect::<String>())
        .unwrap_or_default();
    app.set_fetch_text(
        format!(
            "{reason}, so the AI features are off.

{name} runs them on the CPU only, slower: {download} MB from {host}, {kept} MB kept.
License: {license} ({url}).
Kept in {kept_in}.",
            name = runtime.name.split(',').next().unwrap_or(runtime.name),
            download = mb(runtime.archive.bytes),
            host = runtime.archive.url.split('/').nth(2).unwrap_or("its site"),
            kept = mb(runtime.bytes()),
            license = runtime.license.name,
            url = runtime.license.url,
            kept_in = st
                .develop
                .store
                .as_ref()
                .map_or("no cache directory".to_string(), |s| s
                    .root()
                    .display()
                    .to_string()),
        )
        .into(),
    );
    app.set_fetch_note(
        "Microsoft's own build, as published. Not now keeps the AI features off this session."
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
pub(crate) const EDGE_DECLINED: &str =
    "Whole person keeps its coarser edge without the Subject model";

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
        format!(
            "For a Whole person's finer edge, hair against the sky; the mask is made \
             meanwhile with a coarser one. {}",
            model_note(model)
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
}

/// List the store again and show it.
pub(crate) fn refresh_presets(st: &mut State, app: &App) {
    st.develop.presets = st
        .develop
        .preset_store
        .as_ref()
        .map(|s| s.list())
        .unwrap_or_default();
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
        return "No configuration directory to keep presets in.".into();
    }
    match missing {
        [] => "Every default preset is in the list.".into(),
        [one] => format!("{one} is not in the list."),
        [rest @ .., last] => format!("{} and {last} are not in the list.", rest.join(", ")),
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
        app.set_status("no configuration directory to keep presets in".into());
        return;
    };
    let brought: Vec<Brought> = paths.iter().map(|p| bring(store, p)).collect();
    refresh_presets(st, app);
    show_import_report(app, import_report(&brought));
}

/// One file into the store, and what it came to.
fn bring(store: &preset::Store, path: &Path) -> Brought {
    let file = file_name(path);
    // Never over a preset the list has: its bin is the way to a new
    // copy, and a refusal here says so.
    match store.import(path, false) {
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
                carried: preset::carried_words(preset.sections.len(), preset.parts.len()),
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
pub(crate) fn refusal_words(file: &str, e: &preset::ImportError) -> String {
    use preset::ImportError;
    match e {
        ImportError::Unreadable(e) => format!("{file} could not be read: {e}."),
        ImportError::NotAPreset(why) => format!("{file} is not a preset: {why}."),
        ImportError::Newer(_) => format!("{file} is {e}."),
        ImportError::Nameless => format!("{file} was not imported: {e}."),
        ImportError::Taken(name) => {
            format!("{file} was not imported: there is a preset called \"{name}\" already.")
        }
        ImportError::Unsaved(_, e) => format!(
            "{file} could not be written into the list: {}.",
            preset::plain(e)
        ),
    }
}

/// What an import's report says, and where.
pub(crate) fn import_report(brought: &[Brought]) -> Report {
    let line = |name: &str, carried: &str, passed: &[String]| {
        let mut line = format!("{name} ({carried})");
        if !passed.is_empty() {
            line.push_str(&format!("; no place here for {}", passed.join(", ")));
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
        let status = format!("imported {}", line(name, carried, passed));
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
        ([name], 1) => format!("Imported {name}"),
        ([], 1) => "Not imported".to_string(),
        ([], _) => "No presets imported".to_string(),
        (_, n) if names.len() == n => format!("Imported {n} presets"),
        (_, n) => format!("Imported {} of {n} presets", names.len()),
    };
    let note = if passed_any {
        "What had no place here is named as Lightroom names it; set it by hand where a \
         picture wants it."
            .to_string()
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

/// What the LENS panel says of the open file: the profile found, or
/// why there is none.
pub(crate) fn show_lens(report: &LensReport, app: &App) {
    app.set_lens_available(matches!(report, LensReport::Found { .. }));
    app.set_lens_database(!matches!(report, LensReport::NoDatabase));
    app.set_lens_profile(
        match report {
            LensReport::NoDatabase => "no lens profiles on this machine".to_string(),
            LensReport::NoLensName => "the file names no lens".to_string(),
            LensReport::Unknown(name) => format!("no profile for {name}"),
            LensReport::Found {
                lens,
                camera: true,
                smaller_sensor: false,
            } => lens.clone(),
            LensReport::Found {
                lens,
                camera: true,
                smaller_sensor: true,
            } => format!("{lens} (measured on a smaller sensor)"),
            LensReport::Found {
                lens,
                camera: false,
                ..
            } => format!("{lens} (body unknown)"),
        }
        .into(),
    );
}

/// What the CAMERA PROFILE section lists for the open file: the
/// file's own calibrations first, then the profiles in the profile
/// directory that were made for this camera, then the chosen one when
/// it is neither.
pub(crate) fn show_profiles(st: &State, chosen: &greycard_edit::camera::ProfileChoice, app: &App) {
    let (make, model) = (st.develop.camera.0.as_str(), st.develop.camera.1.as_str());
    let mut names = vec![slint::SharedString::from(greycard_edit::camera::EMBEDDED)];
    let mut labels = vec![slint::SharedString::from("Embedded")];
    let embedded = chosen.is_embedded();
    let chosen_name = chosen.name().to_string();
    let chosen = chosen_name.as_str();
    let mut hidden = 0;
    for e in &st.develop.profiles {
        if e.fits(make, model) || e.name == chosen {
            names.push(e.name.as_str().into());
            labels.push(e.label().into());
        } else {
            hidden += 1;
        }
    }
    // A profile the edit names and the directory does not have is
    // still shown, and chosen, so the panel says what the edit says.
    if !embedded && !names.iter().any(|n| n.as_str() == chosen) {
        names.push(chosen.into());
        labels.push(format!("{chosen} (missing)").into());
    }
    app.set_camera_profile_names(ModelRc::new(VecModel::from(names)));
    app.set_camera_profile_labels(ModelRc::new(VecModel::from(labels)));
    app.set_camera_profile(chosen.into());
    app.set_camera_profile_note(
        match (st.develop.profiles.len(), hidden) {
            (0, _) => match greycard_edit::camera::store_dir() {
                Some(dir) => format!("No profiles yet: put DCP files in {}.", dir.display()),
                None => "No profile directory on this machine.".to_string(),
            },
            (_, 0) => String::new(),
            (_, n) => format!("{n} more there, for other cameras."),
        }
        .into(),
    );
    app.set_camera_profile_warning(
        greycard_edit::camera::warning(
            &st.develop.profiles,
            &st.develop.camera,
            chosen_name.as_str(),
        )
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
            Some((text, true)) => (text, format!("Refit under {}…", curve.phrase())),
            Some((text, false)) => (text, String::new()),
            None => greycard_edit::look::stale(
                &st.develop.looks,
                chosen_name,
                curve,
                greycard_match::FIT_VERSION,
            )
            .map(|text| (text, "Refit…".to_string()))
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
        None if st.develop.looks.is_empty() => match greycard_edit::look::store_dir() {
            Some(dir) => format!(
                "No looks yet. Add .cube or HaldCLUT .png files to {}.",
                dir.display()
            ),
            None => "No look directory on this machine.".to_string(),
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

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
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
                    st.sheets.fetching = true;
                    worker.send(Job::Fetch { model });
                }
                (Fetch::Edge(_), false) => {
                    st.sheets.edge_declined = true;
                    app.set_status(EDGE_DECLINED.into());
                }
                (Fetch::Model(model), false) if model.id == greycard_ai::SAM3.id => {
                    st.sheets.declined.push(model.id);
                    crate::panel::parts::model_answered(&mut st, &app, false);
                    if st.develop.part_pending.is_none() {
                        app.set_status(
                            "without the People model a People shape masks nothing".into(),
                        );
                    }
                }
                (Fetch::Model(model), false) => {
                    st.sheets.declined.push(model.id);
                    let lost = if model.id == greycard_ai::SUBJECT_WEBGPU.id {
                        // The masks waiting on it turn to the original,
                        // which the next frame offers.
                        app.window().request_redraw();
                        "without the GPU model the original Subject model is offered"
                    } else if model.id == greycard_ai::FILL.id {
                        "without the model a fill is left as it was"
                    } else if greycard_ai::DENOISERS.iter().any(|(_, m)| m.id == model.id) {
                        "without the model the engine's own denoise stands in"
                    } else {
                        "without the model the shape masks nothing"
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
                            format!(
                                "downloading {}",
                                runtime.name.split(',').next().unwrap_or("")
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
                        "without the profiles the lens is corrected by hand only".into(),
                    );
                    // Asked once: the offer does not come back on its
                    // own, this run or the next. Not from a batch run,
                    // which leaves the user's settings alone, nor from
                    // a test, which has no business with them.
                    st.sheets.lenses_declined = true;
                    if !st.window.batch && !cfg!(test) {
                        let mut settings = settings::Settings::load();
                        if !settings.lenses_declined {
                            settings.lenses_declined = true;
                            settings.save();
                        }
                    }
                }
            }
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
            st.develop.profiles = greycard_edit::camera::list();
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
            show_profiles(&st, &st.develop.edit.camera.profile, &app);
            develop_soon(&mut st, state.clone(), worker.clone(), app.as_weak());
        });
    }
    // The LOOK section opened: read the directory again, as the
    // profile directory's is read, and ask for the chosen table
    // again, so a file replaced in place is picked up. Asking is
    // cheap: `look::load` hands back the same table unless its size
    // or its clock moved, and the renderer re-uploads nothing for a
    // table it already holds.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_looks_wanted(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.develop.looks = greycard_edit::look::list();
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
    // and the profile directory `camera::list` reads for real.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_preset_applied(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
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
                app.set_status("a preset needs a name".into());
                return;
            }
            let choice = st.sheets.preset_checks.choice();
            if choice.is_empty() {
                app.set_status("choose what the preset carries".into());
                return;
            }
            // The demosaic as the open frame runs it, which is what its
            // menu shows.
            let edit = crate::panel::sync::as_source(
                read_edit(&app, &st.develop.edit, st.develop.target),
                st.open_mosaic().kind(),
            );
            let preset = Preset::from_choice(&name, &edit, &choice);
            app.set_preset_open(false);
            match st.develop.preset_store.as_ref().map(|s| s.save(&preset)) {
                Some(Ok(path)) => {
                    app.set_status(
                        format!(
                            "saved {} ({}) to {}",
                            preset.name,
                            choice.words(),
                            path.display()
                        )
                        .into(),
                    );
                }
                Some(Err(e)) => {
                    tracing::warn!("preset {}: not saved: {e}", preset.name);
                    app.set_status(format!("saving {}: {e}", preset.name).into());
                }
                None => {
                    tracing::warn!(
                        "preset {}: no configuration directory to save into",
                        preset.name
                    );
                    app.set_status("no configuration directory to save into".into());
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
            match st.develop.preset_store.as_ref().map(|s| s.remove(&entry)) {
                Some(Ok(())) => {
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
                            format!("{name} removed; Settings can restore it")
                        } else {
                            format!("{name} removed")
                        }
                        .into(),
                    );
                }
                Some(Err(e)) => {
                    tracing::warn!("preset {}: not removed: {e}", entry.preset.name);
                    app.set_status(format!("removing {}: {e}", entry.preset.name).into());
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
                    app.set_status("every default preset is in the list".into());
                }
                Some(Ok(names)) => {
                    tracing::info!("presets restored: {}", names.join(", "));
                    app.set_status(format!("restored {}", names.join(", ")).into());
                }
                Some(Err(e)) => {
                    tracing::warn!("presets: not restored: {e}");
                    app.set_status(format!("restoring the default presets: {e}").into());
                }
                None => {
                    app.set_status("no configuration directory to restore into".into());
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
            app.set_status("choosing presets to import...".into());
            // As many as the user picks, Lightroom's and this engine's
            // together. A test opens no chooser: it answers as a cancel.
            chooser::choose_open_many(
                "Import presets",
                start,
                (
                    "Presets".to_string(),
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
                                app.set_status("import canceled".into());
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
        assert_eq!(app.get_status(), EDGE_DECLINED);
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
        assert_eq!(app.get_fetch_title(), RUNTIME_TITLE);
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
        assert!(app.get_fetch_text().starts_with(LENSES_WHY));

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
        assert!(!app.get_fetch_text().contains(LENSES_WHY));
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
    /// own strip, top down and never past it: the pane scrolls its
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
        let _ = std::fs::remove_dir_all(&d);
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

        // The pane is 240px wide, its bins in the right-hand strip;
        // the list is one of the first things in it with the
        // navigator folded, so a short sweep finds the first row's
        // bin well short of anything below the list.
        let mut bin = None;
        'sweep: for y in (60..280).step_by(3) {
            for x in (150..240).step_by(6) {
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
        // the list, in the pane's own width, lands on the list, the
        // question or the Cancel/Remove row — never the pane's
        // footer, whatever the settled height turns out to be.
        crate::testing::click(&app, bx, by);
        assert_eq!(app.get_preset_confirm_remove(), 0);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(150));
        let mut removed = false;
        'remove: for y in (list_bottom as i32..list_bottom as i32 + 150).step_by(3) {
            for x in (0..240).step_by(6) {
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
        let _ = std::fs::remove_dir_all(&d);
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
        let _ = std::fs::remove_dir_all(&d);
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
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_batch_run_is_never_offered_the_lens_profiles() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        state.borrow_mut().window.batch = true;
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open());
    }
}
