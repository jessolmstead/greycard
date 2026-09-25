use crate::panel::browser::file_name;
use crate::panel::cull::leave_cull;
use crate::panel::edit::{current_turn, edit_to_develop, read_edit, show_edit, write_sidecar};
use crate::*;

/// The undo and redo buttons, the history's rows and the snapshots'
/// names follow the current file's sidecar. The rows are newest
/// first, each named for what it changed from the one before.
pub(crate) fn show_history(st: &State, app: &App) {
    let names = |v: Vec<String>| {
        ModelRc::new(VecModel::from(
            v.into_iter()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>(),
        ))
    };
    let Some(c) = st.current else {
        app.set_history_names(names(Vec::new()));
        app.set_history_current(-1);
        app.set_snapshot_names(names(Vec::new()));
        return;
    };
    let sidecar = &st.sidecars[c];
    app.set_can_undo(!sidecar.history.is_empty());
    app.set_can_redo(!sidecar.redo.is_empty());
    let n = sidecar.states();
    let rows: Vec<String> = (0..n)
        .rev()
        .map(|i| {
            let state = sidecar.state(i).expect("a state within the count");
            // The words the step was recorded with, or what moved.
            match sidecar.describe(i) {
                Some(name) => name,
                // A fresh raw's learned-denoiser blend starts from its
                // ISO (`Noise::blend_for_iso`), not the plain default,
                // so that field alone is left out of the comparison.
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
        })
        .collect();
    app.set_history_names(names(rows));
    app.set_history_current((n - 1 - sidecar.position()) as i32);
    app.set_snapshot_names(names(
        sidecar.snapshots.iter().map(|s| s.name.clone()).collect(),
    ));
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
    // In culling the sidecar's current state is what the leaving
    // develops; the history moved, and the mode ends on that.
    if st.cull.is_some() {
        leave_cull(st, app, worker, None);
        return;
    }
    let edit = st.sidecars[c].current.clone();
    if st.write_sidecars
        && let Err(e) = st.sidecars[c].save_in(&st.files[c], st.placement)
    {
        tracing::warn!("{}: sidecar not saved: {e}", file_name(&st.files[c]));
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
            // In culling the panel is not the frame's, so nothing is
            // read from it: the sidecar's history moves on its own,
            // and the leaving develops what it lands on.
            if st.cull.is_some() {
                let moved = if back {
                    st.sidecars[c].undo()
                } else {
                    st.sidecars[c].redo()
                };
                if moved {
                    leave_cull(&mut st, &app, &worker, None);
                }
                return;
            }
            // Whatever the panel holds is a state first.
            let edit = read_edit(&app, &st.edit, st.target);
            st.sidecars[c].record(edit);
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
            let edit = read_edit(&app, &st.edit, st.target);
            let dirty = st.sidecars[c].record(edit);
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
            let hovered = sidecar.state_at_row(row).and_then(|i| {
                let name = app.get_history_names().row_data(row as usize)?;
                Some((sidecar.state(i)?.clone(), i, name))
            });
            match hovered {
                Some((edit, i, name)) => {
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
                let edit = read_edit(&app, &st.edit, st.target);
                st.sidecars[c].record(edit);
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
            let edit = read_edit(&app, &st.edit, st.target);
            st.sidecars[c].record(edit);
            st.status_kept = None;
            if !st.sidecars[c].restore_snapshot(i as usize) {
                app.set_status(format!("{name} is the current state").into());
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
}
