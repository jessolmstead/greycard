use crate::panel::browser::file_name;
use crate::panel::cull::leave_cull;
use crate::panel::edit::{
    current_turn, edit_to_develop, read_edit, record_panel, show_edit, write_sidecar,
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
    let Some(c) = st.current else {
        app.set_history_rows(ModelRc::default());
        app.set_history_current(-1);
        app.set_snapshot_names(names(Vec::new()));
        return;
    };
    let sidecar = &st.sidecars[c];
    app.set_can_undo(!sidecar.history.is_empty());
    app.set_can_redo(!sidecar.redo.is_empty());
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
fn state_name(sidecar: &Sidecar, i: usize) -> String {
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
    written: &Path,
    preset: Option<String>,
) {
    let exported = Exported {
        file: written.to_string_lossy().into_owned(),
        preset,
        at: now(),
    };
    let not_kept = || {
        tracing::info!(
            "{}: exported under an edit that is no state of its history \
             (undone and replaced since, or the command line's); not recorded",
            file_name(source)
        );
    };
    if let Some(i) = st.files.iter().position(|f| f == source) {
        // A frame never opened: its blend was seeded for the export
        // as its first open would have, and takes that seed now.
        if st.seed_blend.get(i).copied().unwrap_or(false)
            && take_seed(&mut st.sidecars[i], edit)
            && let Some(seed) = st.seed_blend.get_mut(i)
        {
            *seed = false;
        }
        if !st.sidecars[i].record_export(edit, exported) {
            not_kept();
            return;
        }
        write_sidecar(st, i);
        if st.current == Some(i) {
            show_history(st, app);
        }
        return;
    }
    if !st.write_sidecars {
        return;
    }
    let raw = !greycard_core::picture::is_picture_path(source);
    let mut sidecar = match Sidecar::load(source) {
        Ok(Some(s)) => s,
        Ok(None) if !raw => Sidecar {
            current: Edit::for_picture(),
            ..Sidecar::default()
        },
        Ok(None) => Sidecar::default(),
        Err(e) => {
            tracing::warn!("{}: sidecar: {e}; export not recorded", file_name(source));
            return;
        }
    };
    // The same seed for a frame the window has let go of, by the rule
    // a load decides it by.
    if raw && crate::files::never_developed(&sidecar) {
        take_seed(&mut sidecar, edit);
    }
    if !sidecar.record_export(edit, exported) {
        not_kept();
        return;
    }
    match sidecar.save_in(source, st.placement) {
        Ok(()) => {
            if let Some(indexer) = &st.index {
                indexer.file(source.to_path_buf());
            }
        }
        Err(e) => tracing::warn!("{}: sidecar not saved: {e}", file_name(source)),
    }
}

/// Give a frame still waiting on its ISO seed the learned blend its
/// export was rendered with (the worker seeds a set's frame as its
/// first open would), in place and not as a step, as the open's own
/// seed is taken (`Outcome::Opened`); true when it took it. Nothing
/// when the two differ in anything else: then the export was not of
/// this state, and the seed waits for the open.
fn take_seed(sidecar: &mut Sidecar, rendered: &Edit) -> bool {
    let mut seeded = sidecar.current.clone();
    seeded.noise.learned_strength = rendered.noise.learned_strength;
    if seeded != *rendered {
        return false;
    }
    sidecar.current = seeded;
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
            let panel = read_edit(app, &st.edit, st.target);
            let unseen = finish::not_previewed(&edit, &panel);
            let note = if unseen.is_empty() {
                String::new()
            } else {
                format!("; the {} shown once restored", unseen.join(" and "))
            };
            if st.status_kept.is_none() {
                st.status_kept = Some(app.get_status());
            }
            if st.view_kept.is_none() {
                st.view_kept = Some((st.image_size, st.center));
            }
            app.set_status(format!("{what}{note}").into());
            st.peek = Some(edit);
        }
        None => {
            if st.peek.take().is_some() {
                if let Some(status) = st.status_kept.take() {
                    app.set_status(status);
                }
                if let Some((size, center)) = st.view_kept.take() {
                    st.image_size = size;
                    st.center = center;
                }
            }
        }
    }
    app.window().request_redraw();
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
    let Some(c) = st.current else {
        return;
    };
    // The panel is the sidecar's again: the command line's overrides,
    // if they were on it, are done with (`panel_state`).
    st.overridden = None;
    // In culling the sidecar's current state is what the leaving
    // develops; the history moved, and the mode ends on that.
    if st.cull.is_some() {
        leave_cull(st, app, worker, None);
        return;
    }
    let edit = st.sidecars[c].current.clone();
    if st.write_sidecars && !crate::panel::delete::held(st, c) {
        match st.sidecars[c].save_in(&st.files[c], st.placement) {
            Ok(_) => crate::library::sidecar_written(st, c),
            Err(e) => tracing::warn!("{}: sidecar not saved: {e}", file_name(&st.files[c])),
        }
    }
    if st.target.is_some_and(|i| i >= edit.adjustments.len()) {
        st.target = None;
    }
    show_edit(st, &edit, app, st.target);
    show_history(st, app);
    if edit.same_develop(&st.edit) {
        app.window().request_redraw();
    } else {
        st.edit = edit;
        st.generation += 1;
        // A preset or a sync's own words, left for an earlier
        // generation's develop to carry (`status_after_develop`): that
        // generation is superseded now and will not land, so the
        // words it was holding for would otherwise sit unread.
        st.status_after_develop = None;
        app.set_status("developing...".into());
        app.set_busy(true);
        worker.send(Job::Develop {
            edit: edit_to_develop(app, &st.edit),
            generation: st.generation,
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
            // and touches no develop, and culling stays up.
            if st.cull.is_some() {
                let next = crate::panel::browser::step_tags(&mut st, &app, back);
                drop(st);
                if let Some(row) = next {
                    app.invoke_select(row as i32);
                }
                return;
            }
            // Whatever the panel holds is a state first.
            record_panel(&mut st, &app);
            let moved = if back {
                st.sidecars[c].undo()
            } else {
                st.sidecars[c].redo()
            };
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
            let Some(index) = st.sidecars[c].state_at_row(row) else {
                return;
            };
            // In culling the panel is not the frame's: the sidecar
            // goes to the row on its own, and the leaving develops it.
            if st.cull.is_some() {
                st.status_kept = None;
                if st.sidecars[c].go_to(index) {
                    leave_cull(&mut st, &app, &worker, None);
                }
                return;
            }
            let target = st.sidecars[c].state(index).cloned();
            // Whatever the panel holds is a state first. If that was
            // news to the history, what was undone is gone, and an
            // undone row's state follows the panel's as a step.
            let position = st.sidecars[c].position();
            let dirty = record_panel(&mut st, &app);
            let moved = if dirty && index > position {
                // Named by what moved, not by the words it had: it
                // follows the panel's state now, not the one the
                // preset or sync was laid over.
                target.is_some_and(|t| st.sidecars[c].record(t))
            } else {
                st.sidecars[c].go_to(index)
            };
            st.status_kept = None;
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
            let Some(c) = st.current else {
                return;
            };
            let sidecar = &st.sidecars[c];
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
            // In culling the panel is not the frame's: the snapshot
            // is of the sidecar's current state as it stands.
            if st.cull.is_none() {
                record_panel(&mut st, &app);
            }
            let name = format!("Snapshot {}", st.sidecars[c].snapshots.len() + 1);
            let i = st.sidecars[c].take_snapshot(name.clone(), now());
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
            let Some(name) = st.sidecars[c]
                .snapshots
                .get(i as usize)
                .map(|s| s.name.clone())
            else {
                return;
            };
            // In culling the panel is not the frame's: nothing is
            // recorded from it, and the leaving develops the restore.
            if st.cull.is_some() {
                st.status_kept = None;
                if st.sidecars[c].restore_snapshot(i as usize) {
                    leave_cull(&mut st, &app, &worker, None);
                    app.set_status(format!("{name} restored").into());
                }
                return;
            }
            record_panel(&mut st, &app);
            st.status_kept = None;
            if !st.sidecars[c].restore_snapshot(i as usize) {
                // The sidecar is there already, but a panel still
                // showing the command line's overrides is not: it
                // takes the snapshot's state all the same.
                if st.overridden.is_some() {
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
        app.on_snapshot_hovered(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(c) = st.current else {
                return;
            };
            let snapshot = usize::try_from(i)
                .ok()
                .and_then(|i| st.sidecars[c].snapshots.get(i))
                .cloned();
            match snapshot {
                Some(s) => {
                    let what = format!(
                        "{}, taken {}; click to restore, double-click to rename",
                        s.name,
                        date_of(s.taken)
                    );
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
            let Some(s) = st.sidecars[c].snapshots.get_mut(i as usize) else {
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
            if (i as usize) >= st.sidecars[c].snapshots.len() {
                return;
            }
            let removed = st.sidecars[c].snapshots.remove(i as usize);
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
        let edit = read_edit(app, &st.edit, st.target);
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
            Outcome::Exported {
                path: PathBuf::from("/out/IMG_0000.jpg"),
                seconds: 1.0,
                note: None,
                source: source.clone(),
                edit: sent.clone(),
                preset: Some("Web".into()),
            },
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

        // A failed export, a skipped one, and a set's frame that failed
        // or was canceled: nothing.
        crate::panel::deliver::deliver(
            &app,
            Outcome::ExportFailed {
                message: "disk full".into(),
            },
        );
        crate::panel::deliver::deliver(
            &app,
            Outcome::ExportSkipped {
                path: PathBuf::from("/out/IMG_0000.jpg"),
            },
        );
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
            crate::queue::Done::Canceled,
        ] {
            crate::panel::deliver::deliver(
                &app,
                Outcome::SetFrameDone {
                    set: set.clone(),
                    index: 0,
                    source: source.clone(),
                    edit: sent.clone(),
                    done,
                },
            );
        }
        assert_eq!(rows(&app).len(), 4);
        assert_eq!(state.borrow().sidecars[0].exports(1).len(), 1);

        // Hovered, the viewport shows the look that went out.
        app.invoke_history_hovered(1);
        assert_eq!(state.borrow().peek.as_ref(), Some(&sent));
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
        state.borrow_mut().write_sidecars = true;
        let exported = |edit: &Edit| Outcome::Exported {
            path: dir.join("IMG_0100.jpg"),
            seconds: 1.0,
            note: None,
            source: raw.clone(),
            edit: edit.clone(),
            preset: None,
        };
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
        std::fs::remove_dir_all(&dir).unwrap();
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
            st.write_sidecars = true;
            st.sidecars[0] = Sidecar::load(&raws[0]).unwrap().unwrap();
            st.overrides_at_start = Some((
                0,
                crate::panel::startup::Overrides {
                    temperature: temperature.then_some(4000.0),
                    exposure: Some(1.0),
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
        state.borrow_mut().batch = true;
        let rendered = state.borrow().edit.clone();
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
            Outcome::Exported {
                path: dir.join("IMG_0200.jpg"),
                seconds: 1.0,
                note: None,
                source: raw.clone(),
                edit: rendered,
                preset: None,
            },
        );
        {
            let mut st = state.borrow_mut();
            let edit = read_edit(&app, &st.edit, st.target);
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        assert_eq!(state.borrow().edit, exposed(0.5));
        app.set_exposure(1.0);
        save(&app, &state);
        assert_eq!(state.borrow().sidecars[0].states(), 3);
        assert_eq!(on_disk(&raw).current, exposed(1.0));
        std::fs::remove_dir_all(&dir).unwrap();
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
            assert!(st.overridden.is_none(), "done with");
        }
        assert_eq!(app.get_exposure(), 0.0);
        app.invoke_redo();
        assert_eq!(state.borrow().sidecars[0].current, exposed(0.5));
        app.invoke_redo();
        assert_eq!(state.borrow().sidecars[0].current, exposed(0.75));
        std::fs::remove_dir_all(&dir).unwrap();
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
            st.write_sidecars = true;
            st.seed_blend[0] = true;
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
                    done: crate::queue::Done::Exported {
                        path: dir.join(format!("{i}.jpg")),
                        seconds: 1.0,
                        note: None,
                    },
                },
            );
        }
        assert!(!state.borrow().seed_blend[0], "taken");
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
        std::fs::remove_dir_all(&dir).unwrap();
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
            },
            crate::queue::Done::Failed {
                message: "no".into(),
            },
            crate::queue::Done::Exported {
                path: PathBuf::from("/out/IMG_0002.jpg"),
                seconds: 1.0,
                note: None,
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
        let generation = state.borrow().generation;
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
            assert_eq!(st.edit.retouch.patches.len(), 1);
            assert!(st.edit.retouch.patches[0].source.is_none());
            st.edit.retouch.patches[0].clone()
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
            let edit = crate::panel::edit::read_edit(&app, &st.edit, st.target);
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
        assert_eq!(state.borrow().edit.retouch.patches[0].source, Some(source));

        // One undo, and the spot is gone.
        app.invoke_undo();
        assert_eq!(spots(&state), vec![]);
        assert!(state.borrow().edit.retouch.patches.is_empty());
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
        let first = state.borrow().edit.retouch.patches[0].clone();
        let developed = greycard_edit::retouch::Patch {
            source: Some([0.05, -0.02]),
            ..first.clone()
        };

        // The save records the first spot; then it is deleted, and
        // another placed elsewhere, before the develop lands.
        {
            let mut st = state.borrow_mut();
            let edit = crate::panel::edit::read_edit(&app, &st.edit, st.target);
            crate::panel::edit::save_edit(&mut st, edit);
        }
        app.set_patch(0);
        app.invoke_delete_patch();
        assert!(state.borrow().edit.retouch.patches.is_empty());
        app.invoke_place_pressed(10.0, 10.0, false);
        app.invoke_place_released();
        {
            let mut st = state.borrow_mut();
            // Wherever the test window puts it, somewhere else.
            st.edit.retouch.patches[0].points = vec![[0.8, 0.2]];
            let second = &st.edit.retouch.patches[0];
            assert_eq!(second.id, first.id, "the id is reused");
            assert_ne!(second.points, first.points);
        }
        let states = state.borrow().sidecars[0].states();
        land(&app, &state, vec![developed.clone()]);
        assert!(state.borrow().edit.retouch.patches[0].source.is_none());
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
            st.edit.retouch.patches[0].points = first.points.clone();
            st.edit.retouch.patches[0].radius = first.radius * 2.0;
        }
        land(&app, &state, vec![developed]);
        assert!(state.borrow().edit.retouch.patches[0].source.is_none());
    }
}
