//! A Fill's strokes painted and not yet made. With the Fill in hand a
//! stroke is drawn over the picture and nothing is generated; Enter or
//! Generate, and nothing else, makes what is painted, as one step of
//! the history. Strokes that touch are one area; areas apart are fills
//! apart; with a Fill chosen in the list, the strokes that touch it are
//! added to it and it is made again. Esc lets what is painted go, and
//! an undo right after brings it back.
//!
//! What is painted waits on its frame for the session: the tool, the
//! mode or the frame left and come back to, it is there, drawn only
//! while the Fill is in hand on that frame. It is never in the edit,
//! the history or the sidecar, so an export, a quit or anything else
//! that reads the edit passes it by. A turn of its frame carries it
//! round as it does the patches, a file followed to a new path takes it
//! along, a swap with the original keeps it on the line it was painted
//! over, and a frame gone from the list takes it with it. Enter and Esc
//! reach it only on the Develop picture, where it is drawn.

use std::path::PathBuf;

use greycard_edit::Edit;
use greycard_edit::retouch::{Method as RetouchMethod, Patch, Stroke};
use slint::ComponentHandle;

use crate::panel::retouch::{PatchShape, patch_outlines, show_patches};
use crate::panel::viewport::{ViewMap, source_known};
use crate::{App, State};

/// How many frames keep strokes waiting at once: the oldest painted
/// goes past it, with a line in the log.
pub(crate) const FRAMES_KEPT: usize = 32;

/// A frame by its key, none for a window with no frame open.
type Frame = Option<crate::versions::Key>;

/// The strokes painted with the Fill in hand and not yet made, by
/// frame.
#[derive(Default)]
pub(crate) struct Painted {
    /// Each frame's, the least lately painted first.
    frames: Vec<(Frame, Vec<Brushed>)>,
    /// Whether the open frame's last stroke is still under the pointer.
    under_way: bool,
    /// What the last Esc let go, which an undo right after brings back.
    dropped: Option<Dropped>,
    /// The finished strokes' union, and the one under way, contoured.
    shape: Option<PatchShape>,
    under_way_shape: Option<PatchShape>,
}

/// A stroke painted, with the feather and the opacity the brush had: a
/// fill made of it takes its first stroke's.
#[derive(Clone)]
struct Brushed {
    stroke: Stroke,
    feather: f32,
    opacity: f32,
}

/// What an Esc let go: the frame's strokes, and the panel's edit and
/// the frame's history as they were, since only an undo with nothing
/// done since brings them back. An edit changed and undone back to the
/// same value has moved the history on, so it is the history that says.
struct Dropped {
    frame: Frame,
    strokes: Vec<Brushed>,
    edit: Edit,
    line: Option<greycard_edit::Line>,
}

/// The panel's edit as it stands, and the open frame's history: what an
/// Esc keeps and an undo compares.
fn as_now(st: &State, app: &App) -> (Edit, Option<greycard_edit::Line>) {
    let edit = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    let line = st.current.and_then(|c| st.line(c)).cloned();
    (edit, line)
}

impl Painted {
    fn of(&self, frame: &Frame) -> &[Brushed] {
        self.frames
            .iter()
            .find(|(f, _)| f == frame)
            .map_or(&[], |(_, s)| s.as_slice())
    }

    fn of_mut(&mut self, frame: &Frame) -> Option<&mut Vec<Brushed>> {
        self.frames
            .iter_mut()
            .find(|(f, _)| f == frame)
            .map(|(_, s)| s)
    }

    fn take(&mut self, frame: &Frame) -> Vec<Brushed> {
        match self.frames.iter().position(|(f, _)| f == frame) {
            Some(i) => self.frames.remove(i).1,
            None => Vec::new(),
        }
    }
}

/// The open frame.
fn here(st: &State) -> Frame {
    st.current.and_then(|c| st.key_of(c))
}

/// Whether the Fill is in hand, so a press paints rather than repairs.
pub(crate) fn painting(st: &State) -> bool {
    st.develop.retouching == Some(RetouchMethod::Fill)
}

/// Whether strokes wait on the open frame with the Fill in hand: what
/// lights Generate and gives Enter and Esc to it.
pub(crate) fn waiting(st: &State) -> bool {
    painting(st) && !st.develop.painted.of(&here(st)).is_empty()
}

/// How many strokes wait on the open frame.
#[cfg(test)]
pub(crate) fn len(st: &State) -> usize {
    st.develop.painted.of(&here(st)).len()
}

/// How many strokes wait on frame `key`.
#[cfg(test)]
pub(crate) fn waiting_at(st: &State, key: &crate::versions::Key) -> usize {
    st.develop.painted.of(&Some(key.clone())).len()
}

/// The window told whether strokes wait here, and redrawn.
pub(crate) fn show(st: &State, app: &App) {
    let waiting = waiting(st);
    if app.get_fill_painted() != waiting {
        app.set_fill_painted(waiting);
    }
    app.window().request_redraw();
}

/// A press with the Fill in hand at `at`, in the picture's units: a
/// stroke begun on the open frame, at the brush's size, feather and
/// opacity.
pub(crate) fn pressed(st: &mut State, app: &App, at: (f32, f32)) {
    let frame = here(st);
    let painted = &mut st.develop.painted;
    painted.dropped = None;
    // The frame painted last goes to the end; past the bound, the
    // frame painted longest ago lets its strokes go.
    let mine = painted.take(&frame);
    painted.frames.push((frame, mine));
    while painted.frames.len() > FRAMES_KEPT {
        let (gone, strokes) = painted.frames.remove(0);
        tracing::info!(
            "fill: {} strokes waiting on {} let go, {FRAMES_KEPT} frames having strokes since",
            strokes.len(),
            gone.as_ref()
                .map_or_else(String::new, |k| k.path.display().to_string())
        );
    }
    if let Some((_, strokes)) = painted.frames.last_mut() {
        strokes.push(Brushed {
            stroke: Stroke {
                points: vec![[at.0, at.1]],
                radius: app.get_patch_size().clamp(0.002, 0.3),
            },
            feather: app.get_patch_feather().clamp(0.0, 1.0),
            opacity: app.get_patch_opacity().clamp(0.0, 1.0),
        });
    }
    painted.under_way = true;
    show(st, app);
}

/// The pointer moved with a stroke under way: a point once it has gone
/// as far as a repair's stroke takes one. False with none under way.
pub(crate) fn dragged(st: &mut State, at: (f32, f32), step: f32) -> bool {
    if !st.develop.painted.under_way {
        return false;
    }
    let frame = here(st);
    if let Some(b) = st.develop.painted.of_mut(&frame).and_then(|s| s.last_mut())
        && b.stroke
            .points
            .last()
            .is_none_or(|l| (at.0 - l[0]).hypot(at.1 - l[1]) > step)
    {
        b.stroke.points.push([at.0, at.1]);
    }
    true
}

/// The stroke under way finished: it stays painted. False with none.
pub(crate) fn released(st: &mut State, app: &App) -> bool {
    if !std::mem::take(&mut st.develop.painted.under_way) {
        return false;
    }
    show(st, app);
    true
}

/// Esc: what is painted on the open frame let go, the stroke under way
/// with it, and kept for an undo right after; the tool stays in hand.
pub(crate) fn discard(st: &mut State, app: &App) {
    let frame = here(st);
    let (edit, line) = as_now(st, app);
    let painted = &mut st.develop.painted;
    painted.under_way = false;
    let strokes = painted.take(&frame);
    if !strokes.is_empty() {
        painted.dropped = Some(Dropped {
            frame,
            strokes,
            edit,
            line,
        });
    }
    show(st, app);
}

/// The keys' undo with the Fill in hand on the Develop picture: a stroke under the pointer
/// keeps it from doing anything; with strokes waiting, the last goes;
/// with none, what the last Esc let go comes back if neither the edit
/// nor its history has moved since. False when the undo is the edit's.
pub(crate) fn undo_last(st: &mut State, app: &App) -> bool {
    // Only where the strokes are drawn, by the window's one rule for
    // the Fill's keys: in the grid, the lightbox, the crop, or culling
    // the undo is the view's own.
    if !painting(st) || !app.get_fill_view() {
        return false;
    }
    if st.develop.painted.under_way {
        return true;
    }
    let frame = here(st);
    let (edit_now, line_now) = as_now(st, app);
    let painted = &mut st.develop.painted;
    if let Some(strokes) = painted.of_mut(&frame)
        && !strokes.is_empty()
    {
        strokes.pop();
        if strokes.is_empty() {
            painted.take(&frame);
        }
    } else if painted
        .dropped
        .as_ref()
        .is_some_and(|d| d.frame == frame && d.edit == edit_now && d.line == line_now)
    {
        let dropped = painted.dropped.take().expect("just seen");
        painted.frames.push((frame, dropped.strokes));
    } else {
        return false;
    }
    show(st, app);
    true
}

/// The frame of `path` turned: the strokes waiting on any of its
/// versions carried round by `t`, the map the turn carried the
/// sidecar's patches by ([`greycard_edit::Sidecar::turn_by`]), so the
/// two stay together. A frame nothing could measure, with patches, is
/// not turned, and neither are its strokes.
pub(crate) fn turn(st: &mut State, path: &std::path::Path, t: greycard_edit::mask::Turned) {
    let painted = &mut st.develop.painted;
    let on = |f: &Frame| f.as_ref().is_some_and(|k| k.path == path);
    let dropped = painted
        .dropped
        .as_mut()
        .filter(|d| on(&d.frame))
        .map(|d| &mut d.strokes);
    for strokes in painted
        .frames
        .iter_mut()
        .filter(|(f, _)| on(f))
        .map(|(_, s)| s)
        .chain(dropped)
    {
        for b in strokes {
            b.stroke.turn(t);
        }
    }
}

/// The files at `paths` gone from the list, deleted or moved away: the
/// strokes waiting on them go too, with a line in the log.
pub(crate) fn drop_files(st: &mut State, paths: &[PathBuf]) {
    if !paths.is_empty() {
        let_go(st, |k| paths.contains(&k.path));
    }
}

/// The frame `key` gone (a version deleted): its strokes with it.
pub(crate) fn drop_frame(st: &mut State, key: &crate::versions::Key) {
    let_go(st, |k| k == key);
}

/// The file at `was` followed to `to`: the strokes waiting on any of its
/// frames follow it. Strokes still kept at `to` were a file's gone from
/// there, and go.
pub(crate) fn file_moved(st: &mut State, was: &std::path::Path, to: &std::path::Path) {
    if was == to {
        return;
    }
    let_go(st, |k| k.path == to);
    let painted = &mut st.develop.painted;
    let dropped = painted.dropped.as_mut().map(|d| &mut d.frame);
    for k in painted
        .frames
        .iter_mut()
        .map(|(f, _)| f)
        .chain(dropped)
        .flatten()
        .filter(|k| k.path == was)
    {
        k.path = to.to_path_buf();
    }
}

/// Version `id` of the file at `path` swapped with the original, their
/// lines exchanged whole: the strokes go with the line they were painted
/// over, the version's to the original and the original's to it.
pub(crate) fn swapped(st: &mut State, path: &std::path::Path, id: &str) {
    let painted = &mut st.develop.painted;
    let dropped = painted.dropped.as_mut().map(|d| &mut d.frame);
    for k in painted
        .frames
        .iter_mut()
        .map(|(f, _)| f)
        .chain(dropped)
        .flatten()
        .filter(|k| k.path == path)
    {
        k.version = match k.version.as_deref() {
            None => Some(std::sync::Arc::from(id)),
            Some(v) if v == id => None,
            Some(_) => continue,
        };
    }
}

/// The strokes waiting on every frame `gone` says, and what an Esc let
/// go there, let go, with a line in the log.
fn let_go(st: &mut State, gone: impl Fn(&crate::versions::Key) -> bool) {
    let painted = &mut st.develop.painted;
    let on = |f: &Frame| f.as_ref().is_some_and(&gone);
    painted.frames.retain(|(f, strokes)| {
        let keep = !on(f);
        if !keep {
            tracing::info!(
                "fill: {} strokes waiting on {} let go with the frame",
                strokes.len(),
                f.as_ref()
                    .map_or_else(String::new, |k| k.path.display().to_string())
            );
        }
        keep
    });
    if painted.dropped.as_ref().is_some_and(|d| on(&d.frame)) {
        painted.dropped = None;
    }
}

/// Which of `strokes` join the repair whose strokes are `target`, and
/// the rest in their areas: strokes whose covers overlap
/// ([`Stroke::touches`]) are one area, as are strokes that reach the
/// target through others. Each area is its strokes in the order they
/// were painted, the areas in the order of their first strokes.
pub(crate) fn split(
    target: Option<&[Stroke]>,
    strokes: &[Stroke],
) -> (Vec<usize>, Vec<Vec<usize>>) {
    let n = strokes.len();
    // `n` stands for the target.
    let mut parent: Vec<usize> = (0..=n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let union = |parent: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    };
    for i in 0..n {
        for j in i + 1..n {
            if strokes[i].touches(&strokes[j]) {
                union(&mut parent, i, j);
            }
        }
        if target.is_some_and(|t| t.iter().any(|s| s.touches(&strokes[i]))) {
            union(&mut parent, i, n);
        }
    }
    let to_target = find(&mut parent, n);
    let mut joined = Vec::new();
    let mut areas: Vec<(usize, Vec<usize>)> = Vec::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        if target.is_some() && root == to_target {
            joined.push(i);
        } else if let Some(area) = areas.iter_mut().find(|(r, _)| *r == root) {
            area.1.push(i);
        } else {
            areas.push((root, vec![i]));
        }
    }
    (joined, areas.into_iter().map(|(_, a)| a).collect())
}

/// What waits on the open frame put into the edit: added to the Fill
/// chosen in the list where it touches it, and each area apart a Fill
/// of its own, after every repair there is. Nothing is saved or
/// developed; true when the edit changed.
fn commit(st: &mut State, app: &App) -> bool {
    let frame = here(st);
    let painted = &mut st.develop.painted;
    painted.under_way = false;
    painted.dropped = None;
    let brushed = painted.take(&frame);
    show(st, app);
    if brushed.is_empty() {
        return false;
    }
    let strokes: Vec<Stroke> = brushed.iter().map(|b| b.stroke.clone()).collect();
    let chosen = usize::try_from(app.get_patch()).ok().filter(|&i| {
        st.develop
            .edit
            .retouch
            .patches
            .get(i)
            .is_some_and(|p| p.method == RetouchMethod::Fill)
    });
    let target = chosen.map(|i| st.develop.edit.retouch.patches[i].stroke_list());
    let (joined, areas) = split(target.as_deref(), &strokes);
    if let Some(i) = chosen
        && !joined.is_empty()
    {
        let patch = &mut st.develop.edit.retouch.patches[i];
        let mut all = patch.stroke_list();
        all.extend(joined.iter().map(|&k| strokes[k].clone()));
        patch.set_strokes(all);
    }
    // Made by the choice in hand where it can run, as a stroke was.
    let choice = crate::panel::retouch::drawn_fill(st);
    for area in areas {
        let first = &brushed[area[0]];
        let mut patch = Patch {
            id: st.develop.edit.retouch.next_id(),
            method: RetouchMethod::Fill,
            feather: first.feather,
            opacity: first.opacity,
            ..Default::default()
        };
        patch.set_strokes(area.iter().map(|&k| strokes[k].clone()).collect());
        patch.set_fill_choice(choice);
        st.develop.edit.retouch.patches.push(patch);
    }
    show_patches(st, &st.develop.edit, app);
    true
}

/// Generate, by Enter or its button and nothing else: what waits on the
/// open frame put in its edit as one step of the history, written, and
/// the frame developed, which makes the fills. Whatever the panel held
/// before is a state of its own first. True when anything was painted.
pub(crate) fn generate(st: &mut State, app: &App) -> bool {
    if !waiting(st) {
        return false;
    }
    crate::panel::edit::record_panel(st, app);
    if !commit(st, app) {
        return false;
    }
    let edit = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    crate::panel::edit::save_edit(st, edit);
    crate::panel::history::show_history(st, app);
    let state = crate::STATE.with(|s| s.borrow().clone());
    let worker = crate::WORKER.with(|w| w.borrow().clone());
    if let (Some(state), Some(worker)) = (state, worker) {
        crate::panel::edit::develop_soon(st, state, worker, app.as_weak());
    }
    true
}

/// What waits on the open frame, on the view while the Fill is in hand
/// on the Retouch tab: the finished strokes' union and the stroke under
/// way, each its edge and where its feather begins, drawn as a repair
/// being drawn is.
pub(crate) fn outlines(st: &mut State, app: &App) -> (String, String) {
    if app.get_panel_tab() != "Retouch" || !waiting(st) || !source_known(st) {
        return (String::new(), String::new());
    }
    let map = ViewMap::of(st, app);
    let frame = here(st);
    let painted = &mut st.develop.painted;
    let strokes: Vec<Brushed> = painted.of(&frame).to_vec();
    let as_patch = |list: &[Brushed]| {
        let mut patch = Patch {
            feather: list[0].feather,
            ..Default::default()
        };
        patch.set_strokes(list.iter().map(|b| b.stroke.clone()).collect());
        patch
    };
    let done = strokes.len() - usize::from(painted.under_way);
    let (mut edge, mut feather) = (String::new(), String::new());
    if done > 0 {
        let (e, f) = patch_outlines(&mut painted.shape, &as_patch(&strokes[..done]), &map);
        edge.push_str(&e);
        feather.push_str(&f);
    }
    if painted.under_way && done < strokes.len() {
        let (e, f) = patch_outlines(
            &mut painted.under_way_shape,
            &as_patch(&strokes[done..]),
            &map,
        );
        edge.push_str(&e);
        feather.push_str(&f);
    }
    (edge, feather)
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use greycard_edit::retouch::{Method as RetouchMethod, Patch, Stroke};
    use slint::platform::Key;
    use slint::{ComponentHandle, Model};

    use crate::testing::{by_id, folder, press, state_for, window};
    use crate::worker::Worker;
    use crate::{App, Shared};

    /// A window on `n` frames, the first open, its source's size known,
    /// the Retouch tab up and the Fill in hand, the brush 0.05 wide.
    fn painting(n: usize) -> (App, Rc<Shared>, Rc<Worker>) {
        let app = window(n);
        let (state, worker) = state_for(&app, folder(n));
        app.invoke_select(0);
        hand(&app, &state);
        (app, state, worker)
    }

    /// The Fill in hand on the Retouch tab, nothing chosen, the brush
    /// 0.05 wide, no develop waiting.
    fn hand(app: &App, state: &Rc<Shared>) {
        let mut st = state.borrow_mut();
        st.develop.source_size = (400, 300);
        st.develop.image_size = (400, 300);
        st.develop.zoom = 1.0;
        st.develop.retouching = Some(RetouchMethod::Fill);
        st.develop.debounce.stop();
        app.set_retouch_mode("Fill".into());
        app.set_panel_tab("Retouch".into());
        app.set_patch(-1);
        app.set_patch_size(0.05);
    }

    /// [`hand`], leaving the list's choice as it is.
    fn hand_keeping_choice(app: &App, state: &Rc<Shared>) {
        let chosen = app.get_patch();
        hand(app, state);
        app.set_patch(chosen);
    }

    /// A stroke through `points`, in the picture's units.
    fn paint(app: &App, state: &Rc<Shared>, points: &[(f32, f32)]) {
        let mut st = state.borrow_mut();
        super::pressed(&mut st, app, points[0]);
        for &p in &points[1..] {
            super::dragged(&mut st, p, 0.0);
        }
        super::released(&mut st, app);
    }

    fn patches(state: &Rc<Shared>) -> Vec<Patch> {
        state.borrow().develop.edit.retouch.patches.clone()
    }

    /// How many strokes wait on the open frame.
    fn painted(state: &Rc<Shared>) -> usize {
        super::len(&state.borrow())
    }

    /// The quit's saves, as `startup::main` makes them.
    fn quit(app: &App, state: &Rc<Shared>) {
        let mut st = state.borrow_mut();
        let edit = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
        crate::panel::startup::save_at_quit(&mut st, edit, crate::sync::QUIT_WRITE_WAIT);
    }

    /// Whether a develop is asked.
    fn asked(state: &Rc<Shared>) -> bool {
        state.borrow().develop.debounce.running()
    }

    fn states(state: &Rc<Shared>) -> usize {
        state.borrow().sidecars[0].states()
    }

    /// The panel saved, as its timer saves it.
    fn save(app: &App, state: &Rc<Shared>) {
        let mut st = state.borrow_mut();
        let edit = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(&mut st, edit);
    }

    fn fill(id: u64, u: f32) -> Patch {
        Patch {
            id,
            method: RetouchMethod::Fill,
            points: vec![[u, 0.5]],
            radius: 0.05,
            ..Default::default()
        }
    }

    /// With the Fill in hand strokes are painted and drawn, and are
    /// neither put in the edit nor developed, nor saved with the
    /// panel; Enter makes them, and asks the develop that fills them.
    #[test]
    fn strokes_wait_for_enter() {
        let (app, state, _worker) = painting(1);
        let before = states(&state);
        app.invoke_place_pressed(100.0, 100.0, false);
        app.invoke_place_dragged(140.0, 100.0);
        app.invoke_place_released();
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        assert_eq!(painted(&state), 2);
        assert!(app.get_fill_painted());
        assert!(patches(&state).is_empty(), "nothing in the edit");
        assert!(!asked(&state), "no develop asked");
        assert_eq!(app.get_retouch_mode(), "Fill");
        // Drawn as a repair being drawn is.
        let (edge, feather) = {
            let mut st = state.borrow_mut();
            let edit = st.develop.edit.clone();
            crate::panel::retouch::shown_outlines(&mut st, &app, &edit)
        };
        assert!(!edge.is_empty() && !feather.is_empty());
        // Nothing painted reaches the sidecar with the panel.
        save(&app, &state);
        assert_eq!(states(&state), before);
        assert!(
            state.borrow().sidecars[0]
                .current()
                .retouch
                .patches
                .is_empty()
        );
        assert!(!state.borrow().sidecars[0].to_json().contains("\"points\""));

        press(&app, Key::Return);
        assert_eq!(painted(&state), 0);
        assert!(!app.get_fill_painted());
        let made = patches(&state);
        assert_eq!(made.len(), 2, "two strokes apart, two fills");
        assert!(made.iter().all(|p| p.method == RetouchMethod::Fill));
        assert!(asked(&state), "the develop that makes them");
        assert_eq!(app.get_retouch_mode(), "Fill", "the tool stays in hand");
        assert_eq!(states(&state), before + 1);
        assert_eq!(state.borrow().sidecars[0].current().retouch.patches, made);
        // A second Enter with nothing painted makes nothing more.
        press(&app, Key::Return);
        assert_eq!(patches(&state), made);
    }

    /// Generate is live only with strokes painted, and does as Enter.
    #[test]
    fn the_generate_button_does_as_enter_and_only_with_strokes_painted() {
        let click = |app: &App| {
            let (p, size) = by_id(app, "RetouchSection::generate-button");
            crate::testing::click(app, p.x + size.width / 2.0, p.y + size.height / 2.0);
        };
        let (app, state, _worker) = painting(1);
        click(&app);
        assert!(patches(&state).is_empty());
        assert!(!asked(&state), "greyed, it does nothing");
        hand(&app, &state);
        paint(&app, &state, &[(0.2, 0.5), (0.3, 0.5)]);
        paint(&app, &state, &[(0.7, 0.5)]);
        click(&app);
        assert_eq!(painted(&state), 0);
        assert!(asked(&state));
        let by_button = patches(&state);
        assert_eq!(by_button.len(), 2);

        let (app2, state2, _worker2) = painting(1);
        paint(&app2, &state2, &[(0.2, 0.5), (0.3, 0.5)]);
        paint(&app2, &state2, &[(0.7, 0.5)]);
        press(&app2, Key::Return);
        assert_eq!(patches(&state2), by_button);
    }

    /// Esc lets what is painted go and keeps the Fill in hand; a second
    /// Esc, with nothing painted, puts it down.
    #[test]
    fn esc_lets_the_strokes_go_then_puts_the_fill_down() {
        let (app, state, _worker) = painting(1);
        paint(&app, &state, &[(0.2, 0.5)]);
        paint(&app, &state, &[(0.6, 0.5)]);
        press(&app, Key::Escape);
        assert_eq!(painted(&state), 0);
        assert!(!app.get_fill_painted());
        assert_eq!(app.get_retouch_mode(), "Fill");
        assert!(patches(&state).is_empty());
        press(&app, Key::Escape);
        assert_eq!(app.get_retouch_mode(), "");
        assert!(patches(&state).is_empty(), "nothing made by the second");
        // Mid-stroke, Esc lets the stroke under way go too, and the
        // release after it paints nothing.
        hand(&app, &state);
        app.invoke_place_pressed(100.0, 100.0, false);
        app.invoke_place_dragged(150.0, 100.0);
        press(&app, Key::Escape);
        app.invoke_place_released();
        assert_eq!(painted(&state), 0);
        assert!(patches(&state).is_empty());
        assert_eq!(app.get_retouch_mode(), "Fill");
    }

    /// Strokes waiting on frame `cell`, whatever is open.
    fn waiting_on(state: &Rc<Shared>, cell: usize) -> usize {
        let st = state.borrow();
        st.develop.painted.of(&st.key_of(cell)).len()
    }

    /// Esc's strokes come back with the undo right after it, once; not
    /// after the edit has moved on.
    #[test]
    fn an_undo_right_after_esc_brings_the_strokes_back() {
        let (app, state, _worker) = painting(1);
        paint(&app, &state, &[(0.2, 0.5)]);
        paint(&app, &state, &[(0.6, 0.5)]);
        press(&app, Key::Escape);
        assert_eq!(painted(&state), 0);
        app.invoke_undo();
        assert_eq!(painted(&state), 2, "back");
        assert!(app.get_fill_painted());
        assert!(patches(&state).is_empty());
        // Gone again, and the edit moved on: the undo is the edit's.
        press(&app, Key::Escape);
        state.borrow_mut().develop.edit.retouch.patches = vec![fill(9, 0.9)];
        save(&app, &state);
        app.invoke_undo();
        assert_eq!(painted(&state), 0, "not back over a change");
        assert!(patches(&state).is_empty(), "the edit's own undo");
    }

    /// No way out of the session but Enter and Generate makes anything:
    /// the strokes wait on their frame, the edit and its line are as
    /// they were, and nothing is developed for them.
    #[test]
    fn no_way_out_makes_or_writes_anything() {
        type Out = fn(&App, &Rc<Shared>);
        let outs: [(&str, Out); 13] = [
            ("the Fill's button", |app, _| {
                app.invoke_retouch_toggled("Fill".into())
            }),
            ("Heal", |app, _| app.invoke_retouch_toggled("Heal".into())),
            ("Clone", |app, _| app.invoke_retouch_toggled("Clone".into())),
            ("another tab", |app, _| {
                app.invoke_tab_changed("Develop".into())
            }),
            ("the crop", |app, _| {
                app.set_crop_mode(true);
                app.invoke_crop_toggled();
            }),
            ("R", |app, _| press(app, "r")),
            ("the grid", |app, _| {
                app.set_grid_open(true);
                slint::platform::update_timers_and_animations();
                app.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(1.0, 1.0),
                    });
            }),
            ("culling", |app, state| {
                crate::panel::cull::enter_cull(&mut state.borrow_mut(), app, 1)
            }),
            ("Cull drawn developed", |app, state| {
                crate::panel::cull::enter_developed(&mut state.borrow_mut(), app)
            }),
            ("another frame", |app, _| app.invoke_select(1)),
            ("the quit", quit),
            ("an export", |app, _| app.invoke_export()),
            ("the export queue", |app, _| app.invoke_export_queue_add()),
        ];
        for (how, out) in outs {
            let (app, state, _worker) = painting(2);
            let line = state.borrow().sidecars[0].clone();
            paint(&app, &state, &[(0.2, 0.5), (0.3, 0.5)]);
            paint(&app, &state, &[(0.7, 0.5)]);
            out(&app, &state);
            assert_eq!(waiting_on(&state, 0), 2, "{how}: still waiting");
            assert!(patches(&state).is_empty(), "{how}: nothing in the edit");
            let st = state.borrow();
            let now = &st.sidecars[0];
            assert_eq!(now.current(), line.current(), "{how}: the line as it was");
            assert_eq!(now.states(), line.states(), "{how}: no step");
            assert!(!st.develop.debounce.running(), "{how}: nothing developed");
        }
    }

    /// The tool, a mode, the tab or the frame left and come back to:
    /// the strokes are there, drawn and ready, with nothing made.
    #[test]
    fn strokes_wait_through_a_trip_away_and_back() {
        type Away = fn(&App, &Rc<Shared>, &Rc<Worker>);
        let trips: [(&str, Away); 5] = [
            ("the tool", |app, _, _| {
                app.invoke_retouch_toggled("Fill".into());
                assert!(!app.get_fill_painted(), "unseen with the tool down");
            }),
            ("the tab", |app, _, _| {
                app.invoke_tab_changed("Develop".into());
                app.set_panel_tab("Retouch".into());
            }),
            ("the crop", |app, _, _| {
                app.set_crop_mode(true);
                app.invoke_crop_toggled();
                app.set_crop_mode(false);
                app.invoke_crop_toggled();
            }),
            ("culling", |app, state, worker| {
                crate::panel::cull::enter_cull(&mut state.borrow_mut(), app, 1);
                crate::panel::cull::leave_cull(&mut state.borrow_mut(), app, worker, None);
            }),
            ("the frame", |app, state, _| {
                app.invoke_select(1);
                state.borrow_mut().develop.retouching = Some(RetouchMethod::Fill);
                app.invoke_patch_changed(-1);
                assert!(!app.get_fill_painted(), "not on another frame");
                let (edge, _) = {
                    let mut st = state.borrow_mut();
                    st.develop.source_size = (400, 300);
                    let edit = st.develop.edit.clone();
                    crate::panel::retouch::shown_outlines(&mut st, app, &edit)
                };
                assert!(edge.is_empty(), "not drawn on another frame");
                press(app, Key::Return);
                assert!(
                    state.borrow().develop.edit.retouch.patches.is_empty(),
                    "nor made there"
                );
                app.invoke_select(0);
            }),
        ];
        for (how, away) in trips {
            let (app, state, worker) = painting(2);
            paint(&app, &state, &[(0.2, 0.5)]);
            paint(&app, &state, &[(0.7, 0.5)]);
            away(&app, &state, &worker);
            hand(&app, &state);
            app.invoke_patch_changed(-1);
            assert_eq!(painted(&state), 2, "{how}");
            assert!(app.get_fill_painted(), "{how}: Generate lit");
            assert!(patches(&state).is_empty(), "{how}: nothing made");
            press(&app, Key::Return);
            assert_eq!(patches(&state).len(), 2, "{how}: made at Enter");
        }
    }

    /// A frame deleted or moved away takes its strokes with it; they are
    /// never shown or made on the frame that takes its place.
    #[test]
    fn a_frame_gone_takes_its_strokes() {
        let (app, state, worker) = painting(3);
        paint(&app, &state, &[(0.2, 0.5)]);
        crate::panel::cull::drop_files(&mut state.borrow_mut(), &app, &worker, &[0], "gone".into());
        hand(&app, &state);
        app.invoke_patch_changed(-1);
        assert_eq!(painted(&state), 0);
        assert!(!app.get_fill_painted());
        assert!(state.borrow().develop.painted.frames.is_empty());
    }

    /// Strokes wait on at most `FRAMES_KEPT` frames: the one painted
    /// longest ago goes first.
    #[test]
    fn strokes_wait_on_so_many_frames() {
        let n = super::FRAMES_KEPT + 1;
        let (app, state, _worker) = painting(n);
        for i in 0..n {
            app.invoke_select(i as i32);
            hand(&app, &state);
            paint(&app, &state, &[(0.2, 0.5)]);
        }
        assert_eq!(waiting_on(&state, 0), 0, "the oldest let go");
        assert_eq!(waiting_on(&state, 1), 1);
        assert_eq!(waiting_on(&state, n - 1), 1);
    }

    /// A turn of the frame carries the strokes waiting on it round as it
    /// carries its patches, so the fill lands where they were painted.
    #[test]
    fn a_turn_carries_the_strokes_with_the_patches() {
        let (app, state, worker) = painting(1);
        let mut on = fill(1, 0.2);
        on.points = vec![[0.2, 0.3], [0.25, 0.32]];
        state.borrow_mut().develop.edit.retouch.patches = vec![on];
        save(&app, &state);
        paint(&app, &state, &[(0.2, 0.3), (0.25, 0.32)]);
        crate::panel::browser::turn_frames(&mut state.borrow_mut(), &app, &worker, &[0], 1);
        let patch = patches(&state)[0].clone();
        assert_ne!(patch.points[0], [0.2, 0.3], "the patch turned");
        let st = state.borrow();
        let waiting = &st.develop.painted.of(&st.key_of(0))[0].stroke;
        assert_eq!(waiting.points, patch.points, "the strokes with it");
        assert_eq!(waiting.radius, patch.radius);
    }

    /// Enter in a number field, or on a slider with the keys, is
    /// theirs: it makes nothing.
    #[test]
    fn enter_in_a_number_field_makes_nothing() {
        let (app, state, _worker) = painting(1);
        paint(&app, &state, &[(0.2, 0.5)]);
        let (p, size) = crate::testing::by_id(&app, "RetouchSection::size-slider");
        // The value: its field, typed in.
        crate::testing::click(&app, p.x + size.width - 20.0, p.y + size.height / 2.0);
        press(&app, Key::Return);
        assert!(patches(&state).is_empty(), "the field's Enter");
        // The label: the slider takes the keys.
        crate::testing::click(&app, p.x + 8.0, p.y + size.height / 2.0);
        press(&app, Key::Return);
        assert!(patches(&state).is_empty(), "the slider's Enter");
        assert_eq!(painted(&state), 1);
    }

    /// With the Fill in hand and a Fill chosen, the wheel, the brush
    /// keys and the sliders set the brush and never the chosen Fill, and
    /// start nothing; with no tool in hand its sliders resize it, and it
    /// is made again when the slider is let go, or at once for a value
    /// typed.
    #[test]
    fn a_chosen_fill_is_resized_only_with_no_tool_and_on_release() {
        let (app, state, _worker) = painting(1);
        state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.3)];
        save(&app, &state);
        app.invoke_patch_changed(0);
        hand_keeping_choice(&app, &state);
        app.set_patch_size(0.1);
        app.invoke_patch_edited();
        press(&app, "]");
        assert_eq!(patches(&state), [fill(1, 0.3)], "the chosen Fill as it was");
        assert!(!asked(&state), "no notch or key starts a fill");
        paint(&app, &state, &[(0.8, 0.5)]);
        let radius = state.borrow().develop.painted.frames[0].1[0].stroke.radius;
        assert!(radius > 0.1, "the brush took the size: {radius}");

        // The tool down: its sliders are the Fill's.
        press(&app, Key::Escape);
        app.invoke_retouch_toggled("Fill".into());
        assert_eq!(app.get_retouch_mode(), "");
        app.invoke_patch_changed(0);
        assert_eq!(app.get_patch_size(), 0.05, "its own size shown");
        app.set_patch_sliding(true);
        app.set_patch_size(0.08);
        app.invoke_patch_edited();
        assert_eq!(patches(&state)[0].radius, 0.08);
        assert!(!asked(&state), "not while the slider is held");
        app.set_patch_sliding(false);
        app.invoke_patch_settled();
        assert!(asked(&state), "made again at the release");
        state.borrow_mut().develop.debounce.stop();
        app.invoke_patch_settled();
        assert!(!asked(&state), "once");
        app.set_patch_size(0.06);
        app.invoke_patch_edited();
        assert!(asked(&state), "a typed value at once");
    }

    /// The chosen Fill's center pin still moves it; a stroke started just
    /// beside the pin paints.
    #[test]
    fn a_stroke_beside_the_pin_paints() {
        use crate::Pt;
        use slint::{ModelRc, VecModel};
        let (app, state, _worker) = painting(1);
        let base =
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
                .next()
                .expect("the viewport is on screen")
                .absolute_position();
        app.set_patch_handles(ModelRc::new(VecModel::from(vec![Pt {
            x: 500.0,
            y: 300.0,
        }])));
        let grabbed = Rc::new(std::cell::Cell::new(false));
        {
            let grabbed = grabbed.clone();
            app.on_patch_grabbed(move |_| grabbed.set(true));
        }
        crate::testing::click(&app, base.x + 500.0, base.y + 300.0);
        assert!(grabbed.get(), "the pin");
        assert_eq!(painted(&state), 0);
        grabbed.set(false);
        crate::testing::drag(
            &app,
            &[
                (base.x + 514.0, base.y + 300.0),
                (base.x + 540.0, base.y + 300.0),
            ],
        );
        assert!(!grabbed.get());
        assert_eq!(painted(&state), 1, "a stroke beside it paints");
    }

    /// The undo key mid-stroke does nothing: not the stroke, and not the
    /// edit's last step.
    #[test]
    fn an_undo_mid_stroke_does_nothing() {
        let (app, state, _worker) = painting(1);
        state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.3)];
        save(&app, &state);
        app.invoke_place_pressed(100.0, 100.0, false);
        app.invoke_undo();
        assert_eq!(patches(&state), [fill(1, 0.3)], "the edit's step kept");
        app.invoke_place_released();
        assert_eq!(painted(&state), 1, "the stroke kept");
    }

    /// With a Fill chosen in the list, the strokes that touch it, or
    /// touch it through others, are added to it and it is made again;
    /// a stroke apart is a Fill of its own. It stays chosen.
    #[test]
    fn a_chosen_fill_grows_and_is_made_again() {
        let (app, state, _worker) = painting(1);
        state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.3)];
        save(&app, &state);
        app.invoke_patch_changed(0);
        assert_eq!(app.get_patch(), 0);
        hand_keeping_choice(&app, &state);
        paint(&app, &state, &[(0.36, 0.5)]);
        paint(&app, &state, &[(0.44, 0.5)]);
        paint(&app, &state, &[(0.8, 0.5)]);
        assert_eq!(app.get_patch(), 0, "chosen while painting");
        press(&app, Key::Return);
        let made = patches(&state);
        assert_eq!(made.len(), 2);
        assert_eq!(made[0].id, 1);
        assert_eq!(made[0].points, vec![[0.3, 0.5]]);
        let more: Vec<Vec<[f32; 2]>> = made[0].more.iter().map(|s| s.points.clone()).collect();
        assert_eq!(more, [vec![[0.36, 0.5]], vec![[0.44, 0.5]]]);
        assert_eq!(made[1].points, vec![[0.8, 0.5]]);
        assert!(made[1].more.is_empty());
        assert_ne!(made[1].id, 1);
        assert_eq!(app.get_patch(), 0, "still chosen");
        assert_eq!(app.get_patch_names().row_count(), 2);
        assert!(asked(&state), "made again");
    }

    /// With nothing chosen, strokes over a Fill make a new one on top of
    /// it, later in the order; the old one is left as it was.
    #[test]
    fn with_nothing_chosen_strokes_over_a_fill_make_a_new_one() {
        let (app, state, _worker) = painting(1);
        state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.3)];
        save(&app, &state);
        paint(&app, &state, &[(0.32, 0.5)]);
        press(&app, Key::Return);
        let made = patches(&state);
        assert_eq!(made.len(), 2);
        assert_eq!(made[0], fill(1, 0.3));
        assert_eq!(made[1].points, vec![[0.32, 0.5]]);
        assert_eq!(made[1].id, 2);
    }

    /// Strokes apart are fills apart; strokes that touch, directly or
    /// through another, are one fill of several strokes.
    #[test]
    fn strokes_apart_are_fills_apart_and_touching_ones_one() {
        let (app, state, _worker) = painting(1);
        app.set_patch_size(0.02);
        paint(&app, &state, &[(0.2, 0.5), (0.3, 0.5)]);
        paint(&app, &state, &[(0.7, 0.5)]);
        paint(&app, &state, &[(0.33, 0.5), (0.33, 0.6)]);
        paint(&app, &state, &[(0.73, 0.5)]);
        paint(&app, &state, &[(0.5, 0.2)]);
        press(&app, Key::Return);
        let made = patches(&state);
        let strokes: Vec<usize> = made.iter().map(|p| 1 + p.more.len()).collect();
        assert_eq!(strokes, [2, 2, 1]);
        assert_eq!(made[0].points, vec![[0.2, 0.5], [0.3, 0.5]]);
        assert_eq!(made[0].more[0].points, vec![[0.33, 0.5], [0.33, 0.6]]);
        assert_eq!(made[1].points, vec![[0.7, 0.5]]);
        assert_eq!(made[2].points, vec![[0.5, 0.2]]);
        let ids: Vec<u64> = made.iter().map(|p| p.id).collect();
        assert_eq!(ids, [1, 2, 3]);
    }

    /// The split itself: through a chain to the target, apart from it,
    /// and in the order painted.
    #[test]
    fn the_split_follows_the_covers() {
        let s = |u: f32| Stroke {
            points: vec![[u, 0.5]],
            radius: 0.05,
        };
        let target = [s(0.1)];
        let strokes = [s(0.9), s(0.18), s(0.5), s(0.26), s(0.58), s(0.95)];
        let (joined, areas) = super::split(Some(&target), &strokes);
        assert_eq!(joined, [1, 3]);
        assert_eq!(areas, [vec![0, 5], vec![2, 4]]);
        let (joined, areas) = super::split(None, &strokes);
        assert!(joined.is_empty());
        assert_eq!(areas, [vec![0, 5], vec![1, 3], vec![2, 4]]);
    }

    /// One Generate is one step of the history, however many fills it
    /// makes, and one undo takes it all back.
    #[test]
    fn one_generate_is_one_step() {
        let (app, state, _worker) = painting(1);
        let before = states(&state);
        paint(&app, &state, &[(0.2, 0.5)]);
        paint(&app, &state, &[(0.5, 0.5)]);
        paint(&app, &state, &[(0.8, 0.5)]);
        press(&app, Key::Return);
        assert_eq!(patches(&state).len(), 3);
        assert_eq!(states(&state), before + 1);
        app.invoke_undo();
        assert!(patches(&state).is_empty(), "one undo, every fill");
    }

    /// The undo key with strokes painted takes the last one back and
    /// leaves the edit and its history alone.
    #[test]
    fn undo_takes_the_last_stroke_painted() {
        let (app, state, _worker) = painting(1);
        state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.3)];
        save(&app, &state);
        let before = states(&state);
        paint(&app, &state, &[(0.6, 0.5)]);
        paint(&app, &state, &[(0.8, 0.5)]);
        app.invoke_undo();
        assert_eq!(painted(&state), 1);
        assert_eq!(patches(&state), [fill(1, 0.3)], "the edit as it was");
        assert_eq!(states(&state), before);
        app.invoke_undo();
        assert_eq!(painted(&state), 0);
        assert!(!app.get_fill_painted());
        app.invoke_undo();
        assert!(patches(&state).is_empty(), "then the edit's own undo");
    }

    /// Painting goes on while a fill is made: the develop that left it
    /// for the worker is not asked again, and the next Generate makes
    /// the next area.
    #[test]
    fn painting_goes_on_while_a_fill_is_made() {
        let (app, state, _worker) = painting(1);
        paint(&app, &state, &[(0.2, 0.5)]);
        press(&app, Key::Return);
        let id = patches(&state)[0].id;
        state.borrow_mut().develop.debounce.stop();
        crate::testing::land_develop(
            &app,
            &state,
            crate::worker::LearnedReport::Off,
            crate::worker::FillReport {
                pending: vec![id],
                ..Default::default()
            },
        );
        assert_eq!(state.borrow().develop.fills_pending, [id]);
        hand_keeping_choice(&app, &state);
        paint(&app, &state, &[(0.7, 0.5)]);
        assert_eq!(painted(&state), 1);
        assert!(!asked(&state), "the fill at work is left to finish");
        assert_eq!(state.borrow().develop.fills_pending, [id]);
        assert_eq!(patches(&state).len(), 1);
        press(&app, Key::Return);
        assert_eq!(patches(&state).len(), 2);
    }

    /// A sidecar version 8 wrote, opened and left with nothing changed,
    /// is not written again; nor with strokes waiting on it, through a
    /// trip to the other frame and back and a quit; a Generate on it
    /// writes it at version 9.
    #[test]
    fn an_old_sidecar_opened_and_left_is_not_written_again() {
        use greycard_library::fixture::{R5, R6, write_frame};
        fn eight(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(o) => {
                    if o.get("version").and_then(|v| v.as_u64())
                        == Some(greycard_edit::VERSION.into())
                    {
                        o.insert("version".into(), 8.into());
                    }
                    o.values_mut().for_each(eight);
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(eight),
                _ => {}
            }
        }
        let dir = crate::testing::scratch_dir("painted-old-sidecar");
        let shots = dir.join("Shots");
        std::fs::create_dir_all(&shots).unwrap();
        write_frame(&shots.join("a.tif"), &R5, 1);
        write_frame(&shots.join("b.tif"), &R6, 2);
        let raw = shots.join("a.tif");
        let mut old = greycard_edit::Sidecar::default();
        {
            let mut e = old.current().clone();
            e.retouch.patches.push(fill(1, 0.5));
            old.record(e);
        }
        let mut value: serde_json::Value = serde_json::from_str(&old.to_json()).unwrap();
        eight(&mut value);
        let text = value.to_string();
        assert!(text.contains("\"version\":8") && !text.contains("\"version\":9"));
        let path = greycard_edit::Sidecar::path_for(&raw);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();

        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        crate::panel::browser::open_folder(&state, &app, &worker, &shots);
        crate::roots::land_sent(&state, &app, &worker);
        app.invoke_select(0);
        app.invoke_select(1);
        app.invoke_select(0);
        quit(&app, &state);
        let unchanged = || std::fs::read_to_string(&path).unwrap() == text;
        assert!(unchanged(), "not written again");

        // Strokes waiting on it, the frame left and come back to, and a
        // quit: nothing written for them.
        hand(&app, &state);
        app.invoke_patch_changed(0);
        hand_keeping_choice(&app, &state);
        paint(&app, &state, &[(0.55, 0.5)]);
        app.invoke_retouch_toggled("Fill".into());
        app.invoke_select(1);
        app.invoke_select(0);
        quit(&app, &state);
        assert!(unchanged(), "nothing written for strokes waiting");

        hand_keeping_choice(&app, &state);
        app.invoke_patch_changed(0);
        assert_eq!(painted(&state), 1, "still waiting");
        press(&app, Key::Return);
        let written = greycard_edit::Sidecar::read_as_file(&path).unwrap();
        assert_eq!(
            written.current().retouch.patches[0].more.len(),
            1,
            "grown, and written"
        );
        let on_disk: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk["current"]["version"], greycard_edit::VERSION);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A view, by its name and how it is opened.
    type View = (&'static str, fn(&App));

    /// The views away from the Develop picture, where the strokes are not
    /// drawn and the keys are the view's own.
    fn views_away() -> [View; 5] {
        [
            ("the grid", |app| app.set_grid_open(true)),
            ("the lightbox", |app| app.set_lightbox(true)),
            ("the lightbox's loupe", |app| {
                app.set_lightbox(true);
                app.set_lightbox_loupe(true);
            }),
            ("the crop", |app| app.set_crop_mode(true)),
            ("culling", |app| app.set_culling(true)),
        ]
    }

    /// In the grid, the lightbox, the crop and culling, Enter and Esc are
    /// the view's own: nothing is made or let go there, the Fill still in
    /// hand and its strokes waiting.
    #[test]
    fn enter_and_esc_are_the_view_s_own_away_from_the_picture() {
        for (how, view) in views_away() {
            let (app, state, _worker) = painting(1);
            paint(&app, &state, &[(0.2, 0.5)]);
            paint(&app, &state, &[(0.7, 0.5)]);
            for key in [Key::Return, Key::Escape] {
                // The Fill in hand whatever the view did, as the render
                // loop tells the window.
                hand(&app, &state);
                view(&app);
                super::show(&state.borrow(), &app);
                press(&app, key);
                assert!(patches(&state).is_empty(), "{how}: nothing made");
                assert_eq!(waiting_on(&state, 0), 2, "{how}: nothing let go");
                assert!(!asked(&state), "{how}: nothing developed");
            }
        }
    }

    /// In the grid, the lightbox, the crop and culling, the undo is the
    /// view's own: a mark made there is taken back, and no stroke waiting
    /// goes, nor do strokes an Esc let go come back.
    #[test]
    fn the_undo_is_the_view_s_own_away_from_the_picture() {
        use greycard_edit::meta::Flag;
        let flag = |state: &Rc<Shared>| state.borrow().sidecars[0].meta.flag;
        for (how, view) in views_away() {
            let (app, state, _worker) = painting(1);
            paint(&app, &state, &[(0.2, 0.5)]);
            paint(&app, &state, &[(0.7, 0.5)]);
            view(&app);
            app.invoke_meta_key("x".into());
            assert_eq!(flag(&state), Flag::Reject, "{how}: marked");
            app.invoke_undo();
            assert_eq!(flag(&state), Flag::None, "{how}: the mark undone");
            assert_eq!(waiting_on(&state, 0), 2, "{how}: no stroke taken");

            // Let go on the picture, then the view's undo: not back.
            let (app, state, _worker) = painting(1);
            paint(&app, &state, &[(0.2, 0.5)]);
            press(&app, Key::Escape);
            view(&app);
            app.invoke_meta_key("x".into());
            app.invoke_undo();
            assert_eq!(flag(&state), Flag::None, "{how}: the mark undone");
            assert_eq!(waiting_on(&state, 0), 0, "{how}: not brought back");
        }
    }

    /// A held Esc is one press: it lets the strokes go and leaves the
    /// Fill in hand and the chosen Fill chosen, so the undo after it
    /// brings them back.
    #[test]
    fn a_held_esc_lets_the_strokes_go_and_no_more() {
        use slint::platform::WindowEvent;
        let held = |app: &App| {
            let text: slint::SharedString = Key::Escape.into();
            app.window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            for _ in 0..3 {
                app.window()
                    .dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
            }
            app.window()
                .dispatch_event(WindowEvent::KeyReleased { text });
        };
        let (app, state, _worker) = painting(1);
        paint(&app, &state, &[(0.2, 0.5)]);
        held(&app);
        assert_eq!(painted(&state), 0);
        assert_eq!(app.get_retouch_mode(), "Fill", "still in hand");
        app.invoke_undo();
        assert_eq!(painted(&state), 1, "back");

        state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.8)];
        save(&app, &state);
        app.invoke_patch_changed(0);
        hand_keeping_choice(&app, &state);
        held(&app);
        assert_eq!(painted(&state), 0);
        assert_eq!(app.get_patch(), 0, "still chosen");
        assert_eq!(app.get_retouch_mode(), "Fill");

        // Only the Esc that lets strokes go eats its repeats: held with
        // Heal in hand and a repair chosen, it goes on as it always
        // did, the repair let go and the tool put down.
        let (app, state, _worker) = painting(1);
        state.borrow_mut().develop.edit.retouch.patches = vec![Patch {
            method: RetouchMethod::Heal,
            ..fill(1, 0.5)
        }];
        save(&app, &state);
        app.invoke_retouch_toggled("Heal".into());
        app.invoke_patch_changed(0);
        assert_eq!(
            (app.get_retouch_mode().as_str(), app.get_patch()),
            ("Heal", 0)
        );
        held(&app);
        assert_eq!((app.get_retouch_mode().as_str(), app.get_patch()), ("", -1));
    }

    /// The undo after an Esc brings the strokes back only with nothing
    /// done since: an edit changed and undone back to the same value has
    /// moved the history, and the next undo is the edit's.
    #[test]
    fn an_edit_changed_and_undone_ends_the_restore() {
        let (app, state, _worker) = painting(1);
        state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.8)];
        save(&app, &state);
        paint(&app, &state, &[(0.2, 0.5)]);
        press(&app, Key::Escape);
        state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.8), fill(2, 0.5)];
        save(&app, &state);
        app.invoke_undo();
        assert_eq!(patches(&state), [fill(1, 0.8)], "the change undone");
        assert_eq!(painted(&state), 0);
        app.invoke_undo();
        assert_eq!(painted(&state), 0, "not back");
        assert!(patches(&state).is_empty(), "the edit's next undo");
    }

    /// The tool put down by a tab or a mode with a Fill chosen: the
    /// sliders show the Fill they now edit, not the brush.
    #[test]
    fn the_tool_put_down_shows_the_chosen_fill_s_sliders() {
        type Down = fn(&App, &Rc<Shared>);
        let downs: [(&str, Down); 2] = [
            ("another tab", |app, _| {
                app.invoke_tab_changed("Develop".into())
            }),
            ("culling", |app, state| {
                crate::panel::cull::enter_cull(&mut state.borrow_mut(), app, 1)
            }),
        ];
        for (how, down) in downs {
            let (app, state, _worker) = painting(2);
            state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.3)];
            save(&app, &state);
            app.invoke_patch_changed(0);
            hand_keeping_choice(&app, &state);
            app.set_patch_size(0.12);
            app.invoke_patch_edited();
            assert_eq!(patches(&state)[0].radius, 0.05, "{how}: the brush");
            down(&app, &state);
            assert_eq!(app.get_retouch_mode(), "", "{how}");
            assert_eq!(app.get_patch_size(), 0.05, "{how}: the Fill's own size");
        }
    }

    /// A frame whose file is followed to a new path keeps its strokes,
    /// moved once or twice; a file that comes to the old path has none.
    #[test]
    fn strokes_follow_their_file_when_it_moves() {
        let (app, state, _worker) = painting(3);
        paint(&app, &state, &[(0.2, 0.5)]);
        let was = state.borrow().files[0].clone();
        let once = was.with_file_name("moved.CR3");
        let twice = was.with_file_name("moved again.CR3");
        state.borrow_mut().move_file(0, once.clone());
        assert_eq!(waiting_on(&state, 0), 1, "moved once");
        state.borrow_mut().move_file(0, twice);
        assert_eq!(waiting_on(&state, 0), 1, "moved twice");
        hand(&app, &state);
        app.invoke_patch_changed(-1);
        assert!(app.get_fill_painted());
        // Another file at either path it left: nothing there.
        state.borrow_mut().move_file(1, was);
        assert_eq!(waiting_on(&state, 1), 0);
        state.borrow_mut().move_file(2, once);
        assert_eq!(waiting_on(&state, 2), 0);
        assert_eq!(waiting_on(&state, 0), 1);
    }

    /// A version swapped with its original: the strokes stay with the
    /// line they were painted over, now the original's.
    #[test]
    fn strokes_go_with_their_line_when_a_version_is_swapped() {
        let (app, state, worker) = painting(1);
        let id = {
            let mut st = state.borrow_mut();
            let id = st.sidecars[0].new_version(None, Some("Warm")).unwrap();
            crate::versions::list(&mut st);
            crate::panel::browser::rebuild_browser(&mut st, &app);
            id
        };
        app.invoke_select(1);
        hand(&app, &state);
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(state.borrow().version_of(1), Some(id.as_str()));
        paint(&app, &state, &[(0.2, 0.5)]);
        assert_eq!((waiting_on(&state, 0), waiting_on(&state, 1)), (0, 1));
        crate::panel::versions::swap(&mut state.borrow_mut(), &app, &worker, false);
        assert_eq!(
            (waiting_on(&state, 0), waiting_on(&state, 1)),
            (1, 0),
            "{}",
            app.get_status()
        );
    }

    /// A drag begun on a slider's track, away from its handle, makes a
    /// chosen Fill again once, when it is let go.
    #[test]
    fn a_drag_from_the_track_makes_the_fill_once() {
        let (app, state, _worker) = painting(1);
        state.borrow_mut().develop.edit.retouch.patches = vec![fill(1, 0.3)];
        save(&app, &state);
        app.invoke_retouch_toggled("Fill".into());
        app.invoke_patch_changed(0);
        assert_eq!(app.get_retouch_mode(), "");
        let asks = Rc::new(std::cell::Cell::new(0));
        let (mid, y) = crate::testing::track_middle(&app, "RetouchSection::size-slider");
        let window = app.window();
        let at = |x: f32| slint::LogicalPosition::new(x, y);
        use slint::platform::{PointerEventButton, WindowEvent};
        let count = |asks: &Rc<std::cell::Cell<i32>>| {
            if state.borrow().develop.debounce.running() {
                asks.set(asks.get() + 1);
                state.borrow_mut().develop.debounce.stop();
            }
        };
        window.dispatch_event(WindowEvent::PointerPressed {
            position: at(mid + 30.0),
            button: PointerEventButton::Left,
        });
        count(&asks);
        for x in [mid + 40.0, mid + 50.0] {
            window.dispatch_event(WindowEvent::PointerMoved { position: at(x) });
            count(&asks);
        }
        assert_eq!(asks.get(), 0, "nothing while the slider is held");
        window.dispatch_event(WindowEvent::PointerReleased {
            position: at(mid + 50.0),
            button: PointerEventButton::Left,
        });
        count(&asks);
        assert_ne!(patches(&state)[0].radius, 0.05, "resized");
        assert_eq!(asks.get(), 1, "made once, at the release");
    }
}
