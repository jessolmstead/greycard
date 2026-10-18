use crate::panel::browser::file_name;
use crate::panel::cull::leave_cull;
use crate::panel::edit::{
    current_turn, edit_to_develop, read_edit, record_panel, show_edit, write_file, write_sidecar,
};
use crate::*;
use greycard_edit::{Exported, Row};
use std::path::Path;

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
        return;
    };
    // Which version the history is of, after its title: none for the
    // file's own.
    app.set_history_version(st.version_name(c).unwrap_or_default().into());
    app.set_can_undo(!sidecar.history.is_empty());
    app.set_can_redo(!sidecar.redo.is_empty());
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
        // field alone is left out of the comparison.
        None if {
            let mut plain = state.clone();
            plain.noise.learned_strength = Edit::default().noise.learned_strength;
            plain == Edit::default() || *state == Edit::for_picture()
        } =>
        {
            "Original".to_string()
        }
        None => "Earliest kept".to_string(),
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

fn not_kept(source: &Path) {
    tracing::info!(
        "{}: exported under an edit that is no state of its history \
         (undone and replaced since, or the command line's); not recorded",
        file_name(source)
    );
}

/// The version the export was of is no longer in the sidecar (deleted,
/// or taken out by a join since the set was queued): nothing to record
/// it on.
fn no_version(source: &Path, version: Option<&str>) {
    tracing::warn!(
        "{}: no version {}; export not recorded",
        file_name(source),
        version.unwrap_or("(the file's own)")
    );
}

/// [`record_export`] for a frame no window holds: its sidecar read
/// from where it is and brought up to date as an open brings it, the
/// export noted on the state that is `edit`, and written back where
/// `placement` says, the frame's `.xmp` with it when `xmp` asks, as
/// every sidecar write of the window's does (`write_sidecar`); true
/// when it was written. The window's for a frame it has let go of,
/// and the headless `--export`'s for every frame.
pub(crate) fn record_export_on_disk(
    source: &Path,
    edit: &Edit,
    version: Option<&str>,
    exported: Exported,
    placement: greycard_edit::Placement,
    xmp: bool,
) -> bool {
    // Read as an open reads it (`load_sidecar_said`): the `.gcd`, a
    // picture's own default where there is none, and what an `.xmp`
    // beside the frame says that the sidecar has not taken yet. Read
    // without the `.xmp`, the write below would put the `.gcd`'s old
    // rating and turn over whatever another tool wrote there since.
    let (mut sidecar, trouble) = crate::panel::browser::load_sidecar_said(source, true);
    if let Some(crate::rows::Trouble::Unreadable(e)) = trouble {
        tracing::warn!("{}: sidecar: {e}; export not recorded", file_name(source));
        return false;
    }
    // The edit was rendered from the sidecar as an open leaves it, so
    // an older build's is brought up to date the same way before the
    // two are compared; otherwise a version 3 Original crop is never
    // the state its own export was made from.
    crate::files::migrate_from_file(&mut sidecar, source);
    // The same seed for a frame the window has let go of, by the rule
    // a load decides it by.
    take_seed(&mut sidecar, source, version, edit);
    let Some(line) = sidecar.line_mut(version) else {
        no_version(source, version);
        return false;
    };
    if !line.record_export(edit, exported) {
        not_kept(source);
        return false;
    }
    if xmp {
        // As `write_sidecar` writes it: the camera's tag and the
        // frame's turns composed, or no orientation when the file
        // will not say its tag.
        let turn = greycard_core::decode::stance_path(source)
            .ok()
            .map(|s| xmp::Turn::new(s.orientation, sidecar.turn));
        let taken = || crate::panel::time_shift::camera_time_of(source);
        match xmp::save_dated(source, &sidecar.meta, turn, taken) {
            Ok(Some(mark)) => sidecar.xmp = Some(mark),
            Ok(None) => {}
            Err(e) => tracing::warn!("{}: xmp not written: {e}", file_name(source)),
        }
    }
    match sidecar.save_in(source, placement) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("{}: sidecar not saved: {e}", file_name(source));
            false
        }
    }
}

/// Give a frame still waiting on its ISO seed the learned blend its
/// export was rendered with (the worker seeds a set's frame as its
/// first open would), in place and not as a step, as the open's own
/// seed is taken (`Outcome::Opened`); true when it took it. Nothing
/// when the two differ in anything else: then the export was not of
/// this state, and the seed waits for the open.
fn take_seed(sidecar: &mut Sidecar, source: &Path, version: Option<&str>, rendered: &Edit) -> bool {
    if greycard_core::picture::is_picture_path(source) {
        return false;
    }
    let Some(line) = sidecar.line_mut(version) else {
        return false;
    };
    if !crate::files::line_never_developed(line) {
        return false;
    }
    let mut seeded = line.current.clone();
    seeded.noise.learned_strength = rendered.noise.learned_strength;
    if seeded != *rendered {
        return false;
    }
    line.current = seeded;
    true
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
                format!("; the {} shown once restored", unseen.join(" and "))
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
        "restore" => format!("{name}, {taken}; click to restore it"),
        "rename" => format!("{name}, {taken}; click to rename it"),
        "remove" => format!("{name}, {taken}; click to remove it"),
        _ => format!("{name}, {taken}; click to restore, double-click to rename"),
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
    if st.current.is_none() {
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
    let develop = !edit.same_develop(&st.develop.edit);
    st.develop.edit = edit;
    if !develop {
        app.window().request_redraw();
    } else {
        st.develop.generation += 1;
        // A preset or a sync's own words, left for an earlier
        // generation's develop to carry (`status_after_develop`): that
        // generation is superseded now and will not land, so the
        // words it was holding for would otherwise sit unread.
        st.develop.status_after_develop = None;
        app.set_status("developing...".into());
        app.set_busy(true);
        worker.send(Job::Develop {
            edit: edit_to_develop(app, &st.develop.edit),
            generation: st.develop.generation,
            turn: current_turn(st),
        });
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // Undo and redo walk the sidecar's history.
    for back in [true, false] {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        let step = move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(c) = st.current else {
                return;
            };
            // In culling the keys are the ratings', flags' and
            // labels': undo steps back the session's changes to those
            // and touches no develop, and culling stays up. A request
            // like a key's, with nothing to read, so it queues behind
            // the keys still waiting on their reads and takes back the
            // last key the user pressed, not the last that landed.
            if st.culling.cull.is_some() {
                drop(st);
                crate::rows::request(
                    &state,
                    &app,
                    &worker,
                    Vec::new(),
                    crate::rows::Scope::File,
                    if back { "undo" } else { "redo" },
                    Box::new(move |state, app, _, _| {
                        let next =
                            crate::panel::browser::step_tags(&mut state.borrow_mut(), app, back);
                        if let Some(row) = next {
                            app.invoke_select(row as i32);
                        }
                    }),
                );
                return;
            }
            // Whatever the panel holds is a state first.
            record_panel(&mut st, &app);
            let Some(line) = st.line_mut(c) else {
                return;
            };
            let moved = if back { line.undo() } else { line.redo() };
            if !moved {
                return;
            }
            take_current(&mut st, &app, &worker);
        };
        if back {
            app.on_undo(step);
        } else {
            app.on_redo(step);
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
                        Some(p) => format!(" with the export preset {p}"),
                        None => String::new(),
                    };
                    let what = format!(
                        "{}, exported {}{preset}; click to go back to the state it was \
                         exported from",
                        e.file,
                        local_date_of(e.at)
                    );
                    peek(&mut st, &app, Some(edit), &what);
                }
                Some((edit, Row::State(i), name)) => {
                    let what = if i == sidecar.position() {
                        format!("the current state: {name}")
                    } else {
                        format!("step {i}: {name}; click to make it current")
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
            app.set_status(format!("{name} taken; type a name for it").into());
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_snapshot_restored(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
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
                    app.set_status(format!("{name} restored").into());
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
                    app.set_status(format!("{name} restored").into());
                    take_current(&mut st, &app, &worker);
                } else {
                    app.set_status(format!("{name} is the current state").into());
                }
                return;
            }
            app.set_status(format!("{name} restored").into());
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
            app.set_status(format!("{} removed", removed.name).into());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame exported as a set of one, as the window sends every
    /// export: what lands is that frame's `Done::Exported`.
    fn landed(
        source: &std::path::Path,
        edit: &Edit,
        path: PathBuf,
        preset: Option<String>,
    ) -> Outcome {
        let set = crate::queue::Set::new(
            1,
            None,
            crate::export::Settings::default(),
            crate::export::OnExists::Increment,
        )
        .with_preset(preset);
        Outcome::SetFrameDone {
            set: Arc::new(set),
            index: 0,
            source: source.to_path_buf(),
            edit: edit.clone(),
            version: None,
            done: crate::queue::Done::Exported {
                path,
                seconds: 1.0,
                note: None,
                left_out: Vec::new(),
            },
        }
    }

    #[test]
    fn a_moment_reads_as_a_date() {
        assert_eq!(date_of(0), "1970-01-01 00:00 UTC");
        assert_eq!(date_of(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(date_of(1_789_000_000), "2026-09-10 00:26 UTC");
    }

    /// An export's moment in local time: the date and time on hover,
    /// the time alone on its row when it was today, the date when not.
    #[test]
    fn an_export_s_moment_reads_in_local_time() {
        let at = 1_789_000_000;
        let whole = local_date_of(at);
        assert_eq!(whole.len(), "2026-09-10 00:26".len(), "{whole}");
        assert!(!whole.contains("UTC"));
        assert_eq!(row_time(at, at), whole[11..]);
        assert_eq!(row_time(at, at + 3 * 86_400), whole[..10]);
    }

    /// The panel's edit made a state, as the save timer (or the
    /// export button) makes it.
    fn save(app: &App, state: &Rc<RefCell<State>>) {
        let mut st = state.borrow_mut();
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(&mut st, edit);
        show_history(&st, app);
    }

    fn rows(app: &App) -> Vec<(String, bool)> {
        let rows = app.get_history_rows();
        (0..rows.row_count())
            .map(|r| {
                let row = rows.row_data(r).unwrap();
                (row.name.to_string(), row.exported)
            })
            .collect()
    }

    /// Which rows are drawn as undone, newest first.
    fn undone(app: &App) -> Vec<bool> {
        let rows = app.get_history_rows();
        (0..rows.row_count())
            .map(|r| rows.row_data(r).unwrap().undone)
            .collect()
    }

    /// The two marks a frame's cell wears, as the strip and the grid
    /// are given them.
    fn marks(app: &App, row: usize) -> (bool, bool) {
        let t = app.get_thumbs().row_data(row).unwrap();
        (t.edited, t.exported)
    }

    /// A saved edit puts the pencil on the frame's cell, a reset takes
    /// it off again whatever history it leaves, and a frame never
    /// touched wears nothing.
    #[test]
    fn a_saved_edit_shows_its_mark_and_a_reset_hides_it() {
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
        app.invoke_select(0);
        assert_eq!(marks(&app, 0), (false, false));
        assert_eq!(crate::testing::count_labeled(&app, "Edited"), 0);

        app.set_exposure(0.5);
        save(&app, &state);
        assert_eq!(marks(&app, 0), (true, false));
        assert_eq!(marks(&app, 1), (false, false), "the other frame is bare");
        assert_eq!(
            crate::testing::count_labeled(&app, "Edited"),
            1,
            "drawn once, on the one cell"
        );
        assert_eq!(crate::testing::count_labeled(&app, "Exported"), 0);

        // Reset: the panel back to the default is a state of its own,
        // and the frame is no longer edited.
        app.set_exposure(0.0);
        save(&app, &state);
        assert!(!state.borrow().sidecars[0].history.is_empty());
        assert_eq!(marks(&app, 0), (false, false));
        assert_eq!(crate::testing::count_labeled(&app, "Edited"), 0);
    }

    /// An export landing puts its mark on the cell, beside the pencil
    /// when the frame is edited as well, and one that failed puts
    /// none.
    #[test]
    fn an_export_landing_shows_its_mark() {
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
        app.invoke_select(0);
        let source = state.borrow().files[0].clone();
        let set = Arc::new(crate::queue::Set::new(
            1,
            None,
            crate::export::Settings::default(),
            crate::export::OnExists::Increment,
        ));
        crate::panel::deliver::deliver(
            &app,
            Outcome::SetFrameDone {
                set,
                index: 0,
                source: source.clone(),
                edit: Edit::default(),
                version: None,
                done: crate::queue::Done::Failed {
                    message: "disk full".into(),
                },
            },
        );
        assert_eq!(marks(&app, 0), (false, false));

        // An unedited frame exported: the export mark alone.
        let plain = state.borrow().sidecars[0].current.clone();
        crate::panel::deliver::deliver(
            &app,
            landed(&source, &plain, PathBuf::from("/out/IMG_0000.jpg"), None),
        );
        assert_eq!(marks(&app, 0), (false, true));
        assert_eq!(marks(&app, 1), (false, false));
        assert_eq!(crate::testing::count_labeled(&app, "Exported"), 1);
        assert_eq!(crate::testing::count_labeled(&app, "Edited"), 0);

        // Edited as well: both, together.
        app.set_exposure(0.5);
        save(&app, &state);
        assert_eq!(marks(&app, 0), (true, true));
        assert_eq!(crate::testing::count_labeled(&app, "Exported"), 1);
        assert_eq!(crate::testing::count_labeled(&app, "Edited"), 1);
    }

    /// The export mark is a fact about the frame: an undo back past the
    /// exported state leaves it, and so do more steps than the history
    /// keeps.
    #[test]
    fn the_export_mark_outlives_the_state_it_was_made_from() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        let source = state.borrow().files[0].clone();
        app.set_exposure(0.5);
        save(&app, &state);
        let sent = state.borrow().sidecars[0].current.clone();
        crate::panel::deliver::deliver(
            &app,
            landed(&source, &sent, PathBuf::from("/out/IMG_0000.jpg"), None),
        );
        assert_eq!(marks(&app, 0), (true, true));

        // Back past the exported state: the pencil goes, the export stays.
        {
            let st = &mut *state.borrow_mut();
            assert!(st.sidecars[0].undo());
            show_history(st, &app);
        }
        assert_eq!(marks(&app, 0), (false, true));

        // More steps than the history keeps: the record is off it, the
        // mark is not.
        for k in 0..51 {
            app.set_exposure(1.0 + k as f32 / 100.0);
            save(&app, &state);
        }
        assert!(
            state.borrow().sidecars[0]
                .history
                .iter()
                .all(|s| s.exports.is_empty())
        );
        assert_eq!(marks(&app, 0), (true, true));
    }

    /// An export that finishes after the panel moved on is recorded on
    /// the state it was written from, as a row of its own marked as
    /// one; a click on it goes back to that look; undo walks the
    /// changes as before; a failed export records nothing.
    #[test]
    fn an_export_is_a_row_on_the_state_it_was_written_from() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        let source = state.borrow().files[0].clone();

        // Exported at +0.5, and the panel moved on to +1.0 while the
        // file was written.
        app.set_exposure(0.5);
        save(&app, &state);
        let sent = state.borrow().sidecars[0].current.clone();
        assert_eq!(sent.light.exposure, 0.5);
        app.set_exposure(1.0);
        save(&app, &state);
        let states = state.borrow().sidecars[0].states();
        crate::panel::deliver::deliver(
            &app,
            landed(
                &source,
                &sent,
                PathBuf::from("/out/IMG_0000.jpg"),
                Some("Web".into()),
            ),
        );
        {
            let st = state.borrow();
            let s = &st.sidecars[0];
            assert_eq!(s.states(), states, "no state for the export");
            assert_eq!(s.current.light.exposure, 1.0, "the panel's moved on");
            assert!(s.current_exports.is_empty());
            let on = s.exports(1);
            assert_eq!(on.len(), 1);
            assert_eq!(on[0].file, "/out/IMG_0000.jpg");
            assert_eq!(on[0].preset.as_deref(), Some("Web"));
            assert!(on[0].at > 0);
        }
        assert_eq!(
            rows(&app),
            [
                ("Exposure +1.00".to_string(), false),
                ("IMG_0000.jpg · Web".to_string(), true),
                ("Exposure +0.50".to_string(), false),
                ("Original".to_string(), false),
            ]
        );
        assert_eq!(app.get_history_current(), 0);
        assert_eq!(undone(&app), [false; 4]);
        assert!(!app.get_history_rows().row_data(1).unwrap().time.is_empty());

        // A set's frame that failed, was skipped or was canceled:
        // nothing.
        let set = Arc::new(crate::queue::Set::new(
            1,
            None,
            crate::export::Settings::default(),
            crate::export::OnExists::Increment,
        ));
        for done in [
            crate::queue::Done::Failed {
                message: "no".into(),
            },
            crate::queue::Done::Skipped {
                path: PathBuf::from("/out/IMG_0000.jpg"),
            },
            crate::queue::Done::Canceled,
        ] {
            crate::panel::deliver::deliver(
                &app,
                Outcome::SetFrameDone {
                    set: set.clone(),
                    index: 0,
                    source: source.clone(),
                    edit: sent.clone(),
                    version: None,
                    done,
                },
            );
        }
        assert_eq!(rows(&app).len(), 4);
        assert_eq!(state.borrow().sidecars[0].exports(1).len(), 1);

        // Hovered, the viewport shows the look that went out.
        app.invoke_history_hovered(1);
        assert_eq!(state.borrow().develop.peek.as_ref(), Some(&sent));
        assert!(app.get_status().contains("/out/IMG_0000.jpg, exported"));
        app.invoke_history_hovered(-1);

        // Clicked: that state is current, the panel and all.
        app.invoke_history_clicked(1);
        assert_eq!(state.borrow().sidecars[0].current, sent);
        assert_eq!(app.get_exposure(), 0.5);
        assert_eq!(app.get_history_current(), 2, "the state's row, lit");
        assert_eq!(state.borrow().sidecars[0].current_exports.len(), 1);
        // The current state's export above it reads as live; only the
        // step undone above them is muted.
        assert_eq!(undone(&app), [true, false, false, false]);

        // One undo is one change: back to the original, not a step
        // for the export; and a redo brings the record back with its
        // state.
        app.invoke_undo();
        assert_eq!(state.borrow().sidecars[0].current, Edit::default());
        // The export's state undone: its row is muted with it.
        assert_eq!(undone(&app), [true, true, true, false]);
        app.invoke_redo();
        assert_eq!(state.borrow().sidecars[0].current, sent);
        assert_eq!(state.borrow().sidecars[0].current_exports.len(), 1);
        app.invoke_redo();
        assert_eq!(state.borrow().sidecars[0].current.light.exposure, 1.0);
        assert!(rows(&app)[1].1);
    }

    /// A frame the window let go of while its file was written (another
    /// folder opened): its sidecar is read from the disk, noted, and
    /// written back through the same save, counted; a later export of
    /// a state it no longer has writes nothing.
    #[test]
    fn a_frame_no_longer_shown_is_noted_on_the_disk() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/scratch/export-record-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0100.CR3");
        let mut on_disk = Sidecar::default();
        let mut edit = Edit::default();
        edit.light.exposure = 0.75;
        on_disk.record(edit.clone());
        on_disk.save(&raw).unwrap();

        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        state.borrow_mut().window.write_sidecars = true;
        let exported = |edit: &Edit| landed(&raw, edit, dir.join("IMG_0100.jpg"), None);
        crate::panel::deliver::deliver(&app, exported(&edit));
        let back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back.saved, 2);
        assert_eq!(back.current, edit);
        assert_eq!(back.current_exports.len(), 1);
        assert_eq!(back.current_exports[0].label(), "IMG_0100.jpg");
        // The window's own frames are untouched.
        assert_eq!(state.borrow().sidecars[0], Sidecar::default());

        crate::panel::deliver::deliver(&app, exported(&Edit::for_picture()));
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), back);
        crate::testing::remove_dir_retry(&dir);
    }

    fn scratch(what: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/scratch/export-{what}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn exposed(ev: f32) -> Edit {
        let mut e = Edit::default();
        e.light.exposure = ev;
        e
    }

    /// A shoot of `frames` raws in `dir`, the first with a sidecar at
    /// 0.5 EV carrying one export record, opened in a window as a run
    /// with `--exposure 1.0` (and `--develop-temperature 4000`, when
    /// `temperature`) opens it: the window, the state, the first raw,
    /// and its sidecar's bytes on disk.
    fn launched(
        dir: &Path,
        frames: usize,
        temperature: bool,
    ) -> (App, Rc<RefCell<State>>, Rc<Worker>, PathBuf, Vec<u8>) {
        let raws: Vec<PathBuf> = (0..frames)
            .map(|i| dir.join(format!("IMG_02{i:02}.CR3")))
            .collect();
        let mut kept = Sidecar::default();
        kept.record(exposed(0.5));
        kept.record_export(
            &exposed(0.5),
            Exported {
                file: "/out/earlier.jpg".into(),
                preset: None,
                at: 1,
            },
        );
        kept.save(&raws[0]).unwrap();
        let bytes = std::fs::read(Sidecar::path_for(&raws[0])).unwrap();
        let app = crate::testing::window(frames);
        let (state, worker) = crate::testing::state_for(&app, raws.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.sidecars[0] = Sidecar::load(&raws[0]).unwrap().unwrap();
            st.launch.overrides_at_start = Some((
                0,
                crate::panel::startup::Overrides {
                    temperature: temperature.then_some(4000.0),
                    exposure: Some(1.0),
                    display_curve: None,
                },
            ));
        }
        app.invoke_select(0);
        (app, state, worker, raws[0].clone(), bytes)
    }

    fn on_disk(raw: &Path) -> Sidecar {
        Sidecar::load(raw).unwrap().unwrap()
    }

    /// `--exposure` on a batch export: the command line's value is the
    /// panel's, never the sidecar's. The file is written at it, but it
    /// is no state of the frame's, so nothing is recorded, and the
    /// run's closing save finds nothing to save: the sidecar is left
    /// byte for byte as it was, its 0.5 EV state and that state's
    /// record with it. The first move ends that, one-shot: 1.0, then
    /// 1.5, then 1.0 again is on disk at 1.0.
    #[test]
    fn a_command_line_exposure_leaves_the_sidecar_as_it_was() {
        let dir = scratch("override");
        let half = exposed(0.5);
        let (app, state, _worker, raw, bytes) = launched(&dir, 1, true);
        state.borrow_mut().window.batch = true;
        let rendered = state.borrow().develop.edit.clone();
        assert_eq!(rendered.light.exposure, 1.0, "the panel's");
        assert_eq!(
            state.borrow().sidecars[0].current,
            half,
            "not the sidecar's"
        );

        // The batch export lands, and the run closes with the save it
        // always makes of the open file.
        crate::panel::deliver::deliver(
            &app,
            landed(&raw, &rendered, dir.join("IMG_0200.jpg"), None),
        );
        {
            let mut st = state.borrow_mut();
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            crate::panel::edit::save_edit(&mut st, edit);
        }
        assert!(
            std::fs::read(Sidecar::path_for(&raw)).unwrap() == bytes,
            "rewritten"
        );
        assert_eq!(state.borrow().sidecars[0].states(), 2);
        assert_eq!(state.borrow().sidecars[0].exports(1).len(), 1);

        // Moved on the panel, the edit is a state as any would be, and
        // the overrides are done with: moved back to exactly their
        // values, that is a state too, and it is what is on disk.
        app.set_exposure(1.5);
        save(&app, &state);
        assert_eq!(state.borrow().sidecars[0].states(), 3);
        assert_eq!(on_disk(&raw).current.light.exposure, 1.5);
        app.set_exposure(1.0);
        save(&app, &state);
        assert_eq!(state.borrow().sidecars[0].states(), 4);
        assert_eq!(state.borrow().sidecars[0].current.light.exposure, 1.0);
        assert_eq!(on_disk(&raw).current.light.exposure, 1.0);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Left and come back to, a frame opened under `--exposure` is its
    /// sidecar's again: nothing of the override's was recorded on the
    /// way out, and on the way back an edit that happens to be the
    /// override's values is saved like any other.
    #[test]
    fn a_frame_left_and_come_back_to_is_its_sidecar_s_again() {
        let dir = scratch("override-return");
        let (app, state, _worker, raw, bytes) = launched(&dir, 2, false);
        app.invoke_select(1);
        assert!(std::fs::read(Sidecar::path_for(&raw)).unwrap() == bytes);
        app.invoke_select(0);
        assert_eq!(app.get_exposure(), 0.5, "the sidecar's, not the override's");
        assert_eq!(state.borrow().develop.edit, exposed(0.5));
        app.set_exposure(1.0);
        save(&app, &state);
        assert_eq!(state.borrow().sidecars[0].states(), 3);
        assert_eq!(on_disk(&raw).current, exposed(1.0));
        crate::testing::remove_dir_retry(&dir);
    }

    /// Undo as the first thing done after an `--exposure` launch goes
    /// back from the sidecar's own current state: the override is not
    /// recorded on the way, so the redo stack the sidecar carried is
    /// still there to walk, and redo lands on the sidecar's states.
    #[test]
    fn a_first_undo_under_an_override_records_nothing_and_keeps_the_redo() {
        let dir = scratch("override-undo");
        let (app, state, _worker, _raw, _) = launched(&dir, 1, false);
        // The sidecar as a session left it: 0.75 undone, back at 0.5,
        // and the launch's override on the panel at 1.0.
        {
            let mut st = state.borrow_mut();
            let s = &mut st.sidecars[0];
            s.record(exposed(0.75));
            s.undo();
            assert_eq!(s.current, exposed(0.5));
            assert_eq!(s.redo.len(), 1);
        }
        assert_eq!(app.get_exposure(), 1.0);

        app.invoke_undo();
        {
            let st = state.borrow();
            let s = &st.sidecars[0];
            assert_eq!(s.current, Edit::default());
            assert_eq!(s.states(), 3, "no state for the override");
            assert!(
                (0..s.states()).all(|i| s.state(i) != Some(&exposed(1.0))),
                "the override is no state"
            );
            assert_eq!(s.redo.len(), 2, "the carried redo kept");
            assert!(st.launch.overridden.is_none(), "done with");
        }
        assert_eq!(app.get_exposure(), 0.0);
        app.invoke_redo();
        assert_eq!(state.borrow().sidecars[0].current, exposed(0.5));
        app.invoke_redo();
        assert_eq!(state.borrow().sidecars[0].current, exposed(0.75));
        crate::testing::remove_dir_retry(&dir);
    }

    /// A frame never opened, exported in a set: the worker seeds its
    /// learned blend from the ISO, as a first open would, and the
    /// sidecar its export creates carries that seed as the frame's
    /// current state, not a step; the next load reads it back as the
    /// seed, not the plain default.
    #[test]
    fn an_export_of_a_frame_never_opened_keeps_its_iso_seed() {
        let seed = greycard_edit::Noise::blend_for_iso(Some(200));
        assert!(seed < 1.0);
        let mut rendered = Edit::default();
        rendered.noise.learned_strength = seed;

        // In the window's list, waiting on its seed.
        let dir = scratch("seed");
        let (listed, gone) = (dir.join("IMG_0300.CR3"), dir.join("IMG_0301.CR3"));
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, vec![listed.clone()]);
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            assert!(crate::rows::wants_seed(&st, 0), "a raw never developed");
        }
        let set = Arc::new(crate::queue::Set::new(
            2,
            Some(dir.clone()),
            crate::export::Settings::default(),
            crate::export::OnExists::Increment,
        ));
        for (i, source) in [&listed, &gone].into_iter().enumerate() {
            crate::panel::deliver::deliver(
                &app,
                Outcome::SetFrameDone {
                    set: set.clone(),
                    index: i,
                    source: source.clone(),
                    edit: rendered.clone(),
                    version: None,
                    done: crate::queue::Done::Exported {
                        path: dir.join(format!("{i}.jpg")),
                        seconds: 1.0,
                        note: None,
                        left_out: Vec::new(),
                    },
                },
            );
        }
        assert!(!crate::rows::wants_seed(&state.borrow(), 0), "taken");
        // Both on disk, the one the window had and the one it had not:
        // the seed as current, no step, the record on it.
        for raw in [&listed, &gone] {
            let back = Sidecar::load(raw).unwrap().unwrap();
            assert_eq!(back.current, rendered, "{}", raw.display());
            assert!(back.history.is_empty());
            assert_eq!(back.current_exports.len(), 1);
            let (loaded, seed_again) = crate::panel::browser::load_sidecar(raw, true);
            assert_eq!(loaded.current.noise.learned_strength, seed);
            assert!(!seed_again, "seeded already");
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// A set records one row a frame, each on that frame's own state
    /// under the set's preset; a frame that failed records nothing.
    #[test]
    fn a_set_records_a_row_on_each_frame_it_wrote() {
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        let mut edits = Vec::new();
        {
            let mut st = state.borrow_mut();
            for (i, ev) in [0.25, -0.5, 1.5].into_iter().enumerate() {
                let mut e = Edit::default();
                e.light.exposure = ev;
                st.sidecars[i].record(e.clone());
                edits.push(e);
            }
        }
        let set = Arc::new(
            crate::queue::Set::new(
                3,
                Some(PathBuf::from("/out")),
                crate::export::Settings::default(),
                crate::export::OnExists::Increment,
            )
            .with_preset(Some("Print".into())),
        );
        let files = state.borrow().files.clone();
        for (i, done) in [
            crate::queue::Done::Exported {
                path: PathBuf::from("/out/IMG_0000.jpg"),
                seconds: 1.0,
                note: None,
                left_out: Vec::new(),
            },
            crate::queue::Done::Failed {
                message: "no".into(),
            },
            crate::queue::Done::Exported {
                path: PathBuf::from("/out/IMG_0002.jpg"),
                seconds: 1.0,
                note: None,
                left_out: Vec::new(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            crate::panel::deliver::deliver(
                &app,
                Outcome::SetFrameDone {
                    set: set.clone(),
                    index: i,
                    source: files[i].clone(),
                    edit: edits[i].clone(),
                    version: None,
                    done,
                },
            );
        }
        let st = state.borrow();
        for (i, want) in [Some("/out/IMG_0000.jpg"), None, Some("/out/IMG_0002.jpg")]
            .into_iter()
            .enumerate()
        {
            let s = &st.sidecars[i];
            assert_eq!(s.states(), 2, "frame {i}");
            match want {
                Some(file) => {
                    assert_eq!(s.current_exports.len(), 1, "frame {i}");
                    assert_eq!(s.current_exports[0].file, file);
                    assert_eq!(s.current_exports[0].preset.as_deref(), Some("Print"));
                    assert_eq!(
                        s.current_exports[0].label(),
                        format!("{} · Print", &file[5..])
                    );
                }
                None => assert!(s.current_exports.is_empty(), "frame {i}"),
            }
        }
    }

    /// A develop landing with `sources` for the state developing now.
    fn land(app: &App, state: &Rc<RefCell<State>>, sources: Vec<greycard_edit::retouch::Patch>) {
        let generation = state.borrow().develop.generation;
        crate::panel::deliver::deliver(
            app,
            crate::worker::Outcome::Developed {
                generation,
                turn: 0,
                image: crate::worker::Developed::Halves(std::sync::Arc::new(
                    crate::worker::Halves {
                        width: 60,
                        height: 40,
                        pixels: Vec::new(),
                    },
                )),
                guide: std::sync::Arc::new(crate::finish::Guide::NONE),
                planes: None,
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 0.1,
                detail: None,
                sharpen: None,
                dehaze: None,
                sources,
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport::default(),
            },
        );
    }

    /// Issue 7, with the slow develop's order: a clone spot placed,
    /// the panel's save firing before the develop lands, then the
    /// develop landing with the source the engine chose. One undo
    /// takes the spot away, and stays undone when that state's own
    /// develop lands; a redo brings the spot back with its source.
    #[test]
    fn one_undo_takes_a_clone_spot_away_when_its_source_lands_after_the_save() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        assert_eq!(state.borrow().current, Some(0));

        app.invoke_retouch_toggled("Clone".into());
        app.invoke_place_pressed(10.0, 10.0, false);
        app.invoke_place_released();
        let placed = {
            let st = state.borrow();
            assert_eq!(st.develop.edit.retouch.patches.len(), 1);
            assert!(st.develop.edit.retouch.patches[0].source.is_none());
            st.develop.edit.retouch.patches[0].clone()
        };
        let spots = |state: &Rc<RefCell<State>>| {
            state.borrow().sidecars[0]
                .current
                .retouch
                .patches
                .iter()
                .map(|p| p.source)
                .collect::<Vec<_>>()
        };

        // The save timer fires first: the spot is recorded sourceless.
        {
            let mut st = state.borrow_mut();
            let edit = crate::panel::edit::read_edit(&app, &st.develop.edit, st.develop.target);
            crate::panel::edit::save_edit(&mut st, edit);
        }
        let before = state.borrow().sidecars[0].states();
        assert_eq!(spots(&state), vec![None]);

        // The develop lands with the source: it completes that state.
        let source = [0.05, -0.02];
        let developed = greycard_edit::retouch::Patch {
            source: Some(source),
            ..placed
        };
        land(&app, &state, vec![developed]);
        assert_eq!(state.borrow().sidecars[0].states(), before, "no new step");
        assert_eq!(spots(&state), vec![Some(source)]);
        assert_eq!(
            state.borrow().develop.edit.retouch.patches[0].source,
            Some(source)
        );

        // One undo, and the spot is gone.
        app.invoke_undo();
        assert_eq!(spots(&state), vec![]);
        assert!(state.borrow().develop.edit.retouch.patches.is_empty());
        // That state's develop lands, and nothing comes back.
        land(&app, &state, Vec::new());
        assert_eq!(spots(&state), vec![]);
        assert_eq!(state.borrow().sidecars[0].redo.len(), 1, "the redo stays");

        // A redo brings the spot back with its source, and its
        // develop chooses nothing.
        app.invoke_redo();
        assert_eq!(spots(&state), vec![Some(source)]);
        land(&app, &state, Vec::new());
        assert_eq!(state.borrow().sidecars[0].states(), before);
        app.invoke_undo();
        assert_eq!(spots(&state), vec![]);
    }

    /// A spot deleted while its develop is in flight, and another
    /// placed elsewhere that takes its id: the develop landing with
    /// the first spot's source gives the second nothing. Likewise a
    /// sourceless spot resized while its develop is in flight.
    #[test]
    fn a_source_lands_only_on_the_patch_it_was_chosen_for() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        app.invoke_retouch_toggled("Clone".into());
        app.invoke_place_pressed(10.0, 10.0, false);
        app.invoke_place_released();
        let first = state.borrow().develop.edit.retouch.patches[0].clone();
        let developed = greycard_edit::retouch::Patch {
            source: Some([0.05, -0.02]),
            ..first.clone()
        };

        // The save records the first spot; then it is deleted, and
        // another placed elsewhere, before the develop lands.
        {
            let mut st = state.borrow_mut();
            let edit = crate::panel::edit::read_edit(&app, &st.develop.edit, st.develop.target);
            crate::panel::edit::save_edit(&mut st, edit);
        }
        app.set_patch(0);
        app.invoke_delete_patch();
        assert!(state.borrow().develop.edit.retouch.patches.is_empty());
        app.invoke_place_pressed(10.0, 10.0, false);
        app.invoke_place_released();
        {
            let mut st = state.borrow_mut();
            // Wherever the test window puts it, somewhere else.
            st.develop.edit.retouch.patches[0].points = vec![[0.8, 0.2]];
            let second = &st.develop.edit.retouch.patches[0];
            assert_eq!(second.id, first.id, "the id is reused");
            assert_ne!(second.points, first.points);
        }
        let states = state.borrow().sidecars[0].states();
        land(&app, &state, vec![developed.clone()]);
        assert!(
            state.borrow().develop.edit.retouch.patches[0]
                .source
                .is_none()
        );
        // The sidecar's state with the first spot is completed, with
        // that spot's own source, and nothing is recorded.
        assert_eq!(state.borrow().sidecars[0].states(), states);
        assert_eq!(
            state.borrow().sidecars[0].current.retouch.patches,
            vec![developed.clone()]
        );

        // Resized while in flight: the same id and place, but not the
        // spot the develop chose for.
        {
            let mut st = state.borrow_mut();
            st.develop.edit.retouch.patches[0].points = first.points.clone();
            st.develop.edit.retouch.patches[0].radius = first.radius * 2.0;
        }
        land(&app, &state, vec![developed]);
        assert!(
            state.borrow().develop.edit.retouch.patches[0]
                .source
                .is_none()
        );
    }

    /// The rests a slider's timers wait out: the develop's debounce
    /// and the panel's save, fired as the event loop fires them.
    fn rest() {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 1,
        ));
        slint::platform::update_timers_and_animations();
    }

    /// Click the history's row for the state at `index`.
    fn click_state(app: &App, state: &Rc<RefCell<State>>, index: usize) {
        let row = state.borrow().sidecars[0]
            .row_of_state(index)
            .expect("a row for every state");
        app.invoke_history_clicked(row as i32);
    }

    /// The global exposure of every state, oldest first, and where
    /// the current one sits.
    fn walk(state: &Rc<RefCell<State>>) -> (Vec<f32>, usize) {
        let st = state.borrow();
        let s = &st.sidecars[0];
        let all = (0..s.states())
            .map(|i| s.state(i).unwrap().light.exposure)
            .collect();
        (all, s.position())
    }

    /// A click back in the history with a mask chosen, over states
    /// that differ only in what the finish draws (the global look
    /// here): nothing recorded by whatever comes after it, and the
    /// rows above it still there to click forward to. The panel's
    /// edit kept the newest state's global look and masks behind the
    /// chosen mask's look, since that jump needed no develop, so the
    /// next read of the panel was news to the history and its save
    /// cut the rows above.
    #[test]
    fn a_click_back_keeps_the_rows_above_it_through_what_follows() {
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
        app.invoke_select(0);
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        save(&app, &state);
        // Four global steps, each saved as the save timer saves it.
        app.invoke_target_changed(0);
        for ev in [0.2, 0.4, 0.6, 0.8] {
            app.set_exposure(ev);
            app.invoke_view_changed();
            rest();
        }
        let steps = vec![0.0, 0.0, 0.2, 0.4, 0.6, 0.8];
        assert_eq!(walk(&state), (steps.clone(), 5));
        // The mask chosen again, as it was when the rows were clicked.
        app.invoke_target_changed(1);

        // Two back, and the panel reads as that state.
        click_state(&app, &state, 3);
        assert_eq!(walk(&state), (steps.clone(), 3));
        let panel_is_current = |state: &Rc<RefCell<State>>| {
            let st = state.borrow();
            read_edit(&app, &st.develop.edit, st.develop.target) == st.sidecars[0].current
        };
        assert!(
            panel_is_current(&state),
            "the panel reads as the row clicked"
        );
        assert_eq!(undone(&app), [true, true, false, false, false, false]);

        // A develop landing, the timers running out, and nothing moves.
        land(&app, &state, Vec::new());
        rest();
        save(&app, &state);
        assert_eq!(walk(&state), (steps.clone(), 3));

        // Away to the other frame and back.
        app.invoke_select(1);
        app.invoke_select(0);
        rest();
        assert_eq!(walk(&state), (steps.clone(), 3));

        // Forward a row, then straight on to the newest.
        click_state(&app, &state, 4);
        assert_eq!(walk(&state), (steps.clone(), 4));
        assert!(panel_is_current(&state));
        click_state(&app, &state, 5);
        rest();
        assert_eq!(walk(&state), (steps.clone(), 5));

        // Back past the mask's making, two clicks in a row with the
        // timers running out after: the mask goes with the state, and
        // comes back with the one forward.
        click_state(&app, &state, 2);
        click_state(&app, &state, 0);
        rest();
        assert_eq!(walk(&state), (steps.clone(), 0));
        assert!(state.borrow().develop.edit.adjustments.is_empty());
        assert!(panel_is_current(&state));
        click_state(&app, &state, 3);
        rest();
        assert_eq!(walk(&state), (steps.clone(), 3));
        assert_eq!(state.borrow().develop.edit.adjustments.len(), 1);
        assert!(panel_is_current(&state));

        // An undo and a redo go the same way.
        app.invoke_undo();
        rest();
        assert_eq!(walk(&state), (steps.clone(), 2));
        assert!(panel_is_current(&state));
        app.invoke_redo();
        rest();
        assert_eq!(walk(&state), (steps, 3));
    }

    /// A snapshot just taken waits for its name. A click anywhere
    /// else ends the naming and keeps what was typed, and from then on
    /// a click on the row restores it. Left in naming mode, the row
    /// took every later click as a name to edit and nothing could
    /// restore it.
    #[test]
    fn a_click_away_from_a_new_snapshot_s_name_ends_the_naming() {
        use crate::testing;

        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);

        app.invoke_snapshot_taken();
        assert_eq!(
            app.get_snapshot_editing(),
            0,
            "the new row waits for its name"
        );
        testing::type_text(&app, "Keep");
        // The pointer crossing the picture does not end the naming:
        // the picture takes the keys on hover otherwise.
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(750.0, 450.0),
            });
        assert_eq!(app.get_snapshot_editing(), 0, "a hover leaves the naming");
        // A click on the picture, nowhere near the row, ends it.
        testing::click(&app, 750.0, 450.0);
        assert_eq!(app.get_snapshot_editing(), -1, "the naming ended");
        assert_eq!(state.borrow().sidecars[0].snapshots[0].name, "Keep");

        // The row is a button again: a click restores, once the
        // double-click's window has passed.
        let (at, size) = testing::labeled(&app, "Keep");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(
            app.get_snapshot_editing(),
            -1,
            "a single click does not rename"
        );
        past_double_click();
        assert!(
            app.get_status()
                .as_str()
                .contains("Keep is the current state"),
            "{}",
            app.get_status()
        );
    }

    /// The time a click on a snapshot's row waits before it restores:
    /// past the double-click's window, fired as the event loop fires
    /// it.
    fn past_double_click() {
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(600));
        slint::platform::update_timers_and_animations();
    }

    /// A snapshot's row says what a click does, in its icons: the
    /// restore icon restores at once, the rename icon opens the name
    /// and restores nothing, and a double-click on the row renames
    /// without its first click restoring on the way, which it did
    /// when the row restored on every click.
    #[test]
    fn a_snapshot_s_icons_restore_and_rename_and_a_double_click_only_renames() {
        use crate::testing;
        use slint::platform::Key;

        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.invoke_snapshot_taken();
        testing::type_text(&app, "Base");
        testing::press(&app, Key::Return);
        assert_eq!(app.get_snapshot_editing(), -1);
        assert_eq!(state.borrow().sidecars[0].snapshots[0].name, "Base");
        // A step past the snapshot, so a restore has somewhere to go.
        app.set_exposure(1.0);
        app.invoke_view_changed();
        rest();
        let exposure =
            |state: &Rc<RefCell<State>>| state.borrow().sidecars[0].current.light.exposure;
        assert_eq!(exposure(&state), 1.0);
        let middle = |(at, size): (slint::LogicalPosition, slint::LogicalSize)| {
            (at.x + size.width / 2.0, at.y + size.height / 2.0)
        };

        // The pencil: its words on hover, then the name open for
        // typing, and nothing restored, then or later.
        let (x, y) = middle(testing::labeled(&app, "Rename Base"));
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(x, y),
            });
        // The row's hover is a changed handler's, run with the loop.
        slint::platform::update_timers_and_animations();
        assert!(
            app.get_status().as_str().contains("click to rename it"),
            "{}",
            app.get_status()
        );
        testing::click(&app, x, y);
        assert_eq!(app.get_snapshot_editing(), 0, "the name is open");
        past_double_click();
        assert_eq!(exposure(&state), 1.0, "the rename icon restores nothing");
        testing::press(&app, Key::Escape);
        assert_eq!(app.get_snapshot_editing(), -1);

        // A double-click on the row: the name, and no restore.
        let (x, y) = middle(testing::labeled(&app, "Base"));
        testing::click(&app, x, y);
        testing::click(&app, x, y);
        assert_eq!(app.get_snapshot_editing(), 0, "a double-click renames");
        past_double_click();
        assert_eq!(exposure(&state), 1.0, "and its first click did not restore");
        testing::press(&app, Key::Escape);
        assert_eq!(app.get_snapshot_editing(), -1);

        // The restore icon: back to the snapshot at once.
        // The pointer comes to it first, as a hand does, and its
        // words are up before the press.
        let (x, y) = middle(testing::labeled(&app, "Restore Base"));
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(x, y),
            });
        slint::platform::update_timers_and_animations();
        assert!(
            app.get_status().as_str().contains("click to restore it"),
            "{}",
            app.get_status()
        );
        // Resting on the icon, its name comes up at the pointer too;
        // the press takes it down.
        assert_eq!(app.get_tip_text(), "", "no hover text before the rest");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(800));
        slint::platform::update_timers_and_animations();
        assert_eq!(
            app.get_tip_text(),
            "Restore",
            "the icon's word after a rest"
        );
        testing::click(&app, x, y);
        assert_eq!(app.get_tip_text(), "", "the press took the hover text down");
        assert_eq!(exposure(&state), 0.0, "restored");
        assert_eq!(app.get_snapshot_editing(), -1, "and not renamed");
        // The hover's words stand over the status while the pointer
        // stays on the row; away from it, the restore's are there.
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(750.0, 450.0),
            });
        slint::platform::update_timers_and_animations();
        assert!(
            app.get_status().as_str().contains("Base restored"),
            "{}",
            app.get_status()
        );
    }

    /// A frame with a snapshot named `name`, the naming over.
    fn named_snapshot(app: &App, name: &str) {
        app.invoke_snapshot_taken();
        crate::testing::type_text(app, name);
        crate::testing::press(app, slint::platform::Key::Return);
        assert_eq!(app.get_snapshot_editing(), -1);
    }

    /// A restore still waiting out the double-click's window is for
    /// the frame its row was clicked on: another frame chosen in the
    /// meantime drops it, rather than restoring the new frame's
    /// snapshot of that number. Left alone, the wait ends in the
    /// restore.
    #[test]
    fn a_waiting_restore_is_dropped_when_the_frame_changes() {
        use crate::testing;
        use std::cell::Cell;

        let app = testing::window(2);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = testing::state_for(&app, testing::folder(2));
        app.invoke_select(0);
        named_snapshot(&app, "Base");
        let restores = Rc::new(Cell::new(0));
        {
            let restores = restores.clone();
            app.on_snapshot_restored(move |_| restores.set(restores.get() + 1));
        }
        let (at, size) = testing::labeled(&app, "Base");
        let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);

        testing::click(&app, x, y);
        past_double_click();
        assert_eq!(restores.get(), 1, "a click alone restores");

        // Past the double-click's window, so the next click is a
        // first click again.
        past_double_click();
        testing::click(&app, x, y);
        app.invoke_select(1);
        past_double_click();
        past_double_click();
        assert_eq!(restores.get(), 1, "nothing restored on either frame");
    }

    /// The pencil on one row while another row's new name is being
    /// typed leaves the typing alone: opening the other name would
    /// take the text box away without ending its naming, and the
    /// typed name with it.
    #[test]
    fn the_pencil_leaves_a_name_being_typed_on_another_row() {
        use crate::testing;

        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        named_snapshot(&app, "Base");
        app.invoke_snapshot_taken();
        assert_eq!(app.get_snapshot_editing(), 1);
        testing::type_text(&app, "Two");

        let (at, size) = testing::labeled(&app, "Rename Base");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_ne!(app.get_snapshot_editing(), 0, "the other name stays shut");
        if app.get_snapshot_editing() == 1 {
            testing::press(&app, slint::platform::Key::Return);
        }
        assert_eq!(app.get_snapshot_editing(), -1);
        let names: Vec<String> = state.borrow().sidecars[0]
            .snapshots
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(names, ["Base", "Two"], "the typed name kept");
    }

    /// The ISO's seed an export took for a raw's line never opened goes
    /// to that line alone: not to a version at the same default (which
    /// waits for its own open or export), not to one edited apart, and
    /// never to a picture file's.
    #[test]
    fn an_export_s_seed_goes_to_the_line_exported_alone() {
        let raw = Path::new("/shoot/IMG_0001.CR3");
        let mut sidecar = Sidecar::default();
        let copy = sidecar.new_version(None, Some("Copy")).unwrap();
        let apart = sidecar.new_version(None, Some("Apart")).unwrap();
        let mut moved = Edit::default();
        moved.light.exposure = 0.3;
        sidecar.line_mut(Some(&apart)).unwrap().record(moved);
        let mut rendered = Edit::default();
        rendered.noise.learned_strength = 0.42;
        assert!(take_seed(&mut sidecar, raw, None, &rendered));
        assert_eq!(sidecar.current.noise.learned_strength, 0.42);
        let strength =
            |s: &Sidecar, v: &str| s.line(Some(v)).unwrap().current.noise.learned_strength;
        assert_eq!(
            strength(&sidecar, &copy),
            Edit::default().noise.learned_strength,
            "the copy waits for its own"
        );
        assert_ne!(
            strength(&sidecar, &apart),
            0.42,
            "a line edited apart keeps its own"
        );
        // The copy's own export seeds its line, and the file's own stays.
        assert!(take_seed(&mut sidecar, raw, Some(&copy), &rendered));
        assert_eq!(strength(&sidecar, &copy), 0.42);
        // A line edited apart is never seeded, whatever was rendered.
        let mut seeded = sidecar.line(Some(&apart)).unwrap().current.clone();
        seeded.noise.learned_strength = 0.9;
        assert!(
            !take_seed(&mut sidecar, raw, Some(&apart), &seeded),
            "developed already"
        );
        assert_eq!(
            sidecar.current.noise.learned_strength, 0.42,
            "the file's own as it was"
        );
        assert!(
            !take_seed(&mut sidecar, raw, Some("none"), &rendered),
            "no such version"
        );
        let mut picture = Sidecar::default();
        assert!(
            !take_seed(&mut picture, Path::new("/shoot/scan.tif"), None, &rendered),
            "a picture has no ISO to seed from"
        );
    }
}
