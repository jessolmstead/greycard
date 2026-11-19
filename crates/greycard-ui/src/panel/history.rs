use std::path::Path;
use std::rc::Rc;

use crate::tr;
use greycard_edit::{Edit, Exported, Row};
use greycard_library::export_record::{no_version, not_kept, record_export_on_disk, take_seed};
use greycard_render::finish;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::panel::cull::leave_cull;
use crate::panel::edit::{
    current_turn, edit_to_develop, read_edit, record_panel, show_edit, write_file, write_sidecar,
};
use crate::worker::{Job, Worker};
use crate::{App, HistoryRow, Shared, State};

/// The undo and redo buttons, the history's rows and the snapshots'
/// names follow the current file's sidecar. The rows are newest
/// first, each named for what it changed from the one before, and
/// each export made from a state a row just above that state's.
pub(crate) fn show_history(st: &State, app: &App) {
    let names = |v: Vec<String>| {
        ModelRc::new(VecModel::from(
            v.into_iter()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>(),
        ))
    };
    let Some((c, sidecar)) = st.current.and_then(|c| Some((c, st.line(c)?))) else {
        app.set_history_rows(ModelRc::default());
        app.set_history_current(-1);
        app.set_history_version("".into());
        app.set_snapshot_names(names(Vec::new()));
        app.set_can_undo(false);
        app.set_can_redo(false);
        app.set_history_resettable(false);
        app.set_section_resets(Default::default());
        return;
    };
    // Which version the history is of, after its title: none for the
    // file's own.
    app.set_history_version(st.version_name(c).unwrap_or_default().into());
    app.set_can_undo(!sidecar.history.is_empty());
    app.set_can_redo(!sidecar.redo.is_empty());
    app.set_history_resettable(!crate::panel::reset::at_original(st, c));
    // The headers' resets by the panel, which a frame's open puts its
    // edit on before the frame is the current one.
    let panel = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    crate::panel::section_reset::show_section_resets(st, app, &panel, st.develop.target);
    // Whatever moved the history (a save, a reset, an undo, an export
    // landing) moved what the frame's cell says of its edit and its
    // exports; the cell is written only if it differs.
    crate::panel::browser::show_badges(st, app, c);
    let now = now();
    let position = sidecar.position();
    let rows: Vec<HistoryRow> = sidecar
        .rows()
        .into_iter()
        .map(|row| match row {
            Row::State(i) => HistoryRow {
                name: state_name(sidecar, i).into(),
                exported: false,
                time: Default::default(),
                undone: i > position,
            },
            // A record, not a change: marked as one, with when.
            Row::Exported { state, export } => {
                let e = &sidecar.exports(state)[export];
                HistoryRow {
                    name: e.label().into(),
                    exported: true,
                    time: row_time(e.at, now).into(),
                    undone: state > position,
                }
            }
        })
        .collect();
    app.set_history_rows(ModelRc::new(VecModel::from(rows)));
    app.set_history_current(
        sidecar
            .row_of_state(sidecar.position())
            .map_or(-1, |r| r as i32),
    );
    app.set_snapshot_names(names(
        sidecar.snapshots.iter().map(|s| s.name.clone()).collect(),
    ));
}

/// What the history's row for the state at `i` says: the words the
/// step was recorded with, or what moved.
fn state_name(sidecar: &greycard_edit::Line, i: usize) -> String {
    let state = sidecar.state(i).expect("a state within the count");
    match sidecar.describe(i) {
        Some(name) => name,
        // A fresh raw's learned-denoiser blend starts from its ISO
        // (`Noise::blend_for_iso`), not the plain default, so that
        // field alone is left out of the comparison; its lens section
        // starts as the setting for new edits said.
        None if {
            let mut plain = state.clone();
            plain.noise.learned_strength = Edit::default().noise.learned_strength;
            Edit::raw_defaults().contains(&plain) || Edit::picture_defaults().contains(state)
        } =>
        {
            // History: the first step, the frame as it came
            tr!("Original")
        }
        // History: the first step kept once earlier ones were dropped
        None => tr!("Earliest kept"),
    }
}

/// Note in `source`'s history that `written` was exported from it
/// under `edit`, with `preset` the export preset the sheet was: on
/// the state that is `edit`, which is the one the file was rendered
/// under whatever the panel has done since (see
/// [`Sidecar::record_export`]), and the sidecar written as any save
/// writes it. Here on the window's thread, where every other save of
/// the frame's sidecar is made, so the two never cross.
///
/// A frame the window has let go of since the export began (another
/// folder opened) has its sidecar read from where it is, noted, and
/// written back. Nothing is noted when the edit is no state of the
/// history (one undone and replaced since, or the command line's
/// overrides, which are never the frame's), or the sidecar will not
/// read.
pub(crate) fn record_export(
    st: &mut State,
    app: &App,
    source: &Path,
    edit: &Edit,
    version: Option<&str>,
    written: &Path,
    preset: Option<String>,
) {
    let exported = Exported {
        file: written.to_string_lossy().into_owned(),
        preset,
        at: now(),
    };
    // A frame in the list whose sidecar stands in from its row (the
    // list replaced while the export ran) is recorded on disk below,
    // as a frame the window has let go of is: a stand-in is never
    // written.
    if let Some(file) = st
        .file_by_path(source)
        .filter(|&f| crate::rows::file_is_loaded(st, f))
    {
        // A line never opened: its blend was seeded for the export as
        // its first open would have, and that line takes the seed now.
        take_seed(&mut st.sidecars[file], source, version, edit);
        let Some(line) = st.sidecars[file].line_mut(version) else {
            no_version(source, version);
            return;
        };
        if !line.record_export(edit, exported) {
            not_kept(source);
            return;
        }
        write_file(st, file);
        if st.current.is_some_and(|c| st.file_at(c) == Some(file)) {
            show_history(st, app);
        }
        // The mark on the version's cell, the file's other cells as
        // they were.
        crate::panel::browser::show_badges_of_file(st, app, file);
        return;
    }
    if st.window.write_sidecars
        && record_export_on_disk(
            source,
            edit,
            version,
            exported,
            st.window.placement,
            st.window.xmp_sidecars,
        )
    {
        if let Some(indexer) = &st.views.index {
            indexer.file(source.to_path_buf());
        }
        // The archive's copy, from the file (§233).
        crate::sync::after_disk_save(st, source);
        // The frame's cell while its sidecar stands in from the row:
        // the export is on the file now, and the row says so after the
        // indexer's next look, which the cell need not wait for.
        if let Some(file) = st.file_by_path(source)
            && let Some(from) = st.from_row.get_mut(file)
        {
            match version {
                None => from.exported = true,
                Some(id) => {
                    if let Some(v) = from.versions.iter_mut().find(|v| v.id == id) {
                        v.exported = true;
                    }
                }
            }
            crate::panel::browser::show_badges_of_file(st, app, file);
        }
    }
}

/// A moment in local time, broken into its parts.
fn local(secs: u64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::from_timestamp(i64::try_from(secs).ok()?, 0)
        .map(|t| t.with_timezone(&chrono::Local))
}

/// A moment as a date and a time in local time, in the form
/// [`date_of`] writes: what an export's row says on hover.
pub(crate) fn local_date_of(secs: u64) -> String {
    use chrono::{Datelike, Timelike};
    match local(secs) {
        Some(t) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            t.year(),
            t.month(),
            t.day(),
            t.hour(),
            t.minute()
        ),
        None => date_of(secs),
    }
}

/// When an export was written, as its row has room for: the time when
/// that was on the day of `now`, the date otherwise; both local.
fn row_time(secs: u64, now: u64) -> String {
    use chrono::{Datelike, Timelike};
    let (Some(t), Some(today)) = (local(secs), local(now)) else {
        return String::new();
    };
    if t.date_naive() == today.date_naive() {
        format!("{:02}:{:02}", t.hour(), t.minute())
    } else {
        format!("{:04}-{:02}-{:02}", t.year(), t.month(), t.day())
    }
}

/// Show `edit` in the viewport in place of the panel's while a row
/// is under the pointer, saying so on the status line, or put the
/// panel's back for none.
pub(crate) fn peek(st: &mut State, app: &App, edit: Option<Edit>, what: &str) {
    match edit {
        Some(edit) => {
            let panel = read_edit(app, &st.develop.edit, st.develop.target);
            let unseen = finish::not_previewed(&edit, &panel, st.open_mosaic().kind());
            let note = if unseen.is_empty() {
                String::new()
            } else {
                tr!(
                    "; the {what} shown once restored",
                    what = unseen.join(&tr!(" and "))
                )
            };
            if st.develop.status_kept.is_none() {
                st.develop.status_kept = Some(app.get_status());
            }
            if st.develop.view_kept.is_none() {
                st.develop.view_kept = Some((st.develop.image_size, st.develop.center));
            }
            app.set_status(format!("{what}{note}").into());
            st.develop.peek = Some(edit);
        }
        None => {
            if st.develop.peek.take().is_some() {
                if let Some(status) = st.develop.status_kept.take() {
                    app.set_status(status);
                }
                if let Some((size, center)) = st.develop.view_kept.take() {
                    st.develop.image_size = size;
                    st.develop.center = center;
                }
            }
        }
    }
    app.window().request_redraw();
}

/// The status line's words over a snapshot's row: its name and when
/// it was taken, and what a click on the `part` under the pointer
/// does ("row", or one of its icons: "restore", "rename", "remove").
fn snapshot_hint(name: &str, taken: &str, part: &str) -> String {
    match part {
        "restore" => tr!(
            "{name}, {taken}; click to restore it",
            name = name,
            taken = taken
        ),
        "rename" => tr!(
            "{name}, {taken}; click to rename it",
            name = name,
            taken = taken
        ),
        "remove" => tr!(
            "{name}, {taken}; click to remove it",
            name = name,
            taken = taken
        ),
        _ => tr!(
            "{name}, {taken}; click to restore, double-click to rename",
            name = name,
            taken = taken
        ),
    }
}

/// A moment as a date and a time, UTC.
pub(crate) fn date_of(secs: u64) -> String {
    // Howard Hinnant's civil-from-days.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let (h, min) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02} UTC")
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Show the current file's sidecar's current edit in place of
/// whatever the panel holds, write the sidecar, and develop if the
/// engine's part changed: what an undo, a redo or a preset does once
/// the sidecar says what is current.
pub(crate) fn take_current(st: &mut State, app: &App, worker: &Worker) {
    take_current_with(st, app, worker, true);
}

/// [`take_current`], the sidecar written only with `save`: a paste or a
/// preset over a set whose write of the frame's file already carried
/// the state made current has nothing more to write.
pub(crate) fn take_current_with(st: &mut State, app: &App, worker: &Worker, save: bool) {
    // The export queue's frame keeps its queued edit on the panel.
    if st.current.is_none() || crate::panel::queue_view::refused(st, "a state made current") {
        return;
    }
    // The panel is the sidecar's again: the command line's overrides,
    // if they were on it, are done with (`panel_state`).
    st.launch.overridden = None;
    // An undo, a redo, a row or a snapshot made current: a person
    // offered for the other frames was picked on the state left.
    crate::panel::parts_all::history_moved(st, app);
    // In culling the sidecar's current state is what the leaving
    // develops; the history moved, and the mode ends on that.
    if st.culling.cull.is_some() {
        leave_cull(st, app, worker, None);
        return;
    }
    // In Cull drawn developed the panel is the frame's already: the
    // edit changed, so the mode ends as camera Cull's does, and the
    // state made current goes on the panel and develops below.
    crate::panel::cull::leave_developed(st, app);
    take_current_settled_with(st, app, worker, save);
}

/// [`take_current`] for a sidecar that changed under the window (an
/// archive's copy joined into it, §233) rather than by its hand: the
/// panel shows the sidecar's current state, written and developed
/// again as an undo's would be, but culling, when it is on, goes on;
/// the leaving develops the current state as it always does.
pub(crate) fn take_current_settled(st: &mut State, app: &App, worker: &Worker) {
    take_current_settled_with(st, app, worker, true);
}

/// [`take_current_settled`], the sidecar written only with `save`.
fn take_current_settled_with(st: &mut State, app: &App, worker: &Worker, save: bool) {
    let Some(c) = st.current else {
        return;
    };
    // The panel's white is the state's now, not one As shot's switch
    // replaced: an undo of the switch shows the custom white again,
    // and a redo of it gives back nothing.
    crate::panel::color::forget_before_as_shot(st);
    if save && crate::panel::edit::writable(st, c) {
        crate::panel::edit::save_sidecar(st, c);
    }
    if st.culling.cull.is_some() {
        return;
    }
    let Some(edit) = st.line(c).map(|l| l.current.clone()) else {
        return;
    };
    if st
        .develop
        .target
        .is_some_and(|i| i >= edit.adjustments.len())
    {
        st.develop.target = None;
    }
    show_edit(st, &edit, app, st.develop.target);
    show_history(st, app);
    // The panel's edit is the state's whether or not the engine has
    // to develop again: `read_edit` takes from it all the panel does
    // not show (the masks, the global look while a mask is chosen),
    // so one left behind on the state just moved from made the next
    // read of the panel news to the history, and its save cut the
    // rows above the one clicked.
    let develop = !same_develop(&edit, &st.develop.edit);
    st.develop.edit = edit;
    crate::panel::original::off(st, app, develop);
    if !develop || !crate::panel::edit::develops_here(st) {
        app.window().request_redraw();
    } else {
        st.develop.generation += 1;
        // A preset or a sync's own words, left for an earlier
        // generation's develop to carry (`status_after_develop`): that
        // generation is superseded now and will not land, so the
        // words it was holding for would otherwise sit unread.
        st.develop.status_after_develop = None;
        app.set_status(tr!("developing...").into());
        app.set_busy(true);
        worker.send(Job::Develop {
            edit: edit_to_develop(app, &st.develop.edit),
            generation: st.develop.generation,
            turn: current_turn(st),
            of: crate::panel::edit::develop_of(st),
        });
    }
}

/// Whether `a` and `b` develop the same picture, so far as the window
/// can tell without the frame: the sections a develop is made from, and
/// which Detail bands the masks ask for. The rest (the light, a look,
/// a mask's amounts) is the finish's. The worker's own test is the
/// frame's list; this one only saves asking for a develop that would
/// change nothing.
fn same_develop(a: &Edit, b: &Edit) -> bool {
    a.white_balance == b.white_balance
        && a.noise == b.noise
        && a.demosaic == b.demosaic
        && a.highlight_fill == b.highlight_fill
        && a.camera == b.camera
        && a.lens == b.lens
        && a.retouch == b.retouch
        && a.detail == b.detail
        && a.sharpen == b.sharpen
        && a.detail_wants() == b.detail_wants()
}

/// The mark an undo (`back`) or a redo out of culling would step, as
/// where it is (`None` the latest on its side), and when it was last
/// acted on. In the library, or with no frame open, the session's
/// latest, on whichever frame. In Develop only the open frame's
/// ([`tags::History::reach`]): its file's latest, or the one that
/// carried the window off its frame onto it; the marks on other
/// frames are passed over. A mark over several frames goes whole,
/// unless a later mark passed over names one of them; then only the
/// open frame's share goes, and the rest waits for Cull.
///
/// [`tags::History::reach`]: crate::tags::History::reach
fn mark_to_step(
    st: &State,
    app: &App,
    back: bool,
) -> Option<(Option<crate::tags::Reach>, greycard_edit::Order)> {
    let open = st.current.and_then(|c| st.path_at(c));
    let Some(file) = open.filter(|_| !app.get_grid_open()) else {
        let order = if back {
            st.culling.tags.undo_order()
        } else {
            st.culling.tags.redo_order()
        }?;
        return Some((None, order));
    };
    let reach = st.culling.tags.reach(back, file)?;
    let (order, _) = st.culling.tags.at(back, reach.at)?;
    Some((Some(reach), order))
}

/// The mark an undo (`back`) or a redo out of culling steps rather
/// than the current frame's edit, when the mark was done last
/// ([`tags::marks_first`]); with no frame current, whenever there is
/// one to step. Which mark: [`mark_to_step`].
///
/// [`tags::marks_first`]: crate::tags::marks_first
pub(crate) fn marks_first(st: &State, app: &App, back: bool) -> Option<Option<crate::tags::Reach>> {
    let line = st.current.and_then(|c| st.line(c));
    let mark = mark_to_step(st, app, back);
    let edit = if back {
        line.filter(|l| !l.history.is_empty())
            .map(|l| l.current_order)
    } else {
        line.and_then(|l| l.redo.last()).map(|s| s.order)
    };
    let order = mark.as_ref().map(|(_, o)| *o);
    crate::tags::marks_first(order, edit).then(|| mark.and_then(|(at, _)| at))
}

/// An undo (`back`) or redo out of culling, in its turn: the panel
/// recorded, then the mark or the edit, whichever [`marks_first`]
/// says. The row to select and the word, from a mark's step.
fn step_last(
    st: &mut State,
    app: &App,
    worker: &Worker,
    back: bool,
) -> (Option<usize>, Option<String>) {
    if st.current.is_some() {
        record_panel(st, app);
    }
    if let Some(at) = marks_first(st, app, back) {
        return crate::panel::browser::step_tags_at(st, app, back, at);
    }
    step_edit(st, app, worker, back);
    (None, None)
}

/// The current frame's edit a step back (`back`) or forward.
fn step_edit(st: &mut State, app: &App, worker: &Worker, back: bool) {
    let Some(line) = st.current.and_then(|c| st.line_mut(c)) else {
        return;
    };
    let moved = if back { line.undo() } else { line.redo() };
    if !moved {
        return;
    }
    take_current(st, app, worker);
}

/// An undo (`back`) or redo out of culling that waited its turn, the
/// guards asked again as a press now would ask them: the window may
/// have moved under it while it waited (a sheet opened, Cull turned
/// on, the queue's view opened). Under a sheet or in the queue's view
/// it is dropped, as a press there is; in Cull it is Cull's, the marks
/// alone.
fn step_held(state: &Rc<Shared>, app: &App, worker: &Worker, back: bool) {
    if app.invoke_sheet_open() {
        return;
    }
    let mut st = state.borrow_mut();
    if crate::panel::cull::cull_mode(&st) {
        // As the press in Cull: nothing with no frame current.
        if st.current.is_none() {
            return;
        }
        let (next, _) = crate::panel::browser::step_tags(&mut st, app, back);
        drop(st);
        if let Some(row) = next {
            app.invoke_select(row as i32);
        }
        return;
    }
    if crate::panel::queue_view::refused(&st, if back { "an undo" } else { "a redo" }) {
        return;
    }
    let (next, said) = step_last(&mut st, app, worker, back);
    drop(st);
    show_marks_step(state, app, next, said);
}

/// After a mark's step out of culling: over to the frame it put back,
/// and its word on the status line, there being none over the picture
/// there; held for the develop the move asked for, as a key's word is.
fn show_marks_step(state: &Rc<Shared>, app: &App, next: Option<usize>, said: Option<String>) {
    let generation = state.borrow().develop.generation;
    if let Some(row) = next {
        app.invoke_select(row as i32);
    }
    if let Some(said) = said {
        let mut st = state.borrow_mut();
        if st.develop.generation != generation {
            st.develop.status_after_develop = Some((st.develop.generation, said.clone()));
        }
        app.set_status(said.into());
    }
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    // Undo and redo walk the sidecar's history; the keys (`mixed`)
    // the session's marks too, when one of those was the last thing
    // done. The history panel's buttons are the edit's alone, as their
    // being greyed out says.
    for (back, mixed) in [(true, true), (false, true), (true, false), (false, false)] {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        let step = move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // The keys' undo with a Fill's strokes painted takes the
            // last of them back, and nothing of the edit: they are in
            // neither it nor its history. The history panel's buttons
            // are the edit's alone.
            if back && mixed && crate::panel::painted::undo_last(&mut st, &app) {
                return;
            }
            // In culling the keys are the ratings', flags' and
            // labels': undo steps back the session's changes to those
            // and touches no develop, and culling stays up. A request
            // like a key's, with nothing to read, so it queues behind
            // the keys still waiting on their reads and takes back the
            // last key the user pressed, not the last that landed. The
            // same in Cull drawn developed, whose keys are culling's.
            if crate::panel::cull::cull_mode(&st) {
                if st.current.is_none() {
                    return;
                }
                drop(st);
                crate::rows::request(
                    &state,
                    &app,
                    &worker,
                    Vec::new(),
                    crate::rows::Scope::File,
                    if back {
                        crate::rows::Request::Undo
                    } else {
                        crate::rows::Request::Redo
                    },
                    Box::new(move |state, app, _, _| {
                        let (next, _) =
                            crate::panel::browser::step_tags(&mut state.borrow_mut(), app, back);
                        if let Some(row) = next {
                            app.invoke_select(row as i32);
                        }
                    }),
                );
                return;
            }
            if crate::panel::queue_view::refused(&st, if back { "an undo" } else { "a redo" }) {
                return;
            }
            // Whatever the panel holds is a state first.
            if st.current.is_some() {
                record_panel(&mut st, &app);
            }
            // Out of culling a mark is undone too when it was the
            // last thing done (§117's marks beside the frame's own
            // line; in Develop the open frame's, `mark_to_step`), the
            // frame it went on brought back if
            // the filter hid it. Queued as culling's undo is, behind a
            // key still waiting on its read, and decided when its turn
            // comes. Behind a pick's open too, once there are marks to
            // weigh: the pick is current at once but its line is a
            // stand-in until the read lands, and the undo is the
            // frame's own. An edit's undo with no mark in the way is as
            // it always was, at once.
            if mixed
                && (marks_first(&st, &app, back).is_some()
                    || st.views.loads.marks_waiting()
                    || (st.culling.tags.any() && st.views.loads.open_waiting()))
            {
                drop(st);
                crate::rows::request(
                    &state,
                    &app,
                    &worker,
                    Vec::new(),
                    crate::rows::Scope::File,
                    if back {
                        crate::rows::Request::Undo
                    } else {
                        crate::rows::Request::Redo
                    },
                    Box::new(move |state, app, worker, _| step_held(state, app, worker, back)),
                );
                return;
            }
            step_edit(&mut st, &app, &worker, back);
        };
        match (back, mixed) {
            (true, true) => app.on_undo(step),
            (false, true) => app.on_redo(step),
            (true, false) => app.on_undo_edit(step),
            (false, false) => app.on_redo_edit(step),
        }
    }
    // The history's rows: a click makes that state current, a hover
    // shows it.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_history_clicked(move |row| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if crate::panel::queue_view::refused(&st, "a history step") {
                return;
            }
            let Some(c) = st.current else {
                return;
            };
            let Some(index) = st.line(c).and_then(|l| l.state_at_row(row)) else {
                return;
            };
            // In culling the panel is not the frame's: the sidecar
            // goes to the row on its own, and the leaving develops it.
            if st.culling.cull.is_some() {
                st.develop.status_kept = None;
                if st.line_mut(c).is_some_and(|l| l.go_to(index)) {
                    leave_cull(&mut st, &app, &worker, None);
                }
                return;
            }
            let Some(line) = st.line(c) else {
                return;
            };
            let target = line.state(index).cloned();
            // Whatever the panel holds is a state first. If that was
            // news to the history, what was undone is gone, and an
            // undone row's state follows the panel's as a step.
            let position = line.position();
            let dirty = record_panel(&mut st, &app);
            let Some(line) = st.line_mut(c) else {
                return;
            };
            let moved = if dirty && index > position {
                // Named by what moved, not by the words it had: it
                // follows the panel's state now, not the one the
                // preset or sync was laid over.
                target.is_some_and(|t| line.record(t))
            } else {
                line.go_to(index)
            };
            st.develop.status_kept = None;
            if !moved {
                return;
            }
            take_current(&mut st, &app, &worker);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_history_hovered(move |row| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(sidecar) = st.current.and_then(|c| st.line(c)) else {
                return;
            };
            let hovered = sidecar.row(row).and_then(|r| {
                let name = app.get_history_rows().row_data(row as usize)?.name;
                Some((sidecar.state(r.state())?.clone(), r, name))
            });
            match hovered {
                Some((edit, Row::Exported { state, export }, _)) => {
                    let e = &sidecar.exports(state)[export];
                    let preset = match &e.preset {
                        Some(p) => tr!(" with the export preset {name}", name = p),
                        None => String::new(),
                    };
                    let what = tr!(
                        "{file}, exported {when}{preset}; click to go back to the state it was \
                         exported from",
                        file = e.file,
                        when = local_date_of(e.at),
                        preset = preset
                    );
                    peek(&mut st, &app, Some(edit), &what);
                }
                Some((edit, Row::State(i), name)) => {
                    let what = if i == sidecar.position() {
                        tr!("the current state: {name}", name = name)
                    } else {
                        tr!(
                            "step {i}: {name}; click to make it current",
                            i = i,
                            name = name
                        )
                    };
                    peek(&mut st, &app, Some(edit), &what);
                }
                None => peek(&mut st, &app, None, ""),
            }
        });
    }
    // The snapshots: taken from the current state, restored as a
    // step, shown on hover, renamed, removed.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_snapshot_taken(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if crate::panel::queue_view::refused(&st, "a snapshot") {
                return;
            }
            let Some(c) = st.current else {
                return;
            };
            // A sidecar that stands in takes no snapshot: it could not
            // be written, and the read's landing would replace it.
            if let Some(why) = crate::rows::not_kept(&st, c) {
                app.set_status(why.into());
                return;
            }
            // In culling the panel is not the frame's: the snapshot
            // is of the sidecar's current state as it stands.
            if st.culling.cull.is_none() {
                record_panel(&mut st, &app);
            }
            let Some(line) = st.line_mut(c) else {
                return;
            };
            let name = format!("Snapshot {}", line.snapshots.len() + 1);
            let i = line.take_snapshot(name.clone(), now());
            write_sidecar(&mut st, c);
            show_history(&st, &app);
            app.set_snapshot_editing(i as i32);
            app.set_status(tr!("{name} taken; type a name for it", name = name).into());
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_snapshot_restored(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if crate::panel::queue_view::refused(&st, "a snapshot") {
                return;
            }
            let Some(c) = st.current else {
                return;
            };
            let Some(name) = st
                .line(c)
                .and_then(|l| l.snapshots.get(i as usize))
                .map(|s| s.name.clone())
            else {
                return;
            };
            // In culling the panel is not the frame's: nothing is
            // recorded from it, and the leaving develops the restore.
            if st.culling.cull.is_some() {
                st.develop.status_kept = None;
                if st
                    .line_mut(c)
                    .is_some_and(|l| l.restore_snapshot(i as usize))
                {
                    leave_cull(&mut st, &app, &worker, None);
                    app.set_status(tr!("{name} restored", name = name).into());
                }
                return;
            }
            record_panel(&mut st, &app);
            st.develop.status_kept = None;
            if !st
                .line_mut(c)
                .is_some_and(|l| l.restore_snapshot(i as usize))
            {
                // The sidecar is there already, but a panel still
                // showing the command line's overrides is not: it
                // takes the snapshot's state all the same.
                if st.launch.overridden.is_some() {
                    app.set_status(tr!("{name} restored", name = name).into());
                    take_current(&mut st, &app, &worker);
                } else {
                    app.set_status(tr!("{name} is the current state", name = name).into());
                }
                return;
            }
            app.set_status(tr!("{name} restored", name = name).into());
            take_current(&mut st, &app, &worker);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_snapshot_hovered(move |i, part| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(c) = st.current else {
                return;
            };
            let snapshot = usize::try_from(i)
                .ok()
                .and_then(|i| st.line(c)?.snapshots.get(i))
                .cloned();
            match snapshot {
                Some(s) => {
                    let what = snapshot_hint(&s.name, &date_of(s.taken), &part);
                    peek(&mut st, &app, Some(s.edit), &what);
                }
                None => peek(&mut st, &app, None, ""),
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_snapshot_renamed(move |i, name| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if crate::panel::queue_view::refused(&st, "a snapshot") {
                return;
            }
            let Some(c) = st.current else {
                return;
            };
            let name = name.trim();
            if name.is_empty() {
                return;
            }
            let Some(s) = st.line_mut(c).and_then(|l| l.snapshots.get_mut(i as usize)) else {
                return;
            };
            s.name = name.to_string();
            write_sidecar(&mut st, c);
            show_history(&st, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_snapshot_deleted(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if crate::panel::queue_view::refused(&st, "a snapshot") {
                return;
            }
            let Some(c) = st.current else {
                return;
            };
            let Some(line) = st.line_mut(c) else {
                return;
            };
            if (i as usize) >= line.snapshots.len() {
                return;
            }
            let removed = line.snapshots.remove(i as usize);
            peek(&mut st, &app, None, "");
            write_sidecar(&mut st, c);
            show_history(&st, &app);
            app.set_status(tr!("{name} removed", name = removed.name).into());
        });
    }
}

#[cfg(test)]
mod tests;
