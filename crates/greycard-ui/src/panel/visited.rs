//! The mouse's back and forward buttons over the grid, and Alt+← and
//! Alt+→ there: the places the browser has listed, kept as a web
//! browser keeps the pages visited.
//!
//! A place is a folder, all the roots, or a collection. Every list the
//! browser takes lands through `recent::opened`, which hands the place
//! here ([`Visited::landed`]): a new one puts the place before it on
//! the back stack and clears the forward one, however it was opened (a
//! tile, the tree, Recently opened, Open folder). The stacks move only
//! when a place lands: a press asks for a place, and should something
//! else land first (a tile clicked meanwhile), or nothing land at all
//! (a folder emptied meanwhile), the history holds what was shown. A
//! place no longer there is dropped, and one not answering passed over
//! and kept, each with a line in the log. Each place on a stack keeps
//! the frame current when it was left, and back or forward opens it on
//! that frame when its list still has it. The history is the window's,
//! for the session.

use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::roots::View;
use crate::worker::Worker;
use crate::{App, Shared, State};

/// The most places either stack holds; the oldest go first.
const KEPT: usize = 100;

/// A list the browser can be asked for again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// One folder: from the disk, of a root's tree, or a root itself.
    /// How it is listed (from the disk or the index, with the folders
    /// under it or not) is the switch's and the roots', not the
    /// place's, so a folder is one place however it was opened.
    Folder(PathBuf),
    /// Every root.
    AllRoots,
    /// A collection, by its id.
    Collection(String),
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Place::Folder(dir) => write!(f, "{}", dir.display()),
            Place::AllRoots => write!(f, "all roots"),
            Place::Collection(id) => write!(f, "the collection {id}"),
        }
    }
}

/// A place on a stack, with the frame current when it was left: its
/// file, as the selection keeps files (§156), not a row the filter and
/// the sort move. None when no frame was current.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stop {
    place: Place,
    frame: Option<PathBuf>,
}

/// A press's place asked for and not landed yet: where it is on the
/// stack it was taken from, counted from the nearest, and the places
/// nearer it found gone.
#[derive(Clone, Debug)]
struct Asked {
    back: bool,
    at: usize,
    gone: Vec<usize>,
    to: Place,
}

/// The places behind and ahead, and the one the browser lists.
#[derive(Default)]
pub(crate) struct Visited {
    /// Nearest last.
    back: Vec<Stop>,
    /// Nearest last.
    forward: Vec<Stop>,
    /// The last place that landed. A list that is no place (the export
    /// queue, files from several folders) leaves it be.
    here: Option<Place>,
    /// The list shown is the one `here` names: false over a list that
    /// is no place, whose frame is not the place's to keep.
    shown: bool,
    asked: Option<Asked>,
    /// A press's look at the disk is out; a press meanwhile is dropped.
    looking: bool,
}

/// What the window knows of a candidate before any look at the disk.
enum Check {
    There,
    Gone(&'static str),
    /// A folder on the disk, looked at off the window's thread.
    Look(PathBuf),
}

/// A candidate's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    There,
    /// Gone for good: dropped.
    Gone(&'static str),
    /// Not answering: passed over, kept.
    Silent,
}

impl Visited {
    /// The browser lists `place` now, `left` the frame current on the
    /// list before it. The place here (a list read again) and no place
    /// (the export queue) move nothing; a place on the way a press
    /// asked for moves the stacks that far; any other is a new open.
    /// The place left goes on a stack with `left`.
    pub(crate) fn landed(&mut self, place: Option<Place>, left: Option<PathBuf>) {
        let Some(place) = place else {
            self.shown = false;
            return;
        };
        if self.here.as_ref() == Some(&place) {
            self.shown = true;
            return;
        }
        let frame = left.filter(|_| self.shown);
        self.shown = true;
        let here = self.here.take().map(|place| Stop { place, frame });
        if let Some(asked) = self.asked.take()
            && let Some(at) = self.on_the_way(&asked, &place)
        {
            self.way_taken(asked.back, at, &asked.gone, here);
            self.here = Some(place);
            // A press past this one is still out: what it asked for
            // is nearer now by the places taken.
            if at < asked.at {
                let taken = at + 1;
                self.asked = Some(Asked {
                    at: asked.at - taken,
                    gone: asked
                        .gone
                        .iter()
                        .filter(|g| **g > at)
                        .map(|g| g - taken)
                        .collect(),
                    ..asked
                });
            }
            return;
        }
        if let Some(here) = here {
            push(&mut self.back, here);
        }
        self.forward.clear();
        self.here = Some(place);
    }

    /// An open by hand: no press's place is waited for any more, so
    /// only a landing of what a press asked for moves the stacks its
    /// way.
    pub(crate) fn opened_by_hand(&mut self) {
        self.asked = None;
    }

    /// How far down the asked stack `place` is, when a press asked for
    /// it: the place `asked` names, or one nearer that a press before
    /// it asked for and is landing late. None for any other.
    fn on_the_way(&mut self, asked: &Asked, place: &Place) -> Option<usize> {
        let (from, _) = self.stacks(asked.back);
        let n = from.len();
        if asked.at >= n || from[n - 1 - asked.at].place != asked.to {
            return None;
        }
        (0..=asked.at).find(|d| !asked.gone.contains(d) && from[n - 1 - d].place == *place)
    }

    fn stacks(&mut self, back: bool) -> (&mut Vec<Stop>, &mut Vec<Stop>) {
        if back {
            (&mut self.back, &mut self.forward)
        } else {
            (&mut self.forward, &mut self.back)
        }
    }

    /// The place `at` down the stack landed: it and every place nearer
    /// it taken from the stack, and the place left given to the other
    /// with those passed over that are not gone, the nearest to the
    /// place landed on top, as a browser's back of several steps. Each
    /// keeps its frame.
    fn way_taken(&mut self, back: bool, at: usize, gone: &[usize], here: Option<Stop>) {
        let (from, onto) = self.stacks(back);
        let n = from.len();
        let passed: Vec<Stop> = from.drain(n - 1 - at..).rev().collect();
        if let Some(here) = here {
            push(onto, here);
        }
        for (d, place) in passed.into_iter().enumerate().take(at) {
            if !gone.contains(&d) {
                push(onto, place);
            }
        }
    }

    /// The places at `gone` (from the nearest) taken off a stack.
    fn drop_gone(&mut self, back: bool, gone: &[usize]) {
        let (from, _) = self.stacks(back);
        let mut gone = gone.to_vec();
        // The farthest first: taking a place off shifts those beyond
        // it one nearer, and leaves those nearer where they are.
        gone.sort_unstable_by(|a, b| b.cmp(a));
        gone.dedup();
        for at in gone {
            let n = from.len();
            if at < n {
                from.remove(n - 1 - at);
            }
        }
    }
}

fn push(stack: &mut Vec<Stop>, stop: Stop) {
    stack.push(stop);
    if stack.len() > KEPT {
        stack.remove(0);
    }
}

/// The frame current now, as a stack keeps it: one frame, a selection
/// of several kept as the frame current.
pub(crate) fn frame_of(st: &State) -> Option<PathBuf> {
    st.current.and_then(|c| st.path_at(c)).cloned()
}

/// The place the browser lists now, after `recent::opened` has named
/// the folder.
pub(crate) fn place_of(st: &State) -> Option<Place> {
    match &st.views.view {
        View::Folder => st.views.recent.open.clone().map(Place::Folder),
        View::Branch { folder, .. } => Some(Place::Folder(folder.clone())),
        View::Roots(Some(root)) => Some(Place::Folder(root.clone())),
        View::Roots(None) => Some(Place::AllRoots),
        View::Collection { id } => Some(Place::Collection(id.clone())),
        View::Queue => None,
    }
}

/// `place` opened as its own way of opening it would: a folder under
/// a root as its row in the tree opens it, the switch as it stands; a
/// folder under none as Open folder does. On `frame` when its list has
/// it, else on the frame that way would start on.
fn open(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, place: &Place, frame: Option<PathBuf>) {
    let view = {
        let st = state.borrow();
        match place {
            Place::Folder(dir) => st
                .views
                .library
                .roots
                .root_of(dir)
                .map(|root| crate::tree::view_for(root, dir, st.views.tree.subfolders)),
            Place::AllRoots => Some(View::Roots(None)),
            Place::Collection(id) => Some(View::Collection { id: id.clone() }),
        }
    };
    match (view, place) {
        (Some(view), _) => {
            let on = frame.map_or(crate::roots::On::Screen, crate::roots::On::Left);
            crate::roots::open_view_on(state, app, worker, view, on);
        }
        (None, Place::Folder(dir)) => {
            crate::panel::browser::open_folder_on(state, app, worker, dir, frame);
        }
        (None, _) => {}
    }
}

/// Whether `place` can be gone to, so far as the window knows without
/// the disk.
fn check(st: &State, place: &Place) -> Check {
    match place {
        _ if st.views.visited.here.as_ref() == Some(place) => Check::Gone("the place open"),
        // An offline root's folders are listed from the index.
        Place::Folder(dir)
            if st
                .views
                .library
                .roots
                .root_of(dir)
                .is_some_and(|r| st.views.library.offline.contains(r)) =>
        {
            Check::There
        }
        Place::Folder(dir) => Check::Look(dir.clone()),
        Place::AllRoots => Check::There,
        Place::Collection(id) if st.views.collections.collection(id).is_none() => {
            Check::Gone("deleted")
        }
        Place::Collection(_) => Check::There,
    }
}

/// The most folders one press looks at; the walk stops at the last
/// of them rather than passing over any it has not looked at.
const LOOKS: usize = 16;

/// What a look at the disk says of a folder.
fn said(there: Option<bool>) -> Answer {
    match there {
        Some(true) => Answer::There,
        Some(false) => Answer::Gone("not there"),
        None => Answer::Silent,
    }
}

/// The candidates up to the walk's reach, and how each is known
/// without the disk.
fn known(checks: &[Check]) -> Vec<Option<Answer>> {
    let mut looks = 0;
    let mut out = Vec::new();
    for c in checks {
        if looks == LOOKS {
            break;
        }
        out.push(match c {
            Check::There => Some(Answer::There),
            Check::Gone(why) => Some(Answer::Gone(why)),
            Check::Look(_) => {
                looks += 1;
                None
            }
        });
    }
    out
}

/// The candidates answered nearest first, up to the first that is
/// there. Every folder in reach is looked at at once, each look its own
/// wait (`answer_of`), so a press waits one look at most, and no longer
/// than the nearest folder there takes to answer: the answers are
/// settled in order as they come.
#[cfg(not(test))]
fn answer(checks: &[Check]) -> Vec<Answer> {
    let mut got = known(checks);
    let (tx, rx) = std::sync::mpsc::channel();
    for (i, c) in checks.iter().enumerate().take(got.len()) {
        let Check::Look(dir) = c else {
            continue;
        };
        let (tx, dir) = (tx.clone(), dir.clone());
        let spawned = std::thread::Builder::new()
            .name("greycard visited look".into())
            .spawn(move || {
                let there = crate::roots::answer_of(&dir, crate::roots::ROOT_WAIT);
                let _ = tx.send((i, there));
            });
        if spawned.is_err() {
            got[i] = Some(Answer::Silent);
        }
    }
    drop(tx);
    let mut out = Vec::new();
    loop {
        while let Some(Some(a)) = got.get(out.len()) {
            out.push(*a);
            if *a == Answer::There {
                return out;
            }
        }
        if out.len() == got.len() {
            return out;
        }
        match rx.recv() {
            Ok((i, there)) => got[i] = Some(said(there)),
            Err(_) => {
                for a in got.iter_mut().filter(|a| a.is_none()) {
                    *a = Some(Answer::Silent);
                }
            }
        }
    }
}

/// In a test, in place and in order, up to the first there.
#[cfg(test)]
fn answer(checks: &[Check]) -> Vec<Answer> {
    let got = known(checks);
    let mut out = Vec::new();
    for (c, a) in checks.iter().zip(got) {
        let a = a.unwrap_or_else(|| match c {
            Check::Look(dir) => said(crate::roots::answer_of(dir, crate::roots::ROOT_WAIT)),
            _ => Answer::Silent,
        });
        out.push(a);
        if a == Answer::There {
            break;
        }
    }
    out
}

/// What a press saw when it was made, for its look to land against.
struct Press {
    back: bool,
    generation: u64,
    /// The stack as it was, nearest last.
    stack: Vec<Stop>,
    /// The first candidate, from the nearest: past a place the last
    /// press this way asked for and has not landed.
    start: usize,
    /// The places nearer `start` found gone by that press.
    gone: Vec<usize>,
}

/// A mouse's back (or forward) button over the grid, or Alt+← (Alt+→)
/// there (`keys.rs`): the nearest place
/// that way that is still there, asked for. A press again before it
/// lands goes on past it, as a browser's would. Nothing while a menu is
/// up (the grid asks first), and a press while a look from the last is
/// out is dropped.
pub(crate) fn go(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, back: bool) {
    let way = if back { "back" } else { "forward" };
    let (press, checks) = {
        let mut st = state.borrow_mut();
        let visited = &st.views.visited;
        if visited.looking {
            tracing::info!("visited: {way} pressed while a look is out; dropped");
            return;
        }
        let stack = if back {
            visited.back.clone()
        } else {
            visited.forward.clone()
        };
        let (start, gone) = match &visited.asked {
            Some(a) if a.back == back => (a.at + 1, a.gone.clone()),
            _ => (0, Vec::new()),
        };
        if start >= stack.len() {
            tracing::info!("visited: nothing to go {way} to");
            return;
        }
        let checks: Vec<Check> = stack
            .iter()
            .rev()
            .skip(start)
            .map(|s| check(&st, &s.place))
            .collect();
        st.views.visited.looking = true;
        let press = Press {
            back,
            generation: st.views.view_generation,
            stack,
            start,
            gone,
        };
        (press, checks)
    };
    look(state, app, worker, press, checks);
}

/// The candidates looked at off the window's thread (a share that has
/// stopped answering holds a look for as long as it likes), and the
/// way taken back on it.
#[cfg(not(test))]
fn look(state: &Rc<Shared>, app: &App, _worker: &Rc<Worker>, press: Press, checks: Vec<Check>) {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard visited check".into())
        .spawn(move || {
            let answers = answer(&checks);
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                arrive(&state, &app, &worker, press, &answers);
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("visited: {e}");
        state.borrow_mut().views.visited.looking = false;
    }
}

/// In a test the places are looked at in place: there is no event
/// loop to land the look on.
#[cfg(test)]
fn look(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, press: Press, checks: Vec<Check>) {
    let answers = answer(&checks);
    arrive(state, app, worker, press, &answers);
}

/// The look back: dropped when another list opened meanwhile or the
/// stack changed; else the first place there asked for, those gone
/// before it noted to drop when it lands. With none there, the gone
/// are dropped now.
fn arrive(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, press: Press, answers: &[Answer]) {
    let way = if press.back { "back" } else { "forward" };
    let to = {
        let mut st = state.borrow_mut();
        let generation = st.views.view_generation;
        let visited = &mut st.views.visited;
        visited.looking = false;
        let now = if press.back {
            &visited.back
        } else {
            &visited.forward
        };
        if generation != press.generation {
            tracing::info!("visited: {way} came back after another list opened; dropped");
            return;
        }
        if *now != press.stack {
            tracing::info!(
                "visited: {way} came back after a place landed and moved the history; dropped"
            );
            return;
        }
        let stops: Vec<&Stop> = press.stack.iter().rev().skip(press.start).collect();
        let mut gone = press.gone.clone();
        let mut found = None;
        for (k, (stop, answer)) in stops.iter().zip(answers).enumerate() {
            let at = press.start + k;
            let place = &stop.place;
            match answer {
                Answer::There => {
                    found = Some((at, (*stop).clone()));
                    break;
                }
                Answer::Gone(why) => {
                    tracing::info!("visited: {way} drops {place}: {why}");
                    gone.push(at);
                }
                Answer::Silent => {
                    tracing::info!("visited: {way} passes over {place}: not answering");
                }
            }
        }
        let Some((at, Stop { place: to, frame })) = found else {
            // A press past one still out leaves that one be: only the
            // places this press found gone, all beyond it, are dropped,
            // so its place stays where it was asked for.
            let new: Vec<usize> = gone
                .iter()
                .copied()
                .filter(|g| !press.gone.contains(g))
                .collect();
            if press.start == 0 {
                visited.asked = None;
            }
            visited.drop_gone(press.back, &new);
            tracing::info!("visited: nothing further to go {way} to");
            return;
        };
        tracing::info!("visited: {way} to {to}");
        (
            Asked {
                back: press.back,
                at,
                gone,
                to: to.clone(),
            },
            to,
            frame,
        )
    };
    // Opened as by hand, which lets go of any ask, and then asked.
    let (asked, to, frame) = to;
    open(state, app, worker, &to, frame);
    state.borrow_mut().views.visited.asked = Some(asked);
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
    app.on_visit_folder(move |back| {
        if let Some(app) = app_weak.upgrade() {
            go(&state, &app, &worker, back);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roots::land_sent;
    use crate::testing::{scratch_dir, state_for, window};
    use slint::Model;
    use std::path::Path;

    /// A folder of one frame.
    fn shoot(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("x.tif"), b"").unwrap();
        dir.to_path_buf()
    }

    fn open_dir(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, dir: &Path) {
        crate::panel::browser::open_folder(state, app, worker, dir);
        land_sent(state, app, worker);
    }

    fn press(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, back: bool) {
        app.invoke_visit_folder(back);
        land_sent(state, app, worker);
    }

    fn open_now(state: &Rc<Shared>) -> Option<PathBuf> {
        state.borrow().views.recent.open.clone()
    }

    fn folder(name: &str) -> Place {
        Place::Folder(PathBuf::from(name))
    }

    /// A folder on a stack, left with no frame current.
    fn stop(name: &str) -> Stop {
        Stop {
            place: folder(name),
            frame: None,
        }
    }

    fn places(stack: &[Stop]) -> Vec<Place> {
        stack.iter().map(|s| s.place.clone()).collect()
    }

    /// A mouse button pressed and let go at `at`, as the window has it.
    fn side_button(
        app: &App,
        at: slint::LogicalPosition,
        button: slint::platform::PointerEventButton,
    ) {
        use slint::platform::WindowEvent;
        for event in [
            WindowEvent::PointerMoved { position: at },
            WindowEvent::PointerPressed {
                position: at,
                button,
            },
            WindowEvent::PointerReleased {
                position: at,
                button,
            },
        ] {
            app.window().dispatch_event(event);
        }
    }

    /// A subfolder opened from its tile in the grid: back (the mouse's
    /// own button, pressed over the other tile) lands on the folder it
    /// was opened from, forward on the subfolder again, and a new open
    /// from the folder clears the way forward.
    #[test]
    fn back_returns_to_the_folder_a_tile_was_opened_from() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;
        let dir = scratch_dir("visited-tiles");
        let a = shoot(&dir.join("a"));
        let b = shoot(&a.join("b"));
        let c = shoot(&a.join("c"));
        let app = window(0);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = state_for(&app, Vec::new());
        open_dir(&state, &app, &worker, &a);
        app.set_grid_open(true);
        land_sent(&state, &app, &worker);
        assert_eq!(app.get_grid_tiles().row_count(), 2, "b and c as tiles");

        app.invoke_grid_tile_picked(0);
        land_sent(&state, &app, &worker);
        assert_eq!(open_now(&state), Some(b.clone()));
        assert_eq!(app.get_grid_tiles().row_count(), 0, "b has no folders");

        // The back button over a cell of b's.
        slint::platform::update_timers_and_animations();
        let cell = ElementHandle::find_by_element_id(&app, "GridSheet::ct")
            .next()
            .expect("a cell on the sheet");
        let (at, size) = (cell.absolute_position(), cell.size());
        let middle = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
        side_button(&app, middle, PointerEventButton::Back);
        land_sent(&state, &app, &worker);
        assert_eq!(open_now(&state), Some(a.clone()), "back to the folder");
        land_sent(&state, &app, &worker);

        // The forward button over c's tile in a: b again, not c.
        slint::platform::update_timers_and_animations();
        let tile = crate::testing::keyed(&app, "GridSheet::folder-tile", "c").expect("c's tile");
        let (at, size) = (tile.absolute_position(), tile.size());
        let middle = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
        side_button(&app, middle, PointerEventButton::Forward);
        land_sent(&state, &app, &worker);
        assert_eq!(open_now(&state), Some(b.clone()), "forward again");
        press(&state, &app, &worker, false);
        assert_eq!(open_now(&state), Some(b.clone()), "nothing further on");

        press(&state, &app, &worker, true);
        land_sent(&state, &app, &worker);
        app.invoke_grid_tile_picked(1);
        land_sent(&state, &app, &worker);
        assert_eq!(open_now(&state), Some(c.clone()));
        press(&state, &app, &worker, false);
        assert_eq!(open_now(&state), Some(c.clone()), "forward cleared");
        press(&state, &app, &worker, true);
        assert_eq!(open_now(&state), Some(a.clone()));
        press(&state, &app, &worker, true);
        assert_eq!(open_now(&state), Some(a.clone()), "nothing before");
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Alt+← and Alt+→ in the grid go back and forward as the side
    /// buttons do.
    #[test]
    fn alt_and_the_arrows_go_back_and_forward() {
        use slint::platform::{Key, WindowEvent};
        let alt = |app: &App, key: Key| {
            app.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Alt.into(),
            });
            crate::testing::press(app, key);
            app.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Alt.into(),
            });
        };
        let dir = scratch_dir("visited-alt");
        let a = shoot(&dir.join("a"));
        let b = shoot(&dir.join("b"));
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        open_dir(&state, &app, &worker, &a);
        open_dir(&state, &app, &worker, &b);
        app.set_grid_open(true);
        land_sent(&state, &app, &worker);
        alt(&app, Key::LeftArrow);
        land_sent(&state, &app, &worker);
        assert_eq!(open_now(&state), Some(a.clone()), "back");
        alt(&app, Key::RightArrow);
        land_sent(&state, &app, &worker);
        assert_eq!(open_now(&state), Some(b.clone()), "forward");
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// In the lightbox the side buttons ask for the folders too, and
    /// step no frame.
    #[test]
    fn the_lightbox_goes_back_and_forward_too() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = state_for(&app, crate::testing::folder(3));
        let visits = Rc::new(std::cell::RefCell::new(Vec::new()));
        let asked = visits.clone();
        app.on_visit_folder(move |back| asked.borrow_mut().push(back));
        let steps = Rc::new(std::cell::Cell::new(0));
        let stepped = steps.clone();
        app.on_grid_step(move |_, _, _, _| stepped.set(stepped.get() + 1));
        app.set_grid_open(true);
        app.set_lightbox(true);
        app.invoke_grid_range(0.0, 773.0, 8, 0.0);
        slint::platform::update_timers_and_animations();
        let cell = ElementHandle::find_by_element_id(&app, "GridSheet::ct")
            .next()
            .expect("a cell on the sheet");
        let (at, size) = (cell.absolute_position(), cell.size());
        let middle = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
        side_button(&app, middle, PointerEventButton::Back);
        side_button(&app, middle, PointerEventButton::Forward);
        assert_eq!(*visits.borrow(), [true, false]);
        assert_eq!(steps.get(), 0);
    }

    /// A folder gone since it was visited is dropped; one not answering
    /// is passed over and kept, there to go to once it answers.
    #[test]
    fn a_folder_gone_is_dropped_and_one_not_answering_kept() {
        let dir = scratch_dir("visited-gone");
        let (a, b, c, d) = (
            shoot(&dir.join("a")),
            shoot(&dir.join("b")),
            shoot(&dir.join("c")),
            shoot(&dir.join("d")),
        );
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        for f in [&a, &b, &c, &d] {
            open_dir(&state, &app, &worker, f);
        }
        crate::testing::remove_dir_retry(&c);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(b.clone()));
        press(&state, &app, &worker, true);
        assert_eq!(open_now(&state), Some(a.clone()), "c gone, b silent");
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        press(&state, &app, &worker, false);
        assert_eq!(open_now(&state), Some(b.clone()), "b kept");
        press(&state, &app, &worker, false);
        assert_eq!(open_now(&state), Some(d.clone()), "c dropped");
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A tree's folder, the root's own view, and the folder opened
    /// from the disk are one place: the switch makes no history.
    #[test]
    fn a_folder_is_one_place_however_it_is_listed() {
        let app = window(0);
        let (state, _worker) = state_for(&app, Vec::new());
        let r = PathBuf::from("/r");
        let mut st = state.borrow_mut();
        let mut seen = Vec::new();
        for view in [
            View::Branch {
                root: r.clone(),
                folder: r.clone(),
                deep: false,
            },
            View::Roots(Some(r.clone())),
            View::Folder,
        ] {
            st.views.view = view;
            st.views.recent.open = Some(r.clone());
            seen.push(place_of(&st));
        }
        assert_eq!(seen, vec![Some(Place::Folder(r)); 3]);
        st.views.view = View::Queue;
        assert_eq!(place_of(&st), None);
    }

    /// The stacks as a browser has them: a place asked for moves them
    /// only when it lands, several steps at once moving every place
    /// passed over.
    #[test]
    fn the_stacks_move_when_the_place_asked_for_lands() {
        let mut v = Visited::default();
        for p in ["a", "b", "c", "d"] {
            v.landed(Some(folder(p)), None);
        }
        v.landed(Some(folder("d")), None);
        assert_eq!(places(&v.back), [folder("a"), folder("b"), folder("c")]);
        // Back three, b found gone on the way.
        v.asked = Some(Asked {
            back: true,
            at: 2,
            gone: vec![1],
            to: folder("a"),
        });
        v.landed(Some(folder("a")), None);
        assert!(v.back.is_empty());
        assert_eq!(
            places(&v.forward),
            [folder("d"), folder("c")],
            "c next forward"
        );
        assert_eq!(v.here, Some(folder("a")));
    }

    /// A press superseded by another open before its place lands: the
    /// place asked for never shown is not history, and the one shown is.
    #[test]
    fn a_superseded_ask_leaves_what_was_shown() {
        let mut v = Visited::default();
        v.landed(Some(folder("a")), None);
        v.landed(Some(folder("b")), None);
        v.asked = Some(Asked {
            back: true,
            at: 0,
            gone: Vec::new(),
            to: folder("a"),
        });
        v.landed(Some(folder("c")), None);
        assert_eq!(places(&v.back), [folder("a"), folder("b")]);
        assert!(v.forward.is_empty());
        assert!(v.asked.is_none());
    }

    /// A press whose place opened to nothing (a folder emptied): the
    /// history still has what is on screen, and the next press goes
    /// past the place asked for, not to it again.
    #[test]
    fn a_failed_ask_leaves_the_history_and_the_next_press_goes_past_it() {
        let dir = scratch_dir("visited-empty");
        let (a, b, c) = (
            shoot(&dir.join("a")),
            shoot(&dir.join("b")),
            shoot(&dir.join("c")),
        );
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        for f in [&a, &b, &c] {
            open_dir(&state, &app, &worker, f);
        }
        std::fs::remove_file(b.join("x.tif")).unwrap();
        press(&state, &app, &worker, true);
        assert_eq!(open_now(&state), Some(c.clone()), "b opened to nothing");
        assert_eq!(
            state.borrow().views.visited.here,
            Some(Place::Folder(c.clone()))
        );
        press(&state, &app, &worker, true);
        assert_eq!(open_now(&state), Some(a.clone()), "past b");
        assert_eq!(
            places(&state.borrow().views.visited.forward),
            [Place::Folder(c.clone()), Place::Folder(b.clone())]
        );
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The export queue is no place: entering and leaving it leaves the
    /// history as it was.
    #[test]
    fn the_queue_leaves_the_history_be() {
        let mut v = Visited::default();
        v.landed(Some(folder("a")), None);
        v.landed(Some(folder("b")), None);
        v.forward.push(stop("c"));
        v.landed(None, None);
        v.landed(Some(folder("b")), None);
        assert_eq!(places(&v.back), [folder("a")]);
        assert_eq!(places(&v.forward), [folder("c")]);
        assert_eq!(v.here, Some(folder("b")));
    }

    /// A look that comes back after another list opened is dropped:
    /// nothing opened, nothing asked.
    #[test]
    fn a_look_after_another_open_is_dropped() {
        let dir = scratch_dir("visited-late");
        let (a, b) = (shoot(&dir.join("a")), shoot(&dir.join("b")));
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        open_dir(&state, &app, &worker, &a);
        open_dir(&state, &app, &worker, &b);
        let press = {
            let mut st = state.borrow_mut();
            st.views.visited.looking = true;
            Press {
                back: true,
                generation: st.views.view_generation - 1,
                stack: st.views.visited.back.clone(),
                start: 0,
                gone: Vec::new(),
            }
        };
        arrive(&state, &app, &worker, press, &[Answer::There]);
        assert_eq!(crate::roots::sent_reads(), 0, "nothing opened");
        let st = state.borrow();
        assert!(!st.views.visited.looking);
        assert!(st.views.visited.asked.is_none());
        assert_eq!(places(&st.views.visited.back), [Place::Folder(a.clone())]);
        drop(st);
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A second press before the first's place lands, finding nothing
    /// further (the next place on a share not answering), leaves the
    /// first's ask be: its place lands as the back step.
    #[test]
    fn a_second_press_finding_nothing_keeps_the_first() {
        let dir = scratch_dir("visited-second");
        let (x, a, b) = (
            shoot(&dir.join("x")),
            shoot(&dir.join("a")),
            shoot(&dir.join("b")),
        );
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        for f in [&x, &a, &b] {
            open_dir(&state, &app, &worker, f);
        }
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(x.clone()));
        app.invoke_visit_folder(true);
        app.invoke_visit_folder(true);
        land_sent(&state, &app, &worker);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        assert_eq!(open_now(&state), Some(a.clone()));
        {
            let st = state.borrow();
            assert_eq!(places(&st.views.visited.back), [Place::Folder(x.clone())]);
            assert_eq!(
                places(&st.views.visited.forward),
                [Place::Folder(b.clone())]
            );
        }
        press(&state, &app, &worker, false);
        assert_eq!(open_now(&state), Some(b.clone()), "forward to b kept");
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// An open by hand of the place a failed press asked for is a new
    /// open, not the press's back step.
    #[test]
    fn an_open_by_hand_lets_go_of_the_ask() {
        let dir = scratch_dir("visited-hand");
        let (a, b, c) = (
            shoot(&dir.join("a")),
            shoot(&dir.join("b")),
            shoot(&dir.join("c")),
        );
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        for f in [&a, &b, &c] {
            open_dir(&state, &app, &worker, f);
        }
        std::fs::remove_file(b.join("x.tif")).unwrap();
        press(&state, &app, &worker, true);
        assert_eq!(open_now(&state), Some(c.clone()), "b opened to nothing");
        shoot(&b);
        open_dir(&state, &app, &worker, &b);
        let st = state.borrow();
        assert_eq!(
            places(&st.views.visited.back),
            [a, b.clone(), c].map(Place::Folder)
        );
        assert!(st.views.visited.forward.is_empty());
        assert_eq!(st.views.visited.here, Some(Place::Folder(b)));
        drop(st);
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Two presses out, the first's place landing late: the stacks move
    /// for it, and the second's ask stays, nearer by what was taken.
    #[test]
    fn a_first_press_landing_late_keeps_the_second() {
        let mut v = Visited::default();
        for p in ["a", "b", "c", "d"] {
            v.landed(Some(folder(p)), None);
        }
        v.asked = Some(Asked {
            back: true,
            at: 1,
            gone: Vec::new(),
            to: folder("b"),
        });
        v.landed(Some(folder("c")), None);
        assert_eq!(places(&v.back), [folder("a"), folder("b")]);
        assert_eq!(places(&v.forward), [folder("d")]);
        let asked = v.asked.clone().expect("the second press still out");
        assert_eq!((asked.at, asked.to.clone()), (0, folder("b")));
        v.landed(Some(folder("b")), None);
        assert_eq!(places(&v.back), [folder("a")]);
        assert_eq!(places(&v.forward), [folder("d"), folder("c")]);
    }

    /// Places gone taken off from the farthest, so the nearer keep
    /// their depth.
    #[test]
    fn places_gone_are_dropped_from_the_farthest() {
        let mut v = Visited {
            back: ["a", "b", "c", "d"].map(stop).to_vec(),
            ..Visited::default()
        };
        v.drop_gone(true, &[0, 2]);
        assert_eq!(places(&v.back), [folder("a"), folder("c")]);
    }

    /// The walk stops at the first place there, looking no further, and
    /// stops at its last look rather than passing places it has not
    /// looked at.
    #[test]
    fn the_walk_stops_at_the_first_there_and_at_its_reach() {
        let dir = scratch_dir("visited-walk");
        let here = shoot(&dir.join("here"));
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(dir.join("silent")));
        let answers = answer(&[Check::Look(here), Check::Look(dir.join("silent"))]);
        assert_eq!(answers, [Answer::There]);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        let mut checks: Vec<Check> = (0..LOOKS)
            .map(|i| Check::Look(dir.join(format!("gone{i}"))))
            .collect();
        checks.push(Check::There);
        let answers = answer(&checks);
        assert_eq!(answers.len(), LOOKS);
        assert!(answers.iter().all(|a| *a == Answer::Gone("not there")));
        crate::testing::remove_dir_retry(&dir);
    }

    fn left(place: &str, frame: Option<&str>) -> Stop {
        Stop {
            place: folder(place),
            frame: frame.map(PathBuf::from),
        }
    }

    fn file(name: &str) -> Option<PathBuf> {
        Some(PathBuf::from(name))
    }

    /// A place goes on the back stack with the frame current when it
    /// was left, or none; back gives the place left to the forward
    /// stack with its own frame, the places passed over keeping theirs.
    #[test]
    fn a_place_keeps_the_frame_it_was_left_on() {
        let mut v = Visited::default();
        v.landed(Some(folder("a")), file("a/1"));
        v.landed(Some(folder("b")), file("a/3"));
        v.landed(Some(folder("c")), None);
        v.landed(Some(folder("d")), file("c/2"));
        assert_eq!(
            v.back,
            [
                left("a", Some("a/3")),
                left("b", None),
                left("c", Some("c/2"))
            ]
        );
        // Back two, from d on d/5.
        v.asked = Some(Asked {
            back: true,
            at: 1,
            gone: Vec::new(),
            to: folder("b"),
        });
        v.landed(Some(folder("b")), file("d/5"));
        assert_eq!(v.back, [left("a", Some("a/3"))]);
        assert_eq!(
            v.forward,
            [left("d", Some("d/5")), left("c", Some("c/2"))],
            "c keeps its frame, d has the one it was left on"
        );
        // Forward one, from b on b/4.
        v.asked = Some(Asked {
            back: false,
            at: 0,
            gone: Vec::new(),
            to: folder("c"),
        });
        v.landed(Some(folder("c")), file("b/4"));
        assert_eq!(v.back, [left("a", Some("a/3")), left("b", Some("b/4"))]);
        assert_eq!(v.forward, [left("d", Some("d/5"))]);
    }

    /// The export queue's frame is not the place's: a place left from
    /// over the queue keeps none, one read again keeps its own.
    #[test]
    fn the_queue_s_frame_is_not_the_place_s() {
        let mut v = Visited::default();
        v.landed(Some(folder("a")), None);
        v.landed(None, file("a/2"));
        v.landed(Some(folder("b")), file("q/1"));
        assert_eq!(v.back, [left("a", None)]);
        v.landed(None, file("b/1"));
        v.landed(Some(folder("b")), file("q/1"));
        v.landed(Some(folder("c")), file("b/2"));
        assert_eq!(v.back, [left("a", None), left("b", Some("b/2"))]);
    }

    /// The stacks hold a hundred places, the oldest going first, each
    /// with its frame.
    #[test]
    fn the_stack_keeps_its_last_hundred() {
        let mut v = Visited::default();
        for i in 0..=KEPT + 1 {
            let prev = i.checked_sub(1).map(|p| format!("{p}/x"));
            v.landed(Some(folder(&i.to_string())), prev.map(PathBuf::from));
        }
        assert_eq!(v.back.len(), KEPT);
        assert_eq!(v.back[0], left("1", Some("1/x")));
        assert_eq!(v.back[KEPT - 1], left("100", Some("100/x")));
    }

    /// A folder of `n` frames, 0.tif and on.
    fn shoot_of(dir: &Path, n: usize) -> Vec<PathBuf> {
        std::fs::create_dir_all(dir).unwrap();
        (0..n)
            .map(|i| {
                let f = dir.join(format!("{i}.tif"));
                std::fs::write(&f, b"").unwrap();
                f
            })
            .collect()
    }

    fn current(state: &Rc<Shared>) -> Option<PathBuf> {
        frame_of(&state.borrow())
    }

    /// The frame `f` of the list made current, as a click on it does.
    fn pick(state: &Rc<Shared>, app: &App, f: &Path) {
        let row = {
            let st = state.borrow();
            let i = st.files.iter().position(|p| p == f).expect("in the list");
            crate::panel::browser::row_of(&st, i).expect("shown")
        };
        app.invoke_select(row as i32);
        assert_eq!(current(state).as_deref(), Some(f));
    }

    /// Back and forward between two folders from the disk land on the
    /// frame each was left on, not the first; a frame gone since falls
    /// back to the first, as before; an open by hand is as before.
    #[test]
    fn back_and_forward_land_on_the_frame_left() {
        let dir = scratch_dir("visited-frames");
        let dir = dunce::canonicalize(&dir).unwrap();
        let a = shoot_of(&dir.join("a"), 4);
        let b = shoot_of(&dir.join("b"), 3);
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        // As in a session: a list landing opens its frame.
        state.borrow_mut().launch.setup_ran = true;
        open_dir(&state, &app, &worker, &dir.join("a"));
        pick(&state, &app, &a[2]);
        open_dir(&state, &app, &worker, &dir.join("b"));
        assert_eq!(current(&state), Some(b[0].clone()), "a new open as before");
        pick(&state, &app, &b[1]);
        press(&state, &app, &worker, true);
        assert_eq!(open_now(&state), Some(dir.join("a")));
        assert_eq!(current(&state), Some(a[2].clone()), "back on a's frame");
        press(&state, &app, &worker, false);
        assert_eq!(open_now(&state), Some(dir.join("b")));
        assert_eq!(current(&state), Some(b[1].clone()), "forward on b's frame");
        // a's frame gone: back to a as an open of it would land.
        std::fs::remove_file(&a[2]).unwrap();
        press(&state, &app, &worker, true);
        assert_eq!(open_now(&state), Some(dir.join("a")));
        assert_eq!(
            current(&state),
            Some(a[0].clone()),
            "the frame gone, the first"
        );
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A root's folder, every root, and a collection, each left on a
    /// frame: back and forward land on it.
    #[test]
    fn views_and_collections_come_back_on_their_frame() {
        use greycard_library::fixture::{A7, R5, R6, write_frame};
        let dir = scratch_dir("visited-views");
        let dir = dunce::canonicalize(&dir).unwrap();
        let root = dir.join("photos");
        let (one, two) = (root.join("one"), root.join("two"));
        let mut files = Vec::new();
        for (k, folder) in [&one, &two].into_iter().enumerate() {
            std::fs::create_dir_all(folder).unwrap();
            for (i, n) in ["x.tif", "y.tif", "z.tif"].iter().enumerate() {
                let p = folder.join(n);
                write_frame(&p, [&R5, &R6, &A7][i], (k * 3 + i) as u16 + 1);
                files.push(p);
            }
        }
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&root, &mut |_| {}).unwrap();
        drop(writer);
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.launch.setup_ran = true;
            st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.views.sort = crate::sort::Kept::by_name();
            st.views.library.roots.add(&root).unwrap();
            st.views.collections.dir = Some(dir.join(crate::collections::DIR));
        }
        let view = |state: &Rc<Shared>, v: View| {
            crate::roots::open_view(state, &app, &worker, v);
            land_sent(state, &app, &worker);
        };
        view(&state, View::Roots(None));
        pick(&state, &app, &files[4]);
        // A collection of two's x and z.
        let id = {
            let mut st = state.borrow_mut();
            let at: Vec<usize> = [&files[3], &files[5]]
                .iter()
                .map(|f| st.files.iter().position(|p| p == *f).unwrap())
                .collect();
            let frames = crate::panel::collections::frames_of(&st, &at);
            let mut c = crate::collections::Collection::new("picks");
            c.add(&frames);
            let id = c.id.clone();
            st.views.collections.list.push(c);
            st.views.collections.save(&id);
            id
        };
        view(&state, crate::tree::view_for(&root, &one, false));
        pick(&state, &app, &files[1]);
        view(&state, View::Collection { id: id.clone() });
        pick(&state, &app, &files[5]);
        press(&state, &app, &worker, true);
        assert_eq!(
            current(&state),
            Some(files[1].clone()),
            "the folder on its frame"
        );
        press(&state, &app, &worker, true);
        assert_eq!(state.borrow().views.view, View::Roots(None));
        assert_eq!(
            current(&state),
            Some(files[4].clone()),
            "every root on its frame"
        );
        press(&state, &app, &worker, false);
        assert_eq!(current(&state), Some(files[1].clone()));
        press(&state, &app, &worker, false);
        assert_eq!(state.borrow().views.view, View::Collection { id });
        assert_eq!(
            current(&state),
            Some(files[5].clone()),
            "the collection on its frame"
        );
        crate::testing::remove_scratch(state, &dir);
    }
}
