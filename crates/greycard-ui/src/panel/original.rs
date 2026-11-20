//! Before and after on one key: the viewport shows the open frame as it
//! was, its Original, and back. The Original is the frame's default
//! edit (`reset::original_of`) under the current edit's geometry, the
//! lens's drawing of it included, so the picture holds still at the
//! zoom and the pan it is at. Its develop is made aside
//! (`Job::DevelopAside`), leaving the frame's own develop as it was,
//! once a frame and turn, and kept; so is the edit's picture it stood
//! over, so a press either way swaps two pictures already on the GPU.
//! An edit, a key, a turn, or another frame turns it back.

use std::rc::Rc;
use std::sync::Arc;

use crate::tr;
use greycard_edit::Edit;
use slint::{ComponentHandle, SharedString};

use crate::panel::deliver::Landed;
use crate::panel::edit::{current_turn, edit_to_develop, read_edit};
use crate::worker::{Developed, Job, WhiteBase, Worker};
use crate::{App, Shared, State, WORKER, finish};

/// What the status line says while the Original shows.
pub(crate) fn word() -> String {
    tr!("Original")
}

/// The Original on show, and what it swaps.
#[derive(Default)]
pub(crate) struct Original {
    /// The Original is on show, or on its way.
    pub(crate) on: bool,
    /// What it shows: the frame's default under the edit's geometry.
    pub(crate) edit: Option<Edit>,
    /// The frame and the turn its picture is of.
    of: Option<(crate::versions::Key, u8)>,
    /// The develop of it asked for and not yet up, by its generation.
    pub(crate) asked: Option<u64>,
    /// Its picture once up, kept while the frame is open at its turn.
    picture: Option<Shown>,
    /// The edit's picture it stands over, put back after; none when
    /// that was not the edit's latest develop.
    edited: Option<Shown>,
    /// The status line it covers.
    status: Option<SharedString>,
    /// The open of the file read again while it was on show, by its
    /// generation (`rewritten`).
    reread: Option<u64>,
}

/// A picture that has been up on the GPU, with what it went up with:
/// none of the texture without a GPU, and then it cannot go up again.
struct Shown {
    texture: Option<greycard_gpu::wgpu::Texture>,
    /// The picture's own size as the texture stands, which a texture
    /// that went up reduced to fit the device is not.
    texture_size: Option<(u32, u32)>,
    kind: finish::Source,
    guide: Arc<finish::Guide>,
    planes: Option<Arc<crate::detail::DetailPlanes>>,
    white: WhiteBase,
    size: (u32, u32),
    turn: u8,
}

impl Shown {
    /// The picture on the GPU now, when there is a GPU and it is a
    /// develop: what went up with it, as it went up.
    fn on_screen(st: &State) -> Option<Self> {
        let renderer = st.develop.renderer.as_ref()?;
        let texture = renderer.developed()?.clone();
        Some(Self {
            texture: Some(texture),
            texture_size: renderer.developed_size(),
            kind: st.develop.drawn_kind,
            guide: st.develop.drawn_guide.clone(),
            planes: st.develop.drawn_planes.clone(),
            white: st.develop.base_white?,
            size: st.develop.shown_size,
            turn: st.develop.shown_turn?.1,
        })
    }

    /// `landed`, just gone up on the GPU at `turn`: its own white,
    /// guide, and planes, and the texture the renderer made of it.
    fn landed(st: &State, landed: &Landed, turn: u8) -> Self {
        let renderer = st.develop.renderer.as_ref();
        Self {
            texture: renderer.and_then(|r| r.developed()).cloned(),
            texture_size: renderer.and_then(|r| r.developed_size()),
            kind: landed.kind,
            guide: landed.guide.clone(),
            planes: landed.planes.clone(),
            white: landed.white,
            size: (landed.image.width(), landed.image.height()),
            turn,
        }
    }

    /// Up again on the next frame, as a develop of `generation`; false
    /// with no texture to put up.
    fn put_up(&self, st: &mut State, generation: u64) -> bool {
        let Some(texture) = &self.texture else {
            return false;
        };
        st.develop.shown_size = self.size;
        st.develop.shown_turn = st.current.map(|c| (c, self.turn % 4));
        st.develop.planes_wants = self.planes.as_ref().map(|p| p.wants).unwrap_or_default();
        st.develop.planes_asked = Default::default();
        st.develop.pending = Some(Landed {
            image: Developed::Texture(texture.clone()),
            kind: self.kind,
            guide: self.guide.clone(),
            planes: self.planes.clone(),
            white: self.white,
            generation,
            full: self.texture_size,
        });
        true
    }
}

/// The edit the viewport draws under while the Original's picture is
/// up; none while it is off or still on its way.
pub(crate) fn shown(st: &State) -> Option<&Edit> {
    let o = &st.develop.original;
    o.edit.as_ref().filter(|_| o.on && o.asked.is_none())
}

/// The frame's Original under `edit`'s geometry: its crop, turn,
/// straighten, and aspect, and the lens's drawing of the picture (the
/// distortion, by the profile and by hand, the scale, and the lens
/// chosen), so the picture stands where the edit's does. The lens's
/// vignetting and CA are the Original's own.
pub(crate) fn under_geometry(mut original: Edit, edit: &Edit) -> Edit {
    original.geometry = edit.geometry.clone();
    let (from, to) = (&edit.lens, &mut original.lens);
    to.chosen = from.chosen.clone();
    to.auto_scale = from.auto_scale;
    if from.enabled {
        to.distortion = from.profile && from.distortion;
        to.manual = from.manual;
        to.scale = from.scale;
    } else {
        to.distortion = false;
        to.manual = 0.0;
        to.scale = 1.0;
    }
    original
}

/// Whether the edit's picture on screen is all of it: the latest
/// develop, up, with no generative fill and no highlight fill still to
/// come. Only then is it kept to put back after the Original; else
/// the edit develops again, so a round given way to is made again.
pub(crate) fn edit_picture_whole(st: &State) -> bool {
    st.develop.pending.is_none()
        && st.develop.drawn_generation == st.develop.generation
        && st.develop.fills_pending.is_empty()
        && !st.develop.highlight_pending
}

/// The key: the Original if the edit shows, the edit if the Original
/// does. Only over the open frame's own develop, up and not culled.
pub(crate) fn key(st: &mut State, app: &App, worker: &Worker) {
    if st.develop.original.on {
        off(st, app, false);
        return;
    }
    let Some(c) = st.current else {
        return;
    };
    if st.culling.cull.is_some()
        || !crate::panel::edit::develops_here(st)
        || crate::panel::viewport::own_develop(st).is_none()
        || crate::panel::cull::standing_in(st).is_some()
    {
        return;
    }
    let (Some(frame), Some(original)) = (st.key_of(c), crate::panel::reset::original_of(st, c))
    else {
        return;
    };
    let panel = read_edit(app, &st.develop.edit, st.develop.target);
    let edit = under_geometry(original, &panel);
    let turn = current_turn(st);
    // The edit's picture is put back as it is only when it is the
    // latest develop of it, all made: else the edit develops again.
    let edited = if edit_picture_whole(st) {
        Shown::on_screen(st)
    } else {
        None
    };
    let o = &mut st.develop.original;
    // Kept of this frame at this turn, under this lens drawing.
    let of = Some((frame, turn % 4));
    if o.of != of || o.edit.as_ref().map(|e| &e.lens) != Some(&edit.lens) {
        o.picture = None;
        o.of = of;
    }
    o.edited = edited;
    o.on = true;
    o.edit = Some(edit.clone());
    let status = app.get_status();
    let word = word();
    if status != word.as_str() {
        o.status = Some(status);
    }
    app.set_status(word.into());
    app.set_original_on(true);
    // A develop on its way, either picture's, is not wanted now.
    st.develop.generation += 1;
    let generation = st.develop.generation;
    let kept = st.develop.original.picture.take();
    let up = kept.as_ref().is_some_and(|p| p.put_up(st, generation));
    if up {
        st.develop.original.picture = kept;
        st.develop.original.asked = None;
    } else {
        st.develop.original.asked = Some(generation);
        app.set_busy(true);
        worker.send(Job::DevelopAside {
            edit: edit_to_develop(app, &edit),
            generation,
            turn,
            of: crate::panel::edit::develop_of(st),
        });
    }
    app.window().request_redraw();
}

/// The edit shows again. `follows`: the caller develops the edit next,
/// so a picture of it not kept is not asked for here.
pub(crate) fn off(st: &mut State, app: &App, follows: bool) {
    if app.get_original_on() {
        app.set_original_on(false);
    }
    let o = &mut st.develop.original;
    if !o.on {
        return;
    }
    o.on = false;
    o.asked = None;
    o.reread = None;
    let edited = o.edited.take();
    if let Some(status) = o.status.take()
        && app.get_status() == word().as_str()
    {
        app.set_status(status);
    }
    st.develop.generation += 1;
    let generation = st.develop.generation;
    if edited.is_some_and(|p| p.put_up(st, generation)) {
        app.set_busy(false);
    } else if !follows && crate::panel::edit::develops_here(st) {
        app.set_busy(true);
        let edit = edit_to_develop(app, &st.develop.edit);
        let turn = current_turn(st);
        WORKER.with(|w| {
            if let Some(w) = &*w.borrow() {
                w.send(Job::Develop {
                    edit,
                    generation,
                    turn,
                    of: crate::panel::edit::develop_of(st),
                });
            }
        });
    }
    app.window().request_redraw();
}

/// The frame is left: everything kept of it goes, its texture with it.
/// The Original's edit when it was on show, which the picture still on
/// screen is drawn under until the next frame's lands.
pub(crate) fn forget(st: &mut State) -> Option<Edit> {
    let o = std::mem::take(&mut st.develop.original);
    o.edit.filter(|_| o.on && o.asked.is_none())
}

/// The Original's picture kept is not its develop any more (a turn, a
/// profile or a lens database changed under it): the next press
/// develops it again.
pub(crate) fn drop_picture(st: &mut State) {
    st.develop.original.picture = None;
    st.develop.original.of = None;
}

/// The open frame's file is read again at `generation`
/// (`browser::reread_open`): both pictures kept are of the old bytes
/// and go, the edit's the Original stands over too, so turning it off
/// develops the edit of the new bytes rather than putting the old one
/// up. On show, it stays on: once the file is open again
/// ([`reread_opened`]) its Original of the new bytes is asked for.
pub(crate) fn rewritten(st: &mut State, generation: u64) {
    let o = &mut st.develop.original;
    o.picture = None;
    o.edited = None;
    o.reread = o.on.then_some(generation);
}

/// The worker has opened the file read again at `generation` while the
/// Original was on show: its Original of the new bytes is developed
/// aside, at a generation of its own, in place of the edit's develop of
/// that open, which nothing waits for.
pub(crate) fn reread_opened(st: &mut State, app: &App, generation: u64) {
    let o = &mut st.develop.original;
    if !o.on || o.reread != Some(generation) {
        return;
    }
    o.reread = None;
    let Some(edit) = o.edit.clone() else {
        return;
    };
    // The worker develops the edit of the open all the same, and the
    // aside leaves that develop as it is: the edit's pictures of the
    // new bytes are kept from it, as a develop landing keeps them, or
    // the strip would show the camera's until the Original is off.
    crate::edited::landed(st, current_turn(st));
    st.develop.generation += 1;
    let generation = st.develop.generation;
    st.develop.original.asked = Some(generation);
    let turn = current_turn(st);
    WORKER.with(|w| {
        if let Some(w) = &*w.borrow() {
            w.send(Job::DevelopAside {
                edit: edit_to_develop(app, &edit),
                generation,
                turn,
                of: crate::panel::edit::develop_of(st),
            });
        }
    });
}

/// A develop of `generation` is delivered: whether it is the
/// Original's. Any other while the Original is on is the edit's, asked
/// for by a change made meanwhile, and the Original gives way to it as
/// it lands: no generation moved on, so the rounds of fills after it
/// still land, and nothing put up over it.
pub(crate) fn landing(st: &mut State, app: &App, generation: u64) -> bool {
    let o = &mut st.develop.original;
    if !o.on {
        return false;
    }
    if o.asked == Some(generation) {
        return true;
    }
    o.on = false;
    o.asked = None;
    o.edited = None;
    if let Some(status) = o.status.take()
        && app.get_status() == word().as_str()
    {
        app.set_status(status);
    }
    app.set_original_on(false);
    false
}

/// `landed` has gone up on the GPU: the Original's is kept for the
/// next press, with its own white and planes.
pub(crate) fn went_up(st: &mut State, landed: &Landed) {
    if st.develop.original.asked != Some(landed.generation) {
        return;
    }
    let turn = st.develop.original.of.as_ref().map_or(0, |(_, t)| *t);
    st.develop.original.asked = None;
    st.develop.original.picture = Some(Shown::landed(st, landed, turn));
}

/// The white the Original's kept picture went up with, for a test.
#[cfg(test)]
pub(crate) fn kept_white(st: &State) -> Option<WhiteBase> {
    st.develop.original.picture.as_ref().map(|p| p.white)
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_original_key(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            key(&mut state.borrow_mut(), &app, &worker);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_original_off(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            off(&mut state.borrow_mut(), &app, false);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use greycard_edit::geometry::Crop;
    use slint::ComponentHandle;

    use super::*;
    use crate::testing::{press, window};

    /// A frame open with its develop up, the panel's edit at an
    /// exposure and a crop.
    fn developed_frame() -> (App, Rc<Shared>, Rc<Worker>, Edit) {
        let app = window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        app.set_placeholder(false);
        let mut edit = Edit::default();
        edit.light.exposure = 1.0;
        edit.geometry.crop = Some(Crop {
            x: 0.1,
            y: 0.2,
            w: 0.5,
            h: 0.6,
        });
        {
            let mut st = state.borrow_mut();
            st.develop.held = None;
            st.develop.peek = None;
            st.develop.shown_size = (6000, 4000);
            st.develop.source_size = (6000, 4000);
            st.develop.shown_turn = Some((0, current_turn(&st)));
            st.develop.image_size = (6000, 4000);
            st.develop.drawn_generation = st.develop.generation;
            st.develop.base_white = Some(WhiteBase::IDENTITY);
            crate::panel::edit::show_edit(&st, &edit, &app, None);
            st.develop.edit = edit.clone();
        }
        app.set_status("Portra 400 onto 2 frames".into());
        (app, state, worker, edit)
    }

    /// The edit's own white: gains a raw's develop would carry.
    fn edits_white() -> WhiteBase {
        WhiteBase {
            gains: [2.0, 1.0, 1.5],
            ..WhiteBase::IDENTITY
        }
    }

    /// The Original's: another.
    fn originals_white() -> WhiteBase {
        WhiteBase {
            gains: [1.8, 1.0, 1.7],
            ..WhiteBase::IDENTITY
        }
    }

    /// The develop of `generation` delivered, as the worker would, at
    /// `white` and with `fills` still to make.
    fn deliver_with(app: &App, generation: u64, white: WhiteBase, fills: Vec<u64>) {
        crate::panel::deliver::deliver(
            app,
            crate::worker::Outcome::Developed {
                generation,
                turn: 0,
                kind: crate::finish::Source::SCENE,
                image: Developed::Halves(Arc::new(crate::worker::Halves {
                    width: 6000,
                    height: 4000,
                    pixels: Vec::new(),
                })),
                guide: Arc::new(crate::finish::Guide::NONE),
                planes: None,
                white,
                seconds: 0.1,
                sharpen: None,
                sources: Vec::new(),
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport {
                    pending: fills,
                    ..Default::default()
                },
            },
        );
    }

    fn deliver(app: &App, generation: u64) {
        deliver_with(app, generation, originals_white(), Vec::new());
    }

    /// What the render does with a develop that has landed.
    fn land(state: &Rc<Shared>, app: &App) {
        let mut st = state.borrow_mut();
        crate::panel::startup::land_pending(&mut st, app);
    }

    /// The backslash shows the Original, its own develop asked once and
    /// drawn under the edit's crop, the status line saying so; a
    /// second press, any other key, or an edit shows the edit again.
    #[test]
    fn the_backslash_shows_the_original_under_the_same_crop() {
        let (app, state, _worker, edit) = developed_frame();
        let drawn = |state: &Rc<Shared>, app: &App| {
            let st = state.borrow();
            let panel = read_edit(app, &st.develop.edit, st.develop.target);
            crate::panel::startup::view_edit(&st, &panel)
        };
        assert_eq!(drawn(&state, &app).light.exposure, 1.0);
        press(&app, "\\");
        assert!(app.get_original_on());
        assert_eq!(app.get_status(), word());
        let asked = state
            .borrow()
            .develop
            .original
            .asked
            .expect("its develop asked");
        assert_eq!(asked, state.borrow().develop.generation);
        // Until its picture is up the edit's is drawn as it was.
        assert_eq!(drawn(&state, &app).light.exposure, 1.0);
        deliver(&app, asked);
        assert_eq!(
            app.get_status(),
            word(),
            "the Original's word, once its develop lands"
        );
        assert!(
            state.borrow().develop.pending.is_some(),
            "its picture goes up"
        );
        land(&state, &app);
        assert_eq!(state.borrow().develop.original.asked, None);
        let original = drawn(&state, &app);
        assert_eq!(original.light.exposure, 0.0, "a different picture");
        assert_eq!(original.geometry, edit.geometry, "the same crop");

        // A second press: the edit, and its line back.
        press(&app, "\\");
        assert!(!app.get_original_on());
        assert!(!state.borrow().develop.original.on);
        assert_eq!(app.get_status(), "Portra 400 onto 2 frames");
        assert_eq!(drawn(&state, &app).light.exposure, 1.0);

        // Any other key turns it back too.
        press(&app, "\\");
        assert!(app.get_original_on());
        press(&app, "J");
        assert!(!app.get_original_on());
        assert!(!state.borrow().develop.original.on);

        // And an edit.
        press(&app, "\\");
        assert!(state.borrow().develop.original.on);
        app.set_exposure(0.5);
        app.invoke_view_changed();
        assert!(!state.borrow().develop.original.on);
        assert!(!app.get_original_on());
        assert_ne!(app.get_status(), word());
    }

    /// The Original's picture is kept with its own white, not the
    /// edit's base it went up over.
    #[test]
    fn the_original_keeps_its_own_white() {
        let (app, state, _worker, _) = developed_frame();
        state.borrow_mut().develop.base_white = Some(edits_white());
        press(&app, "\\");
        let asked = state.borrow().develop.original.asked.unwrap();
        deliver_with(&app, asked, originals_white(), Vec::new());
        land(&state, &app);
        let st = state.borrow();
        let kept = kept_white(&st).expect("its picture kept");
        assert_eq!(kept.gains, originals_white().gains);
        assert_eq!(
            st.develop.base_white.unwrap().gains,
            originals_white().gains
        );
    }

    /// A turn between presses: the Original kept is of the turn before,
    /// and its develop is asked again.
    #[test]
    fn a_turn_between_presses_develops_the_original_again() {
        let (app, state, _worker, _) = developed_frame();
        press(&app, "\\");
        let asked = state.borrow().develop.original.asked.unwrap();
        deliver(&app, asked);
        land(&state, &app);
        assert!(state.borrow().develop.original.picture.is_some());
        press(&app, "\\");
        app.invoke_frame_turned(1);
        assert!(state.borrow().develop.original.picture.is_none());
        {
            // Its develop at the new turn is up.
            let mut st = state.borrow_mut();
            st.develop.shown_turn = Some((0, current_turn(&st)));
            st.develop.pending = None;
        }
        press(&app, "\\");
        assert!(
            state.borrow().develop.original.asked.is_some(),
            "the turn before's Original put up"
        );
    }

    /// A develop of the edit that lands while the Original is on (a
    /// change made by a path that asked no turn back) takes its place
    /// at its own generation, and its fills after it still land.
    #[test]
    fn the_edits_develop_turns_the_original_back_and_its_fills_land() {
        let (app, state, _worker, _) = developed_frame();
        press(&app, "\\");
        let generation = {
            let mut st = state.borrow_mut();
            st.develop.generation += 1;
            st.develop.generation
        };
        deliver_with(&app, generation, edits_white(), vec![7]);
        assert!(!state.borrow().develop.original.on);
        assert!(!app.get_original_on());
        assert_ne!(app.get_status(), word());
        assert_eq!(state.borrow().develop.generation, generation);
        assert_eq!(state.borrow().develop.fills_pending, vec![7]);
        land(&state, &app);
        // The fill's round, of the same generation, lands.
        deliver_with(&app, generation, edits_white(), Vec::new());
        assert!(state.borrow().develop.pending.is_some(), "the fill's round");
        assert!(state.borrow().develop.fills_pending.is_empty());
    }

    /// The Original draws the lens as the edit does, the distortion
    /// and the scale, so the crop frames the same picture; its
    /// vignetting and CA are its own.
    #[test]
    fn the_original_carries_the_lens_drawing() {
        let mut edit = Edit::default();
        edit.lens.distortion = false;
        edit.lens.manual = -0.05;
        edit.lens.auto_scale = false;
        edit.lens.scale = 1.1;
        edit.lens.vignetting = false;
        edit.lens.chromatic_aberration = false;
        let original = under_geometry(Edit::default(), &edit);
        assert!(!original.lens.distortion);
        assert_eq!(original.lens.manual, -0.05);
        assert!(!original.lens.auto_scale);
        assert_eq!(original.lens.scale, 1.1);
        assert!(original.lens.vignetting && original.lens.chromatic_aberration);
        // The section off draws nothing of the lens.
        edit.lens.enabled = false;
        let original = under_geometry(Edit::default(), &edit);
        assert!(!original.lens.distortion);
        assert_eq!((original.lens.manual, original.lens.scale), (0.0, 1.0));
        assert!(original.lens.enabled && original.lens.vignetting);
    }

    /// The key is on the keys sheet, and may be bound to another.
    #[test]
    fn the_key_is_on_the_sheet() {
        let line = crate::keys::TABLE
            .iter()
            .flat_map(|g| g.lines.iter())
            .find(|b| b.action == Some("original"))
            .expect("the line");
        assert_eq!(line.chords, &["\\"]);
        assert_eq!(line.does, "Show the frame as it was, and back");
    }
}
