//! Remove rejects from an archive, on the window (notes §197, §216):
//! the CULLING section's entry, one an archive; the sheet that lists the
//! rejects here with their copies there before anything is done; its
//! two buttons, the move into a rejects folder beside each copy (the
//! default, Enter's) and the delete from the archive (red, a click's
//! alone, the trash or permanent as §190 offers it); the card over the
//! run with its Cancel; and the queue of moves confirmed while the
//! archive did not answer, run when it answers again.
//!
//! What asks the disk is `crate::archive::rejects`'s, run off the
//! window's thread on part one's look and beat (`send`): the archive is
//! looked at first with the roots' 3 s look, and a run that answered
//! and then went quiet is set aside, holding its own archive alone.
//! Nothing under an archive is deleted except behind this sheet; the
//! queue only ever moves.

use std::collections::HashSet;

use crate::archive::rejects::{self as core, Deletes, Entry, Item, Look, Moves};
use crate::delete::{self, How, Offer, TRASH_SUPPORTED};

use super::*;

/// The window's side of Remove rejects.
#[derive(Default)]
pub(crate) struct Rejects {
    /// What the sheet is over, while it is up or its look is out.
    asked: Option<Asked>,
    /// The sheet's look, once in.
    look: Option<Look>,
    /// What the sheet offered for the delete.
    offer: Option<Offer>,
    /// Numbers every look: one that lands under an older number is
    /// dropped.
    look_token: u64,
    /// The archives whose queue has been run since they last did not
    /// answer: a queue is run when its archive answers again, not at
    /// every pass over it.
    tried: HashSet<PathBuf>,
    /// The archives heard answering while another job held the card:
    /// their queue runs when it lands.
    due: Vec<PathBuf>,
    /// The runs set aside after the user had pressed Cancel, by number:
    /// what they did not reach was the user's to stop, and is not
    /// queued when they land late.
    canceled_by_hand: HashSet<u64>,
}

/// The sheet's question.
#[derive(Debug, Clone)]
struct Asked {
    archive: PathBuf,
    label: String,
    /// The open folder's rejects folder.
    dir: PathBuf,
}

/// What a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The sheet's move.
    Move,
    /// The sheet's delete.
    Delete(How),
    /// The queue's moves, run when the archive answered.
    Queue,
}

/// What a run's job found.
pub(crate) enum Ran {
    /// The archive did not answer its look: nothing was done.
    NotAnswering,
    Moved(Moves),
    Deleted(Deletes),
}

/// What a run hands back: what it was, over what, and what it did.
pub(crate) struct Done {
    kind: Kind,
    archive: PathBuf,
    label: String,
    /// The rejects it was over, with the copies the sheet listed.
    items: Vec<Item>,
    ran: Ran,
    /// Said after what it did: the queue's malformed entries dropped.
    said: String,
}

fn copies_word(n: usize) -> String {
    if n == 1 {
        "1 copy".into()
    } else {
        format!("{n} copies")
    }
}

fn names(names: &[String]) -> String {
    let paths: Vec<&Path> = names.iter().map(Path::new).collect();
    some_names(&paths)
}

/// The CULLING section's entries: one an archive, by name.
pub(crate) fn show(st: &State, app: &App) {
    let roots = &st.library.roots;
    let items: Vec<slint::SharedString> = roots
        .archives()
        .iter()
        .map(|a| format!("Remove rejects from {}...", roots.label(a)).into())
        .collect();
    app.set_archive_rejects_choices(ModelRc::new(VecModel::from(items)));
}

/// Remove rejects from the archive at `choice` asked for: what it is
/// over worked out here, from the window's state alone, and the look
/// sent off the window's thread. The sheet opens when the look lands.
pub(crate) fn ask(state: &Rc<RefCell<State>>, app: &App, choice: usize) {
    let mut st = state.borrow_mut();
    if st.archive.running.is_some() {
        app.set_status("a job on an archive is still under way".into());
        return;
    }
    let Some(archive) = st.library.roots.archives().get(choice).cloned() else {
        app.set_status("there is no archive: mark a root as one from its menu".into());
        return;
    };
    let label = st.library.roots.label(&archive);
    if st.view != View::Folder {
        app.set_status("a rejects folder is a folder's: open the folder first".into());
        return;
    }
    // The same folder §190's Delete rejects folder takes, in the
    // spelling the reads kept (no disk asked here: the open folder may
    // be on a share that hangs), without the `\\?\` Windows puts on a
    // canonical path, so the guard below and the look's own folder
    // compare with the index's paths.
    let Some(open) = super::open_folder(&st) else {
        app.set_status("nothing is open".into());
        return;
    };
    let open = dunce::simplified(&open).to_path_buf();
    let dir = crate::cull::rejects_dir(&open);
    let on_archive = |p: &Path| p.starts_with(&archive);
    if on_archive(&open) || st.files.first().is_some_and(|f| on_archive(f)) {
        app.set_status(
            format!(
                "the open folder is on {label}: its rejects folder is {label}'s own, and Delete \
                 rejects folder is the way to take it off"
            )
            .into(),
        );
        return;
    }
    if st.archive.aside.iter().any(|(_, a)| *a == archive) {
        app.set_status(
            format!(
                "the job set aside earlier has not come back from {label} yet; another starts \
                 once it has"
            )
            .into(),
        );
        return;
    }
    let r = &mut st.archive.rejects;
    r.look_token += 1;
    let token = r.look_token;
    r.asked = Some(Asked {
        archive: archive.clone(),
        label: label.clone(),
        dir: dir.clone(),
    });
    r.look = None;
    let index = st.index_path.clone();
    app.set_status(format!("looking for the rejects on {label}...").into());
    let sent = send(
        app,
        "archive rejects look",
        token,
        move |beat| {
            let answered = look_at(std::slice::from_ref(&archive), beat)[0];
            core::look(&dir, &archive, answered, index.as_deref(), beat)
        },
        land_look,
    );
    if !sent {
        st.archive.rejects.asked = None;
        app.set_status("the look at the archive could not be started".into());
    }
}

fn land_look(
    state: &Rc<RefCell<State>>,
    app: &App,
    _worker: &Rc<Worker>,
    token: u64,
    heard: Heard<Result<Look, String>>,
) {
    let mut st = state.borrow_mut();
    if token != st.archive.rejects.look_token {
        return;
    }
    let Some(asked) = st.archive.rejects.asked.clone() else {
        return;
    };
    match heard {
        Heard::Aside => {
            app.set_status(
                format!(
                    "{} has said nothing for {} s; the look is set aside and nothing was done",
                    asked.label,
                    ROOT_WAIT.as_secs()
                )
                .into(),
            );
            close(&mut st, app);
        }
        Heard::Lost => {
            app.set_status("the look at the archive failed; the log has why".into());
            close(&mut st, app);
        }
        Heard::Done { late: true, .. } => {}
        Heard::Done {
            result: Err(why), ..
        } => {
            app.set_status(why.into());
            close(&mut st, app);
        }
        Heard::Done {
            result: Ok(look), ..
        } => {
            if look.rejects.is_empty() {
                app.set_status(
                    format!(
                        "{} has no frames in it",
                        dunce::simplified(&look.dir).display()
                    )
                    .into(),
                );
                close(&mut st, app);
                return;
            }
            if !look.answered {
                st.archive.rejects.tried.remove(&asked.archive);
            }
            // The delete's offer, as §190 makes it: the trash, and the
            // permanent delete beside it when the trash has refused
            // files from one of these folders this session.
            let folders: HashSet<PathBuf> = look
                .with_copies()
                .flat_map(|r| r.found.copies.iter())
                .filter_map(|c| c.parent().map(Path::to_path_buf))
                .collect();
            let refused_here = folders.iter().any(|f| st.trash_refused.contains(f));
            let offer = delete::offer(TRASH_SUPPORTED, refused_here);
            fill(app, &asked, &look, offer);
            st.archive.rejects.look = Some(look);
            st.archive.rejects.offer = Some(offer);
            app.set_status("".into());
            app.set_archive_rejects_open(true);
        }
    }
}

fn close(st: &mut State, app: &App) {
    let r = &mut st.archive.rejects;
    r.asked = None;
    r.look = None;
    r.offer = None;
    app.set_archive_rejects_open(false);
    run_due(st, app);
}

/// The sheet's words for a look.
fn fill(app: &App, asked: &Asked, look: &Look, offer: Offer) {
    let label = &asked.label;
    let (n, bytes) = look.copies();
    let with: Vec<&core::Reject> = look.with_copies().collect();
    let mut lines = Vec::new();
    if n > 0 {
        lines.push(format!(
            "{} here {} {} on {label}, {}:",
            frames_word(with.len()).replace("frame", "reject"),
            if with.len() == 1 { "has" } else { "have" },
            copies_word(n),
            archive::size_words(bytes)
        ));
        const SHOWN: usize = 8;
        let mut shown = 0;
        'all: for r in &with {
            for c in &r.found.copies {
                if shown == SHOWN {
                    lines.push(format!("and {} more", n - SHOWN));
                    break 'all;
                }
                lines.push(format!(
                    "{}: {}",
                    file_name(&r.frame),
                    dunce::simplified(c).display()
                ));
                shown += 1;
            }
        }
    } else {
        lines.push(format!(
            "No copy of the {} in {} was found on {label}.",
            frames_word(look.rejects.len()).replace("frame", "reject"),
            dunce::simplified(&asked.dir).display()
        ));
    }
    if !look.answered {
        lines.push(format!(
            "{label} is not answering: the copies are as the index last saw them. A move is \
             queued and runs when {label} answers; a delete cannot be done while it does not."
        ));
    }
    let mut named = Vec::new();
    let none = look.none_found();
    if !none.is_empty() {
        named.push(format!("No copy found on {label}: {}.", some_names(&none)));
    }
    let unknown = look.not_known();
    if !unknown.is_empty() {
        named.push(format!(
            "Not known, a file there that could not be told from it without reading both whole, \
             left alone: {}.",
            some_names(&unknown)
        ));
    }
    let unreadable = look.unreadable();
    if !unreadable.is_empty() {
        let paths: Vec<&Path> = unreadable.iter().map(|(p, _)| *p).collect();
        named.push(format!("Could not be read: {}.", some_names(&paths)));
    }
    let delete_note = match (offer.trash, offer.permanent) {
        (true, false) => format!(
            "Delete sends the copies on {label} and their sidecars to the system trash, where \
             they can be restored from."
        ),
        (true, true) => format!(
            "The trash refused files from these folders on {label} earlier: try the trash \
             again, or delete the copies permanently. A permanent delete cannot be undone."
        ),
        _ => format!(
            "There is no system trash here, so a delete takes the copies off {label} \
             permanently. This cannot be undone."
        ),
    };
    let note = format!(
        "Move puts each copy into a rejects folder beside it on {label}, its sidecars with it; \
         nothing is deleted. {delete_note} The rejects here are not touched."
    );
    app.set_archive_rejects_title(format!("Remove rejects from {label}").into());
    app.set_archive_rejects_text(lines.join("\n").into());
    app.set_archive_rejects_named(named.join("\n").into());
    app.set_archive_rejects_note(note.into());
    let some = n > 0;
    app.set_archive_rejects_move(
        if look.to_move() > 0 {
            format!("Move to rejects on {label}")
        } else {
            String::new()
        }
        .into(),
    );
    let deletable = some && look.answered;
    let (trash, permanent) = match (deletable, offer.trash, offer.permanent) {
        (false, _, _) => (String::new(), String::new()),
        (true, true, false) => (format!("Delete from {label}"), String::new()),
        (true, true, true) => (
            format!("Delete from {label} (to the trash)"),
            format!("Delete from {label} permanently"),
        ),
        (true, false, _) => (String::new(), format!("Delete from {label} permanently")),
    };
    app.set_archive_rejects_trash(trash.into());
    app.set_archive_rejects_permanent(permanent.into());
}

/// The sheet's answer: 0 no, 1 move, 2 delete to the trash, 3 delete
/// permanently. An answer the sheet did not offer is a no.
pub(crate) fn answered(state: &Rc<RefCell<State>>, app: &App, answer: i32) {
    let mut st = state.borrow_mut();
    if answer != 0
        && let Some(asked) = &st.archive.rejects.asked
        && (st.archive.running.is_some()
            || st.archive.aside.iter().any(|(_, a)| *a == asked.archive))
    {
        app.set_status(
            "nothing done: another job on an archive is under way; ask again once it is done"
                .into(),
        );
        close(&mut st, app);
        return;
    }
    let r = &mut st.archive.rejects;
    r.look_token += 1;
    let (asked, look, offer) = (r.asked.take(), r.look.take(), r.offer.take());
    app.set_archive_rejects_open(false);
    let (Some(asked), Some(look), Some(offer)) = (asked, look, offer) else {
        run_due(&mut st, app);
        return;
    };
    let items = look.items();
    // What the sheet offered, and nothing else: no delete from a look
    // the archive did not answer, no move with nothing to move.
    let kind = match answer {
        1 if look.to_move() > 0 => Some(Kind::Move),
        2 if offer.trash && look.answered => Some(Kind::Delete(How::Trash)),
        3 if offer.permanent && look.answered => Some(Kind::Delete(How::Permanent)),
        _ => None,
    };
    if matches!(answer, 2 | 3) && !look.answered {
        app.set_status(
            format!(
                "not deleted: {} is not answering. A delete is never queued; ask again when it \
                 answers",
                asked.label
            )
            .into(),
        );
        run_due(&mut st, app);
        return;
    }
    let Some(kind) = kind.filter(|_| !items.is_empty()) else {
        run_due(&mut st, app);
        return;
    };
    if !st.deletes_allowed {
        let why = "nothing done: a capture, an export or a timing run never moves or deletes";
        tracing::info!("{why}");
        app.set_status(why.into());
        return;
    }
    start(
        &mut st,
        app,
        kind,
        asked.archive,
        asked.label,
        items,
        String::new(),
    );
}

/// A run sent off, its card up.
fn start(
    st: &mut State,
    app: &App,
    kind: Kind,
    archive: PathBuf,
    label: String,
    items: Vec<Item>,
    said: String,
) {
    st.archive.run_token += 1;
    let token = st.archive.run_token;
    let cancel = Arc::new(AtomicBool::new(false));
    let count = Arc::new(AtomicU64::new(0));
    let doing = match kind {
        Kind::Move | Kind::Queue => "moving the rejects on",
        Kind::Delete(_) => "deleting the rejects from",
    };
    st.archive.running = Some(Running {
        cancel: cancel.clone(),
        bytes: count.clone(),
        total: items.len() as u64,
        doing,
        by_files: true,
        label: label.clone(),
        token,
        archive: archive.clone(),
    });
    tracing::info!(
        "archive: {doing} {} ({kind:?}), {}",
        label,
        frames_word(items.len())
    );
    app.set_archive_fraction(0.0);
    app.set_archive_line(format!("{doing} {label}...").into());
    app.set_archive_running(true);
    let index = st.index_path.clone();
    let sent = send(
        app,
        "archive rejects",
        token,
        move |beat| {
            let answered = look_at(std::slice::from_ref(&archive), beat)[0];
            let ran = if !answered {
                Ran::NotAnswering
            } else {
                let hooks = archive::Hooks::new(&**beat, &cancel, &count);
                match kind {
                    Kind::Move | Kind::Queue => Ran::Moved(core::run_moves(
                        &archive,
                        &items,
                        index.as_deref(),
                        &hooks,
                        beat,
                    )),
                    Kind::Delete(how) => Ran::Deleted(core::run_deletes(
                        &archive,
                        &items,
                        index.as_deref(),
                        how,
                        delete::system_trash,
                        &hooks,
                        beat,
                    )),
                }
            };
            Done {
                kind,
                archive,
                label,
                items,
                ran,
                said,
            }
        },
        land_run,
    );
    if !sent {
        st.archive.running = None;
        app.set_archive_running(false);
        app.set_status("the job could not be started; nothing was done".into());
        return;
    }
    start_timer(st, app);
}

/// A run landed, or went quiet: said, the queue kept, the folders
/// handed to the indexer, and a queue that waited on it run.
fn land_run(
    state: &Rc<RefCell<State>>,
    app: &App,
    _worker: &Rc<Worker>,
    token: u64,
    heard: Heard<Done>,
) {
    let mut st = state.borrow_mut();
    let ours = st
        .archive
        .running
        .as_ref()
        .is_some_and(|r| r.token == token);
    match heard {
        Heard::Aside => {
            if !ours {
                return;
            }
            if let Some(r) = st.archive.running.take() {
                // It goes on alone: the frame it is on, and no more. No
                // other job on that archive starts until it lands.
                if r.cancel.swap(true, Ordering::SeqCst) {
                    st.archive.rejects.canceled_by_hand.insert(token);
                }
                st.archive.aside.push((token, r.archive.clone()));
                // Its queue, if it was one, is not done: the next answer
                // tries it again.
                st.archive.rejects.tried.remove(&r.archive);
                st.archive.timer.stop();
                app.set_archive_running(false);
                app.set_status(
                    format!(
                        "{} has said nothing for {} s: the rejects' job is set aside, and stops \
                         after the frame it is on; what it did is said if it answers",
                        r.label,
                        ROOT_WAIT.as_secs()
                    )
                    .into(),
                );
            }
        }
        Heard::Lost => {
            let mut archive = None;
            if ours {
                archive = st.archive.running.take().map(|r| r.archive);
                st.archive.timer.stop();
                app.set_archive_running(false);
            }
            if let Some(i) = st.archive.aside.iter().position(|(t, _)| *t == token) {
                archive = Some(st.archive.aside.remove(i).1);
            }
            if let Some(a) = archive {
                st.archive.rejects.tried.remove(&a);
            }
            st.archive.rejects.canceled_by_hand.remove(&token);
            tracing::error!("archive: the rejects' job ended without a report");
            app.set_status(
                "the rejects' job stopped without a report; what it did is found again by the \
                 next look (the log has why)"
                    .into(),
            );
            run_due(&mut st, app);
        }
        Heard::Done { result, late } => {
            if ours {
                st.archive.running = None;
                st.archive.timer.stop();
                app.set_archive_running(false);
            }
            st.archive.aside.retain(|(t, _)| *t != token);
            // The frames a late run did not reach are queued only when
            // the archive's silence stopped it, not the user's Cancel.
            let by_hand = st.archive.rejects.canceled_by_hand.remove(&token);
            let mut words = landed(&mut st, result, late && !by_hand);
            if late {
                words = format!("the job set aside earlier came back: {words}");
            }
            tracing::info!("archive: {words}");
            app.set_status(words.into());
            run_due(&mut st, app);
        }
    }
}

/// What a run did, taken in: the queue written, the rows and folders
/// handed to the indexer, a refusal of the trash remembered; its words.
fn landed(st: &mut State, done: Done, late: bool) -> String {
    let Done {
        kind,
        archive,
        label,
        items,
        ran,
        said,
    } = done;
    let entries: Vec<Entry> = items.into_iter().map(|i| i.entry).collect();
    let words = landed_words(st, kind, archive, label, entries, ran, late);
    format!("{words}{said}")
}

/// `entries` added to `archive`'s queue; what was dropped from it said.
fn enqueue(st: &State, archive: &Path, entries: &[Entry]) -> Result<String, String> {
    let Some(file) = st.library.file.as_deref().map(core::queue_path) else {
        return Err("there is no library to keep the queue beside".into());
    };
    core::edit_queue(&file, |q| core::enqueue(q, archive, entries))
        .map(|(read, ())| dropped_words(&read.dropped))
        .map_err(|e| {
            tracing::warn!("archive: {}: {e}", file.display());
            e.to_string()
        })
}

fn landed_words(
    st: &mut State,
    kind: Kind,
    archive: PathBuf,
    label: String,
    entries: Vec<Entry>,
    ran: Ran,
    late: bool,
) -> String {
    match (kind, ran) {
        (Kind::Move, Ran::NotAnswering) => {
            st.archive.rejects.tried.remove(&archive);
            match enqueue(st, &archive, &entries) {
                Ok(dropped) => format!(
                    "{label} is not answering: the move of {} is queued, and runs when {label} \
                     answers{dropped}",
                    frames_word(entries.len()).replace("frame", "reject")
                ),
                Err(e) => {
                    format!("{label} is not answering, and the move could not be queued: {e}")
                }
            }
        }
        (Kind::Delete(_), Ran::NotAnswering) => {
            st.archive.rejects.tried.remove(&archive);
            format!(
                "not deleted: {label} is not answering. A delete is never queued; ask again when \
                 it answers"
            )
        }
        (Kind::Queue, Ran::NotAnswering) => {
            st.archive.rejects.tried.remove(&archive);
            format!("{label} did not answer after all: its queued moves wait for the next time")
        }
        (_, Ran::Moved(moves)) => {
            hand_folders(st, &moves.folders);
            let mut words = moves_words(&moves, &label);
            if kind == Kind::Move && late && !moves.canceled.is_empty() {
                // Set aside, the sheet's move was told to stop by the
                // archive's silence, not the user: the frames it did not
                // reach were confirmed while the archive did not answer,
                // and are queued (§197).
                st.archive.rejects.tried.remove(&archive);
                match enqueue(st, &archive, &moves.canceled) {
                    Ok(dropped) => words.push_str(&format!(
                        "; the {} not reached are queued, and run when {label} answers{dropped}",
                        frames_word(moves.canceled.len())
                    )),
                    Err(e) => words.push_str(&format!(
                        "; the {} not reached could not be queued: {e}",
                        frames_word(moves.canceled.len())
                    )),
                }
            }
            if kind != Kind::Queue {
                return words;
            }
            // A queue run the user canceled holds: what it did not reach
            // waits for the archive's next return or the next launch,
            // not the next pass. (Set aside or lost, it is tried again:
            // `land_run`.)
            // The queue: what was done taken out of it, the rest left and
            // named.
            let Some(file) = st.library.file.as_deref().map(core::queue_path) else {
                return words;
            };
            let edited = core::edit_queue(&file, |q| core::dequeue(q, &archive, &moves.done));
            let mut words = format!("{label} answered, and its queued moves ran: {words}");
            match edited {
                Ok((read, ())) => words.push_str(&dropped_words(&read.dropped)),
                Err(e) => {
                    tracing::warn!("archive: {}: {e}", file.display());
                    words.push_str(&format!("; the queue could not be written: {e}"));
                }
            }
            let left: Vec<String> = moves
                .left
                .iter()
                .map(|(e, _)| e.name.clone())
                .chain(moves.canceled.iter().map(|e| e.name.clone()))
                .collect();
            if !left.is_empty() {
                words.push_str(&format!(
                    "; left in the queue: {}{}",
                    names(&left),
                    moves
                        .left
                        .first()
                        .map(|(_, why)| format!(" ({why})"))
                        .unwrap_or_default()
                ));
            }
            words
        }
        (_, Ran::Deleted(deletes)) => {
            if let Some(indexer) = &st.index {
                indexer.forget(deletes.deleted.frames.clone());
            }
            if let Some(r) = &deletes.deleted.trash_refused
                && let Some(dir) = r.frame.parent()
            {
                st.trash_refused.insert(dir.to_path_buf());
            }
            for (f, why) in deletes.refused.iter().chain(&deletes.deleted.failed) {
                tracing::warn!("{}: not deleted: {why}", f.display());
            }
            hand_folders(st, &deletes.folders);
            let how = match kind {
                Kind::Delete(how) => how,
                _ => How::Permanent,
            };
            deletes_words(&deletes, how, &label)
        }
    }
}

/// The folders a run touched, passed over now rather than at the next
/// poll.
fn hand_folders(st: &State, folders: &[PathBuf]) {
    if let Some(indexer) = &st.index
        && !folders.is_empty()
    {
        indexer
            .asker()
            .changes(folders.iter().cloned().map(Change::Folder).collect());
    }
}

fn dropped_words(dropped: &[String]) -> String {
    match dropped {
        [] => String::new(),
        [one] => format!("; dropped from the archive queue as malformed: {one}"),
        many => format!(
            "; dropped from the archive queue as malformed: {} entries ({}; the log has the rest)",
            many.len(),
            many[0]
        ),
    }
}

/// What a run of the moves did, in a line.
pub(crate) fn moves_words(m: &Moves, label: &str) -> String {
    let mut parts = Vec::new();
    if m.moved.is_empty() {
        parts.push(format!("nothing was moved on {label}"));
    } else {
        let mut first = format!(
            "moved {} on {label} into a rejects folder beside each",
            copies_word(m.moved.len())
        );
        if m.sidecars > 0 {
            first.push_str(&format!(
                ", {} with them",
                delete::count(m.sidecars, "sidecar")
            ));
        }
        parts.push(first);
    }
    if m.already > 0 {
        parts.push(format!(
            "{} in a rejects folder there already",
            copies_word(m.already)
        ));
    }
    if let Some((_, why)) = m.left.first() {
        let left: Vec<String> = m.left.iter().map(|(e, _)| e.name.clone()).collect();
        parts.push(format!("left: {} ({why})", names(&left)));
    }
    if !m.not_known.is_empty() {
        parts.push(format!(
            "a copy not known, left alone: {}",
            names(&m.not_known)
        ));
    }
    if !m.canceled.is_empty() {
        parts.push(format!(
            "canceled, {} not reached",
            frames_word(m.canceled.len())
        ));
    }
    parts.join("; ")
}

/// What a run of the deletes did, in a line: §190's words, saying it
/// was the archive.
pub(crate) fn deletes_words(d: &Deletes, how: How, label: &str) -> String {
    let mut words = format!(
        "on {label}, {}",
        delete::summary(&d.deleted, how, &d.refused)
    );
    if !d.not_found.is_empty() {
        words.push_str(&format!("; no copy found there: {}", names(&d.not_found)));
    }
    if !d.not_known.is_empty() {
        words.push_str(&format!(
            "; a copy not known, left alone: {}",
            names(&d.not_known)
        ));
    }
    if d.canceled > 0 {
        let why = if d.deleted.trash_refused.is_some() {
            "after the refusal"
        } else {
            "canceled,"
        };
        words.push_str(&format!("; {why} {} not reached", frames_word(d.canceled)));
    }
    words
}

/// Whether `archive` answered a look made for something else (the
/// launch's, the poll's, a view's read, a pass over it): when it did
/// and its queue has not been run since it last did not, the queue is
/// run; when it did not, its queue is due again for the next answer.
pub(crate) fn heard_from(st: &mut State, app: &App, archive: &Path, answered: bool) {
    if !st.library.roots.is_archive(archive) {
        return;
    }
    if !answered {
        st.archive.rejects.tried.remove(archive);
        return;
    }
    if st.archive.rejects.tried.contains(archive) {
        return;
    }
    run_queue(st, app, archive);
}

/// The archives the roots' look found, one way or the other: those in
/// `offline` did not answer, the rest did. For a look over every root.
pub(crate) fn heard_over(st: &mut State, app: &App, offline: &HashSet<PathBuf>) {
    let archives = st.library.roots.archives().to_vec();
    for a in archives {
        let answered = !offline.contains(&a);
        heard_from(st, app, &a, answered);
    }
}

/// The queue at the launch's look, before there is a window to run it
/// on: the archives that answered are due, those that did not are not
/// tried.
pub(crate) fn heard_at_launch(st: &mut State, offline: &HashSet<PathBuf>) {
    let archives = st.library.roots.archives().to_vec();
    for a in archives {
        if offline.contains(&a) {
            st.archive.rejects.tried.remove(&a);
        } else if !st.archive.rejects.due.contains(&a) {
            st.archive.rejects.due.push(a);
        }
    }
}

/// The queues heard due while the card was held, run now.
pub(crate) fn run_due(st: &mut State, app: &App) {
    if st.archive.running.is_some() {
        return;
    }
    // The first starts; the rest find the card held and are due again.
    for a in std::mem::take(&mut st.archive.rejects.due) {
        heard_from(st, app, &a, true);
    }
}

/// `archive`'s queue read and run: the malformed entries dropped and
/// said, then its moves through the same path as the sheet's, the
/// archive looked at again first. Waits for a job under way, or one set
/// aside on that archive. Never in a run nobody is at.
fn run_queue(st: &mut State, app: &App, archive: &Path) {
    if !st.deletes_allowed {
        return;
    }
    let Some(file) = st.library.file.as_deref().map(core::queue_path) else {
        return;
    };
    let sheet_up = st.archive.asked.is_some() || st.archive.rejects.asked.is_some();
    if sheet_up
        || st.archive.running.is_some()
        || st.archive.aside.iter().any(|(_, a)| a == archive)
    {
        if !st.archive.rejects.due.iter().any(|a| a == archive) {
            st.archive.rejects.due.push(archive.to_path_buf());
        }
        return;
    }
    let read = match core::read_queue(&file) {
        Ok(read) => read,
        Err(e) => {
            tracing::warn!("archive: {}: {e}", file.display());
            return;
        }
    };
    if !read.dropped.is_empty() {
        for d in &read.dropped {
            tracing::warn!("archive: {}: dropped {d}", file.display());
        }
        if let Err(e) = core::write_queue(&file, &read.queue) {
            tracing::warn!("archive: {}: {e}", file.display());
        }
        app.set_status(dropped_words(&read.dropped).trim_start_matches("; ").into());
    }
    st.archive.rejects.tried.insert(archive.to_path_buf());
    let entries: Vec<Item> = read
        .queue
        .into_iter()
        .filter(|q| q.archive == archive)
        .flat_map(|q| q.frames)
        .map(Item::queued)
        .collect();
    if entries.is_empty() {
        return;
    }
    let label = st.library.roots.label(archive);
    app.set_status(
        format!(
            "{label} answered: running its queued move of {}...",
            frames_word(entries.len()).replace("frame", "reject")
        )
        .into(),
    );
    start(
        st,
        app,
        Kind::Queue,
        archive.to_path_buf(),
        label,
        entries,
        dropped_words(&read.dropped),
    );
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_archive_rejects_asked(move |i| {
            if let Some(app) = app_weak.upgrade() {
                ask(&state, &app, i.max(0) as usize);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_archive_rejects_answered(move |a| {
            if let Some(app) = app_weak.upgrade() {
                answered(&state, &app, a);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{ASIDE_NEXT, SENT, land_sent};
    use super::*;
    use crate::testing::{press, state_for, window};
    use greycard_edit::{Placement, SIDECAR_FOLDER, Sidecar};
    use greycard_library::fixture::{A7, R5, R6, write_frame};
    use slint::platform::Key;

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-archive-rejects-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    fn index(db: &Path, roots: &[&Path]) {
        let mut lib = greycard_library::Library::open(db).unwrap();
        for r in roots {
            lib.index_tree(r, &mut |_| {}).unwrap();
        }
    }

    /// A copy of `from` at `to` with its time kept, as a backup makes.
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(from, to).unwrap();
        let t = std::fs::metadata(from).unwrap().modified().unwrap();
        std::fs::File::options()
            .write(true)
            .open(to)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    /// A shoot of two frames culled here, its rejects folder holding a
    /// and b; on the archive `nas`, under a folder of its own, a's copy
    /// with its `.gcd` beside and b's with its `.gcd` under the hidden
    /// folder. The window open on the shoot, `nas` an archive.
    struct Culled {
        dir: PathBuf,
        nas: PathBuf,
        rejects: PathBuf,
        there: PathBuf,
        app: App,
        state: Rc<RefCell<State>>,
        worker: Rc<Worker>,
    }

    fn culled(what: &str) -> Culled {
        static SEED: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(500);
        let seed = || SEED.fetch_add(1, Ordering::Relaxed);
        let dir = scratch(what);
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot = local.join("shoot");
        let rejects = crate::cull::rejects_dir(&shoot);
        std::fs::create_dir_all(&rejects).unwrap();
        let files: Vec<PathBuf> = ["x.tif", "y.tif"].iter().map(|n| shoot.join(n)).collect();
        write_frame(&files[0], &R5, seed());
        write_frame(&files[1], &R6, seed());
        write_frame(&rejects.join("a.tif"), &A7, seed());
        write_frame(&rejects.join("b.tif"), &R5, seed());
        let there = nas.join("clients").join("shoot");
        copy(&rejects.join("a.tif"), &there.join("a.tif"));
        copy(&rejects.join("b.tif"), &there.join("b.tif"));
        let mut s = Sidecar::default();
        s.meta.rating = 1;
        s.save_in(&there.join("a.tif"), Placement::Beside).unwrap();
        s.save_in(&there.join("b.tif"), Placement::Folder).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let app = window(files.len());
        let (state, worker) = state_for(&app, files);
        {
            let mut st = state.borrow_mut();
            st.deletes_allowed = true;
            st.current = Some(0);
            st.index_path = Some(db.clone());
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            let file = dir.join("roots.json");
            let mut roots = greycard_library::Roots::default();
            roots.add(&local).unwrap();
            roots.add(&nas).unwrap();
            roots.set_archive(&nas, true);
            roots.save(&file).unwrap();
            st.library.roots = roots;
            st.library.file = Some(file);
            crate::panel::browser::rebuild_browser(&mut st, &app);
            super::super::show(&st, &app);
        }
        Culled {
            dir,
            nas,
            rejects,
            there,
            app,
            state,
            worker,
        }
    }

    impl Culled {
        fn land(&self) -> usize {
            land_sent(&self.state, &self.app, &self.worker)
        }

        /// The sheet asked for and its look landed.
        fn sheet(&self) {
            self.app.invoke_archive_rejects_asked(0);
            assert!(!self.app.get_archive_rejects_open(), "not until the look");
            assert_eq!(self.land(), 1);
            assert!(
                self.app.get_archive_rejects_open(),
                "{}",
                self.app.get_status()
            );
        }

        fn queue(&self) -> PathBuf {
            core::queue_path(&self.dir.join("roots.json"))
        }

        /// The archive gone, as a share that does not answer: its rows
        /// are as the index has them.
        fn unplug(&self) {
            std::fs::rename(&self.nas, self.dir.join("nas-away")).unwrap();
        }

        fn plug(&self) {
            std::fs::rename(self.dir.join("nas-away"), &self.nas).unwrap();
        }

        fn done(self) {
            drop(self.state);
            std::fs::remove_dir_all(&self.dir).unwrap();
        }
    }

    #[test]
    fn the_sheet_lists_the_rejects_with_their_copies_and_moves_them() {
        let c = culled("move");
        assert_eq!(
            c.app
                .get_archive_rejects_choices()
                .iter()
                .collect::<Vec<_>>(),
            ["Remove rejects from nas..."]
        );
        c.sheet();
        assert_eq!(c.app.get_archive_rejects_title(), "Remove rejects from nas");
        let text = c.app.get_archive_rejects_text();
        assert!(
            text.starts_with("2 rejects here have 2 copies on nas"),
            "{text}"
        );
        assert!(text.contains(&format!("a.tif: {}", c.there.join("a.tif").display())));
        assert_eq!(c.app.get_archive_rejects_move(), "Move to rejects on nas");
        assert_eq!(c.app.get_archive_rejects_trash(), "Delete from nas");
        assert_eq!(c.app.get_archive_rejects_permanent(), "");
        assert!(c.app.get_archive_rejects_note().contains("system trash"));

        // Enter is the move.
        press(&c.app, Key::Return);
        assert!(!c.app.get_archive_rejects_open());
        assert!(c.app.get_archive_running(), "the card is up");
        assert_eq!(c.land(), 1);
        assert!(!c.app.get_archive_running());
        let out = crate::cull::rejects_dir(&c.there);
        assert!(out.join("a.tif").is_file() && out.join("a.tif.gcd").is_file());
        assert!(out.join("b.tif").is_file());
        assert!(out.join(SIDECAR_FOLDER).join("b.tif.gcd").is_file());
        assert!(!c.there.join("a.tif").exists() && !c.there.join("b.tif").exists());
        // The rejects here untouched, nothing queued.
        assert!(c.rejects.join("a.tif").is_file() && c.rejects.join("b.tif").is_file());
        assert!(!c.queue().exists());
        assert_eq!(
            c.app.get_status(),
            "moved 2 copies on nas into a rejects folder beside each, 2 sidecars with them"
        );
        c.done();
    }

    #[test]
    fn the_delete_is_a_click_offering_the_trash_or_permanent_as_190_does() {
        let c = culled("delete");
        // An answer the sheet did not offer is a no: the permanent
        // delete while only the trash is offered.
        c.sheet();
        c.app.invoke_archive_rejects_answered(3);
        assert_eq!(c.land(), 0);
        assert!(c.there.join("a.tif").is_file());
        // The trash refused these folders earlier: both offered.
        c.state.borrow_mut().trash_refused.insert(c.there.clone());
        c.sheet();
        assert_eq!(
            c.app.get_archive_rejects_trash(),
            "Delete from nas (to the trash)"
        );
        assert_eq!(
            c.app.get_archive_rejects_permanent(),
            "Delete from nas permanently"
        );
        assert!(c.app.get_archive_rejects_note().contains("refused"));
        c.app.invoke_archive_rejects_answered(3);
        assert_eq!(c.land(), 1);
        assert!(!c.there.join("a.tif").exists() && !c.there.join("a.tif.gcd").exists());
        assert!(!c.there.join("b.tif").exists());
        assert!(!c.there.join(SIDECAR_FOLDER).join("b.tif.gcd").exists());
        assert!(c.rejects.join("a.tif").is_file(), "the rejects here stay");
        let status = c.app.get_status();
        assert!(
            status.starts_with("on nas, deleted permanently: 2 frames and 2 sidecars"),
            "{status}"
        );
        assert!(!c.queue().exists(), "a delete is never queued");
        c.done();
    }

    /// The move confirmed while the archive does not answer is queued;
    /// a delete is refused and never queued; the queue runs when the
    /// archive answers, through the same path, and what cannot be done
    /// is left and named.
    #[test]
    fn offline_a_move_is_queued_and_run_when_the_archive_answers() {
        let c = culled("queue");
        c.unplug();
        c.sheet();
        let text = c.app.get_archive_rejects_text();
        assert!(
            text.starts_with("2 rejects here have 2 copies on nas"),
            "{text}"
        );
        assert!(text.contains("nas is not answering"), "{text}");

        // No delete is offered from a look the archive did not answer,
        // and one asked for anyway is refused and not queued.
        assert_eq!(c.app.get_archive_rejects_trash(), "");
        assert_eq!(c.app.get_archive_rejects_permanent(), "");
        c.app.invoke_archive_rejects_answered(2);
        assert_eq!(c.land(), 0);
        assert!(
            c.app
                .get_status()
                .starts_with("not deleted: nas is not answering"),
            "{}",
            c.app.get_status()
        );
        // Offered while it answered, confirmed once it no longer does:
        // the run's own look refuses it. (The trash is never reached:
        // the archive's look comes first.)
        c.plug();
        c.sheet();
        assert_eq!(c.app.get_archive_rejects_trash(), "Delete from nas");
        c.unplug();
        c.app.invoke_archive_rejects_answered(2);
        assert_eq!(c.land(), 1);
        assert!(
            c.app
                .get_status()
                .starts_with("not deleted: nas is not answering"),
            "{}",
            c.app.get_status()
        );
        assert!(!c.queue().exists());

        // The move, queued.
        c.sheet();
        c.app.invoke_archive_rejects_answered(1);
        assert_eq!(c.land(), 1);
        let status = c.app.get_status();
        assert!(
            status.starts_with("nas is not answering: the move of 2 rejects is queued"),
            "{status}"
        );
        let read = core::read_queue(&c.queue()).unwrap();
        assert_eq!(read.queue.len(), 1);
        assert_eq!(read.queue[0].archive, c.nas);
        assert_eq!(read.queue[0].frames.len(), 2);
        assert_eq!(read.queue[0].frames[0].name, "a.tif");

        // Heard not answering again, by a pass skipped: nothing runs.
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, false);
        assert_eq!(c.land(), 0);

        // Meanwhile a's reject leaves the folder here (it still runs),
        // an entry that cannot be done joins the queue, and a malformed
        // one.
        std::fs::remove_file(c.rejects.join("a.tif")).unwrap();
        let text = std::fs::read_to_string(c.queue()).unwrap().replacen(
            "\"frames\": [",
            "\"frames\": [{\"hash\": \"0000\", \"name\": \"gone.tif\", \"size\": 7}, \
             {\"name\": \"broken.tif\"},",
            1,
        );
        std::fs::write(c.queue(), text).unwrap();

        // The archive answers (the poll's pass over it): the queue runs.
        c.plug();
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        assert!(c.app.get_archive_running());
        assert_eq!(c.land(), 1);
        let out = crate::cull::rejects_dir(&c.there);
        assert!(out.join("a.tif").is_file() && out.join("b.tif").is_file());
        let status = c.app.get_status();
        assert!(
            status.starts_with("nas answered, and its queued moves ran: moved 2 copies on nas"),
            "{status}"
        );
        assert!(status.contains("left in the queue: gone.tif"), "{status}");
        assert!(
            status.contains("dropped from the archive queue"),
            "{status}"
        );
        let read = core::read_queue(&c.queue()).unwrap();
        assert!(read.dropped.is_empty(), "the malformed entry is gone");
        assert_eq!(read.queue[0].frames.len(), 1);
        assert_eq!(read.queue[0].frames[0].name, "gone.tif");

        // Heard again without going away: not run again.
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        assert_eq!(c.land(), 0);
        c.done();
    }

    /// The archive's chip clicked: the view's look at every root finds
    /// it answering, and its queue runs.
    #[test]
    fn the_chip_clicked_runs_the_queue_when_the_archive_answers() {
        let c = culled("chip");
        c.unplug();
        c.sheet();
        c.app.invoke_archive_rejects_answered(1);
        c.land();
        assert!(c.queue().is_file());
        c.plug();
        crate::roots::open_view(
            &c.state,
            &c.app,
            &c.worker,
            View::Roots(Some(c.nas.clone())),
        );
        crate::roots::land_sent(&c.state, &c.app, &c.worker);
        assert_eq!(c.land(), 1, "the queue's run");
        assert!(crate::cull::rejects_dir(&c.there).join("a.tif").is_file());
        assert!(!c.queue().exists(), "done, and the queue emptied");
        c.done();
    }

    /// Queue a move of both rejects' copies with the archive away, and
    /// bring it back: the queue is due.
    fn queued(c: &Culled) {
        c.unplug();
        c.sheet();
        c.app.invoke_archive_rejects_answered(1);
        assert_eq!(c.land(), 1);
        assert!(c.queue().is_file());
        c.plug();
    }

    /// The launch's look found the archive answering: the queue is due,
    /// and runs once the window has it (`send_build`'s landing). A run
    /// nobody is at never runs it; nor does one while a sheet is up,
    /// which it waits for.
    #[test]
    fn the_launch_runs_the_queue_but_not_in_a_capture_nor_under_a_sheet() {
        let c = culled("launch");
        queued(&c);
        // A capture: nothing moves.
        c.state.borrow_mut().deletes_allowed = false;
        heard_at_launch(&mut c.state.borrow_mut(), &HashSet::new());
        run_due(&mut c.state.borrow_mut(), &c.app);
        assert_eq!(c.land(), 0);
        assert!(c.there.join("a.tif").is_file());
        c.state.borrow_mut().deletes_allowed = true;
        // The sheet up: the queue waits for it, and runs when it closes.
        c.sheet();
        heard_at_launch(&mut c.state.borrow_mut(), &HashSet::new());
        run_due(&mut c.state.borrow_mut(), &c.app);
        assert!(!c.app.get_archive_running());
        c.app.invoke_archive_rejects_answered(0);
        assert!(c.app.get_archive_running(), "{}", c.app.get_status());
        assert_eq!(c.land(), 1);
        assert!(crate::cull::rejects_dir(&c.there).join("a.tif").is_file());
        assert!(!c.queue().exists());
        c.done();
    }

    /// A queue run under way holds the card: the archive heard
    /// answering waits for an open Back up sheet, and a run that began
    /// anyway is not overtaken by that sheet's confirm.
    #[test]
    fn a_queue_run_under_way_is_not_overtaken_by_a_backup() {
        let c = culled("overtaken");
        queued(&c);
        c.app.invoke_archive_header_pressed(0);
        assert_eq!(c.land(), 1);
        assert!(c.app.get_archive_open());
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        assert!(!c.app.get_archive_running(), "it waits for the sheet");
        // A run on the archive begun all the same (the guard's own case).
        {
            let mut st = c.state.borrow_mut();
            let entries = core::read_queue(&c.queue()).unwrap().queue[0]
                .frames
                .iter()
                .cloned()
                .map(Item::queued)
                .collect();
            start(
                &mut st,
                &c.app,
                Kind::Queue,
                c.nas.clone(),
                "nas".into(),
                entries,
                String::new(),
            );
        }
        let token = c.state.borrow().archive.running.as_ref().unwrap().token;
        c.app.invoke_archive_answered(true);
        assert!(
            c.app.get_status().starts_with("not copied: another job"),
            "{}",
            c.app.get_status()
        );
        assert_eq!(
            c.state.borrow().archive.running.as_ref().unwrap().token,
            token
        );
        c.land();
        assert!(crate::cull::rejects_dir(&c.there).join("a.tif").is_file());
        assert!(!c.nas.join("local").exists(), "nothing backed up");
        c.done();
    }

    /// The open folder on the archive itself: refused, its rejects are
    /// the archive's own.
    #[test]
    fn a_folder_on_the_archive_is_refused() {
        let c = culled("on-archive");
        let on = c.nas.join("clients").join("shoot");
        crate::cull::move_rejects(&[on.join("a.tif")], &[0]).unwrap();
        c.state.borrow_mut().files = vec![on.join("b.tif")];
        c.app.invoke_archive_rejects_asked(0);
        assert_eq!(c.land(), 0);
        assert!(
            c.app.get_status().contains("is nas's own"),
            "{}",
            c.app.get_status()
        );
        c.done();
    }

    /// A malformed entry is dropped and said when the queue is read.
    #[test]
    fn a_malformed_queue_entry_is_dropped_and_said() {
        let c = culled("malformed");
        std::fs::write(
            c.queue(),
            format!(
                "{{\"version\": 1, \"queue\": [{{\"archive\": \"{}\", \"frames\": [{{\"name\": \
                 \"broken.tif\"}}]}}]}}",
                c.nas.display()
            ),
        )
        .unwrap();
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        let status = c.app.get_status();
        assert!(
            status.starts_with("dropped from the archive queue as malformed"),
            "{status}"
        );
        assert_eq!(c.land(), 0, "nothing left to run");
        assert!(!c.queue().exists());
        c.done();
    }

    /// A run that goes quiet is set aside: the card down, said, no other
    /// job on that archive until it lands, and its end said late.
    #[test]
    fn a_run_set_aside_holds_the_archive_until_it_lands_late() {
        let c = culled("aside");
        c.sheet();
        ASIDE_NEXT.with(|a| a.set(true));
        c.app.invoke_archive_rejects_answered(1);
        let first: Vec<_> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        for job in first {
            job(&c.state, &c.app, &c.worker);
        }
        assert!(!c.app.get_archive_running());
        assert!(
            c.app.get_status().contains("set aside"),
            "{}",
            c.app.get_status()
        );
        let held: Vec<_> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        c.app.invoke_archive_rejects_asked(0);
        assert!(
            c.app
                .get_status()
                .contains("has not come back from nas yet"),
            "{}",
            c.app.get_status()
        );
        assert_eq!(c.land(), 0);
        // It lands late: told to stop when set aside, it began nothing.
        SENT.with(|s| s.borrow_mut().extend(held));
        c.land();
        let status = c.app.get_status();
        assert!(
            status.starts_with("the job set aside earlier came back: nothing was moved on nas"),
            "{status}"
        );
        assert!(
            status.contains("canceled, 2 frames not reached"),
            "{status}"
        );
        assert!(
            status.contains("the 2 frames not reached are queued"),
            "{status}"
        );
        assert!(c.there.join("a.tif").is_file());
        let read = core::read_queue(&c.queue()).unwrap();
        assert_eq!(read.queue[0].frames.len(), 2);
        // The archive's next answer runs them.
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        assert_eq!(c.land(), 1);
        assert!(crate::cull::rejects_dir(&c.there).join("a.tif").is_file());
        assert!(!c.queue().exists());
        // And the archive is free again.
        c.sheet();
        c.app.invoke_archive_rejects_answered(0);
        c.done();
    }

    /// A queue run set aside, lost, or canceled is tried again at the
    /// archive's next answer; a sheet's own Cancel queues nothing.
    #[test]
    fn a_queue_run_stopped_short_is_tried_again() {
        let c = culled("again");
        queued(&c);
        // Set aside: it lands late having moved nothing; the queue stays.
        ASIDE_NEXT.with(|a| a.set(true));
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        c.land();
        assert!(c.there.join("a.tif").is_file());
        assert_eq!(
            core::read_queue(&c.queue()).unwrap().queue[0].frames.len(),
            2
        );
        assert!(!c.state.borrow().archive.rejects.tried.contains(&c.nas));
        // Lost: the same.
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        let token = c.state.borrow().archive.run_token;
        SENT.with(|s| s.borrow_mut().clear());
        land_run(&c.state, &c.app, &c.worker, token, Heard::Lost);
        assert!(!c.state.borrow().archive.rejects.tried.contains(&c.nas));
        // Canceled by the user: it holds. The passes that follow, or the
        // next poll, do not run it again.
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        super::super::cancel(&c.state.borrow(), &c.app);
        c.land();
        assert!(c.there.join("a.tif").is_file());
        assert!(c.state.borrow().archive.rejects.tried.contains(&c.nas));
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        assert_eq!(c.land(), 0);
        assert_eq!(
            core::read_queue(&c.queue()).unwrap().queue[0].frames.len(),
            2
        );
        // The archive away and back again: now it runs.
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, false);
        heard_from(&mut c.state.borrow_mut(), &c.app, &c.nas, true);
        assert_eq!(c.land(), 1);
        assert!(!c.queue().exists());
        c.done();
    }

    /// The user's Cancel, then the share hangs on the frame under way:
    /// set aside, it lands late, and the frames the user canceled are
    /// not queued.
    #[test]
    fn a_hand_cancel_then_set_aside_queues_nothing() {
        let c = culled("hand-then-aside");
        c.sheet();
        ASIDE_NEXT.with(|a| a.set(true));
        c.app.invoke_archive_rejects_answered(1);
        super::super::cancel(&c.state.borrow(), &c.app);
        c.land();
        let status = c.app.get_status();
        assert!(
            status.starts_with("the job set aside earlier came back"),
            "{status}"
        );
        assert!(!status.contains("queued"), "{status}");
        assert!(!c.queue().exists());
        assert!(c.there.join("a.tif").is_file());
        c.done();
    }

    /// A sheet's move the user canceled is not queued.
    #[test]
    fn a_sheet_move_canceled_by_hand_is_not_queued() {
        let c = culled("hand-cancel");
        c.sheet();
        c.app.invoke_archive_rejects_answered(1);
        super::super::cancel(&c.state.borrow(), &c.app);
        c.land();
        assert!(!c.queue().exists());
        c.done();
    }

    /// The buttons themselves, clicked by their labels: the CULLING
    /// section's entry, the red delete permanently, and the move.
    #[test]
    fn the_buttons_clicked_answer_as_they_say() {
        let c = culled("clicks");
        c.app.set_panel_tab("Cull".into());
        let click = |label: &str| {
            let (at, size) = crate::testing::labeled(&c.app, label);
            crate::testing::click(&c.app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        };
        c.state.borrow_mut().trash_refused.insert(c.there.clone());
        click("Remove rejects from nas...");
        assert_eq!(c.land(), 1);
        assert!(c.app.get_archive_rejects_open());
        click("Delete from nas permanently");
        assert!(!c.app.get_archive_rejects_open());
        assert_eq!(c.land(), 1);
        assert!(!c.there.join("a.tif").exists() && !c.there.join("b.tif").exists());
        assert!(
            c.app
                .get_status()
                .starts_with("on nas, deleted permanently"),
            "{}",
            c.app.get_status()
        );
        c.done();
    }

    #[test]
    fn the_move_button_clicked_moves() {
        let c = culled("click-move");
        c.app.set_panel_tab("Cull".into());
        c.sheet();
        let (at, size) = crate::testing::labeled(&c.app, "Move to rejects on nas");
        crate::testing::click(&c.app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(c.land(), 1);
        assert!(crate::cull::rejects_dir(&c.there).join("a.tif").is_file());
        c.done();
    }

    /// The indexer's words drive the queue: a pass skipped over the
    /// archive makes it due again, a pass landed over it runs it.
    #[test]
    fn the_indexers_words_drive_the_queue() {
        let c = culled("told");
        queued(&c);
        crate::STATE.with(|s| *s.borrow_mut() = Some(c.state.clone()));
        c.state
            .borrow_mut()
            .archive
            .rejects
            .tried
            .insert(c.nas.clone());
        crate::library::told(
            &c.app,
            crate::library::Told::Skipped {
                path: c.nas.clone(),
                root: c.nas.clone(),
                launch: false,
                generation: None,
                first: true,
            },
        );
        assert!(!c.state.borrow().archive.rejects.tried.contains(&c.nas));
        assert_eq!(c.land(), 0);
        crate::library::told(
            &c.app,
            crate::library::Told::Background {
                path: c.nas.join("clients"),
                launch: false,
                report: greycard_library::Report::default(),
                seconds: 0.0,
                error: None,
            },
        );
        assert!(c.app.get_archive_running(), "{}", c.app.get_status());
        assert_eq!(c.land(), 1);
        assert!(crate::cull::rejects_dir(&c.there).join("a.tif").is_file());
        crate::STATE.with(|s| *s.borrow_mut() = None);
        c.done();
    }

    /// A confirm refused because another job holds the card closes the
    /// sheet, rather than leave it up with the keys gone to the window.
    #[test]
    fn a_refused_answer_closes_the_sheet() {
        let c = culled("refused");
        c.sheet();
        {
            let mut st = c.state.borrow_mut();
            let items = vec![Item::queued(
                st.archive.rejects.look.as_ref().unwrap().items()[0]
                    .entry
                    .clone(),
            )];
            let nas = c.nas.clone();
            start(
                &mut st,
                &c.app,
                Kind::Queue,
                nas,
                "nas".into(),
                items,
                String::new(),
            );
        }
        c.app.set_archive_rejects_open(true);
        c.app.invoke_archive_rejects_answered(1);
        assert!(!c.app.get_archive_rejects_open());
        assert!(c.app.get_status().starts_with("nothing done: another job"));
        c.land();
        c.done();
    }

    /// Cancel through the card stops the run between frames; a run whose
    /// thread ends without a report lets go of the window.
    #[test]
    fn cancel_and_a_lost_run() {
        let c = culled("cancel");
        c.sheet();
        c.app.invoke_archive_rejects_answered(1);
        super::super::cancel(&c.state.borrow(), &c.app);
        assert!(
            c.app
                .get_archive_line()
                .contains("stopping after this frame")
        );
        c.land();
        assert!(
            c.app
                .get_status()
                .contains("canceled, 2 frames not reached"),
            "{}",
            c.app.get_status()
        );
        assert!(c.there.join("a.tif").is_file());

        c.sheet();
        c.app.invoke_archive_rejects_answered(1);
        let token = c.state.borrow().archive.run_token;
        SENT.with(|s| s.borrow_mut().clear());
        land_run(&c.state, &c.app, &c.worker, token, Heard::Lost);
        assert!(!c.app.get_archive_running());
        assert!(c.state.borrow().archive.running.is_none());
        assert!(c.app.get_status().contains("stopped without a report"));
        c.done();
    }

    /// Enter is the move and never a delete; Tab moves nothing, so the
    /// red buttons are a click's alone; Escape is the no.
    #[test]
    fn enter_is_only_ever_the_move_and_tab_reaches_no_delete() {
        let app = window(1);
        let answers = Rc::new(RefCell::new(Vec::new()));
        let seen = answers.clone();
        app.on_archive_rejects_answered(move |a| seen.borrow_mut().push(a));
        app.set_archive_rejects_move("Move to rejects on nas".into());
        app.set_archive_rejects_trash("Delete from nas (to the trash)".into());
        app.set_archive_rejects_permanent("Delete from nas permanently".into());
        app.set_archive_rejects_open(true);
        for _ in 0..4 {
            press(&app, Key::Tab);
            press(&app, Key::Backtab);
            press(&app, Key::Tab);
        }
        press(&app, " ");
        assert!(answers.borrow().is_empty(), "{:?}", answers.borrow());
        press(&app, Key::Return);
        assert_eq!(*answers.borrow(), [1]);
        // Nothing to move: Enter is nothing at all, on a sheet of its
        // own (a fresh window, so the keys are the sheet's and not the
        // window's, which the first answer gave them back to).
        let app = window(1);
        let answers = Rc::new(RefCell::new(Vec::new()));
        let seen = answers.clone();
        app.on_archive_rejects_answered(move |a| seen.borrow_mut().push(a));
        app.set_archive_rejects_trash("Delete from nas".into());
        app.set_archive_rejects_open(true);
        press(&app, Key::Tab);
        press(&app, Key::Return);
        press(&app, " ");
        assert!(answers.borrow().is_empty(), "{:?}", answers.borrow());
        press(&app, Key::Escape);
        assert_eq!(*answers.borrow(), [0]);
    }
}
