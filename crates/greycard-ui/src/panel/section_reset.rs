//! A section's own reset, the icon in its header: that section of the
//! frame's edit put back to what a new edit has for it, as one step
//! its history can go back from, the rest of the edit left as it is.
//! On the Masks tab it is the chosen mask's section, put back to what
//! a new mask has; the mask, its shapes, and its other sections stay.
//!
//! What a new edit has is the frame's Original ([`original_of`]), the
//! very state Reset to Original puts the whole edit back to, so a
//! header's reset and Reset to Original agree section by section. The
//! step is recorded without words of its own, as a section's Reset
//! button's always was: the history names it for what moved ("Color
//! tint reset", "White balance as shot").

use std::rc::Rc;

use greycard_edit::{Edit, Look};
use slint::ComponentHandle;

use crate::panel::edit::record_panel;
use crate::panel::history::take_current;
use crate::panel::queue_view::refused;
use crate::panel::reset::original_of;
use crate::worker::Worker;
use crate::{App, SectionResets, Shared, State};

/// The sections with a reset in their header, by the names their order
/// keeps them under: Develop's, then the Crop tab's. Not the Masks
/// tab's mask section (its shapes are the mask), Retouch (a reset would
/// take every stroke and fill at a click), nor the panel's sections
/// that are no part of the edit (the proof, the monitor, the metadata).
pub(crate) const SECTIONS: &[&str] = &[
    "look",
    "white-balance",
    "camera",
    "light",
    "color",
    "curves",
    "mixer",
    "black-and-white",
    "grading",
    "tint",
    "detail",
    "noise",
    "lens",
    "sharpen",
    "output-sharpen",
    "vignette",
    "grain",
    "demosaic",
    "crop",
    "rotate",
    "perspective",
];

/// The sections a mask has of its own, on the Masks tab.
pub(crate) const MASK_SECTIONS: &[&str] = &[
    "white-balance",
    "light",
    "color",
    "curves",
    "mixer",
    "grading",
    "tint",
    "detail",
];

/// Put section `name` of `edit` back to `original`'s, the frame's
/// Original; with `target` a mask, that mask's section back to what a
/// new mask has, its white balance to none of its own (`Off`: no light
/// kept, Follow global off). False, and `edit` untouched, for a section
/// with no reset there or a mask the edit does not have.
pub(crate) fn reset_section(
    edit: &mut Edit,
    original: &Edit,
    target: Option<usize>,
    name: &str,
) -> bool {
    if let Some(i) = target {
        let Some(look) = edit.adjustments.get_mut(i).map(|a| &mut a.look) else {
            return false;
        };
        let fresh = Look::default();
        match name {
            "white-balance" => look.white_balance = fresh.white_balance,
            "light" => look.light = fresh.light,
            "color" => look.color = fresh.color,
            "curves" => look.curves = fresh.curves,
            "mixer" => look.mixer = fresh.mixer,
            "grading" => look.grading = fresh.grading,
            "tint" => look.tint = fresh.tint,
            "detail" => look.detail = fresh.detail,
            _ => return false,
        }
        return true;
    }
    let (g, o) = (&mut edit.geometry, &original.geometry);
    match name {
        "look" => edit.look_lut = original.look_lut.clone(),
        "white-balance" => edit.white_balance = original.white_balance.clone(),
        "camera" => edit.camera = original.camera.clone(),
        // The display curve's choice is in the section, under the
        // sliders, on a raw (`reset_base` keeps it elsewhere); and the
        // zero an edit from before 0.8 keeps goes to the body's, as
        // Reset to Original takes it.
        "light" => {
            edit.light = original.light;
            edit.display_curve = original.display_curve;
        }
        "color" => edit.color = original.color,
        "curves" => edit.curves = original.curves.clone(),
        "mixer" => edit.mixer = original.mixer,
        "black-and-white" => edit.bw = original.bw,
        "grading" => edit.grading = original.grading,
        "tint" => edit.tint = original.tint,
        "detail" => edit.detail = original.detail,
        // The learned tier and its blend too: what the section shows,
        // from the frame's ISO as a new edit has it.
        "noise" => edit.noise = original.noise.clone(),
        "lens" => edit.lens = original.lens.clone(),
        "sharpen" => edit.sharpen = original.sharpen,
        "output-sharpen" => edit.output_sharpen = original.output_sharpen,
        "vignette" => edit.vignette = original.vignette,
        "grain" => edit.grain = original.grain,
        "demosaic" => edit.demosaic = original.demosaic,
        "crop" => {
            g.crop = o.crop;
            g.aspect = o.aspect;
            g.portrait = o.portrait;
        }
        // The quarter turns and the mirror taken back by the very moves
        // that made them, which carry the crop and the keystone with
        // the picture: the crop frames what it framed, and the keystone
        // stays on its axis. The straighten is a number.
        "rotate" => {
            let mut back = g.turned(-i32::from(g.turns));
            if back.flip != o.flip {
                back = back.flipped(false);
            }
            back = back.turned(i32::from(o.turns));
            back.angle = o.angle;
            *g = back;
        }
        // What a new edit has, no keystone, reads the same whichever
        // way the picture is turned.
        "perspective" => {
            g.vertical = o.vertical;
            g.horizontal = o.horizontal;
        }
        _ => return false,
    }
    true
}

/// Whether section `name` of `edit` stands where [`reset_section`]
/// would put it: nothing for its reset to do. True for a section with
/// no reset there.
pub(crate) fn section_at_reset(
    edit: &Edit,
    original: &Edit,
    target: Option<usize>,
    name: &str,
) -> bool {
    if let Some(i) = target {
        let Some(look) = edit.adjustments.get(i).map(|a| &a.look) else {
            return true;
        };
        let fresh = Look::default();
        return match name {
            "white-balance" => look.white_balance == fresh.white_balance,
            "light" => look.light == fresh.light,
            "color" => look.color == fresh.color,
            "curves" => look.curves == fresh.curves,
            "mixer" => look.mixer == fresh.mixer,
            "grading" => look.grading == fresh.grading,
            "tint" => look.tint == fresh.tint,
            "detail" => look.detail == fresh.detail,
            _ => true,
        };
    }
    let (g, o) = (&edit.geometry, &original.geometry);
    match name {
        "look" => edit.look_lut == original.look_lut,
        "white-balance" => edit.white_balance == original.white_balance,
        "camera" => edit.camera == original.camera,
        "light" => edit.light == original.light && edit.display_curve == original.display_curve,
        "color" => edit.color == original.color,
        "curves" => edit.curves == original.curves,
        "mixer" => edit.mixer == original.mixer,
        "black-and-white" => edit.bw == original.bw,
        "grading" => edit.grading == original.grading,
        "tint" => edit.tint == original.tint,
        "detail" => edit.detail == original.detail,
        "noise" => edit.noise == original.noise,
        "lens" => edit.lens == original.lens,
        "sharpen" => edit.sharpen == original.sharpen,
        "output-sharpen" => edit.output_sharpen == original.output_sharpen,
        "vignette" => edit.vignette == original.vignette,
        "grain" => edit.grain == original.grain,
        "demosaic" => edit.demosaic == original.demosaic,
        "crop" => g.crop == o.crop && g.aspect == o.aspect && g.portrait == o.portrait,
        "rotate" => g.turns == o.turns && g.flip == o.flip && g.angle == o.angle,
        "perspective" => g.vertical == o.vertical && g.horizontal == o.horizontal,
        _ => true,
    }
}

/// `live`'s field for section `name`.
fn field<'a>(live: &'a mut SectionResets, name: &str) -> Option<&'a mut bool> {
    Some(match name {
        "look" => &mut live.look,
        "white-balance" => &mut live.white_balance,
        "camera" => &mut live.camera,
        "light" => &mut live.light,
        "color" => &mut live.color,
        "curves" => &mut live.curves,
        "mixer" => &mut live.mixer,
        "black-and-white" => &mut live.black_and_white,
        "grading" => &mut live.grading,
        "tint" => &mut live.tint,
        "detail" => &mut live.detail,
        "noise" => &mut live.noise,
        "lens" => &mut live.lens,
        "sharpen" => &mut live.sharpen,
        "output-sharpen" => &mut live.output_sharpen,
        "vignette" => &mut live.vignette,
        "grain" => &mut live.grain,
        "demosaic" => &mut live.demosaic,
        "crop" => &mut live.crop,
        "rotate" => &mut live.rotate,
        "perspective" => &mut live.perspective,
        _ => return None,
    })
}

/// Which headers' resets have something to do, for `edit` on the panel
/// with `target` chosen: none with no frame, or while the panel is not
/// the frame's own.
pub(crate) fn resets_for(st: &State, edit: &Edit, target: Option<usize>) -> SectionResets {
    let mut live = SectionResets::default();
    let original = st
        .current
        .filter(|&c| st.culling.cull.is_none() && crate::rows::panel_is_frames(st, c))
        .and_then(|c| reset_base(st, c, edit));
    let Some(original) = original else {
        return live;
    };
    let names = if target.is_some() {
        MASK_SECTIONS
    } else {
        SECTIONS
    };
    for name in names {
        if let Some(f) = field(&mut live, name) {
            *f = !superseded(st, edit, target, name)
                && !section_at_reset(edit, &original, target, name);
        }
    }
    live
}

/// What cell `c`'s resets put back for `edit`: its Original, but with
/// `edit`'s display curve where the Light section does not show the
/// choice (on a picture), so a reset there leaves what it cannot show.
fn reset_base(st: &State, c: usize, edit: &Edit) -> Option<Edit> {
    let mut base = original_of(st, c)?;
    if crate::panel::color::open_is_picture(st) {
        base.display_curve = edit.display_curve;
    }
    Some(base)
}

/// Whether section `name` does not act for now, its switch dead and its
/// reset with it: the global mixer under black and white, the white
/// balance of a monochrome frame (the global's and a mask's).
fn superseded(st: &State, edit: &Edit, target: Option<usize>, name: &str) -> bool {
    match name {
        "mixer" => target.is_none() && edit.bw.enabled,
        "white-balance" => crate::panel::color::open_is_monochrome(st),
        _ => false,
    }
}

/// [`resets_for`] on the panel.
pub(crate) fn show_section_resets(st: &State, app: &App, edit: &Edit, target: Option<usize>) {
    let live = resets_for(st, edit, target);
    if app.get_section_resets() != live {
        app.set_section_resets(live);
    }
}

/// The header's reset of section `name` on the frame on screen: what
/// the panel holds recorded first, as a preset over it is, then the
/// section put back as a step of its own, on the panel, saved, and
/// developed when the develop's part moved. Nothing in the export
/// queue's view, with no frame, in culling, while the panel stands in
/// for a frame whose sidecar is not in, or with the section where its
/// reset puts it already.
pub(crate) fn reset_on_screen(st: &mut State, app: &App, worker: &Worker, name: &str) {
    if refused(st, "a reset") {
        return;
    }
    let Some(c) = st.current else {
        return;
    };
    if st.culling.cull.is_some() {
        return;
    }
    if let Some(why) = crate::rows::not_kept(st, c) {
        app.set_status(why.into());
        return;
    }
    let target = st.develop.target;
    // A move on the panel the save has yet to record is a step first,
    // as a preset over it records it; with none waiting the panel is
    // the state, and the reset goes over the state itself, so nothing
    // the panel holds only near (a white balance's slider is an f32,
    // the edit's an f64) is written over a section left alone.
    if st.develop.save_timer.running() {
        st.develop.save_timer.stop();
        record_panel(st, app);
    }
    let Some(edit) = st.line(c).map(|l| l.current.clone()) else {
        return;
    };
    let Some(original) = reset_base(st, c, &edit) else {
        return;
    };
    // A click that reached a dead section's reset all the same.
    if superseded(st, &edit, target, name) {
        return;
    }
    let mut reset = edit.clone();
    if !reset_section(&mut reset, &original, target, name) || reset == edit {
        return;
    }
    // A turn or a keystone put back with a crop kept: the crop fitted
    // to the plane it now lies on, as a turn or a slider fits it.
    if target.is_none() && matches!(name, "rotate" | "perspective") && reset.geometry.crop.is_some()
    {
        crate::panel::crop::show_geometry(&reset.geometry, app);
        crate::panel::crop::refit_crop(
            app,
            st.develop.source_size.0 as f32,
            st.develop.source_size.1 as f32,
            !st.develop.edit.needs_frame(),
            crate::panel::viewport::camera_crop(st),
        );
        reset.geometry = crate::panel::crop::read_geometry(app);
    }
    if !st.line_mut(c).is_some_and(|line| line.record(reset)) {
        return;
    }
    tracing::info!("section reset: {name}");
    st.develop.status_kept = None;
    take_current(st, app, worker);
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
    app.on_section_reset(move |name| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        reset_on_screen(&mut state.borrow_mut(), &app, &worker, name.as_str());
    });
}

#[cfg(test)]
mod tests {
    use greycard_edit::geometry::{Aspect, Crop};
    use greycard_edit::{Adjustment, Demosaic, DisplayCurve, LocalWhite, WhiteBalance};
    use slint::platform::WindowEvent;

    use super::*;
    use crate::panel::edit::read_edit;
    use crate::testing::{click, folder, header_reset, state_for, window};

    /// A look with every section a mask has moved off a new mask's.
    fn moved_look() -> Look {
        let mut look = Look::default();
        look.light.exposure = 0.8;
        look.light.tone.shadows = 0.3;
        look.curves.enabled = false;
        look.curves.parametric.lights = 0.2;
        look.mixer.hue[2] = 0.25;
        look.color.saturation = 0.3;
        look.grading.balance = 0.4;
        look.tint.amount = 0.3;
        look.white_balance = LocalWhite::Absolute {
            temperature: 4200.0,
            tint: 0.004,
        };
        look.detail.clarity = 0.2;
        look
    }

    /// An edit with every section that has a header reset moved off a
    /// new edit's, and nothing else.
    fn moved() -> Edit {
        let mut e = Edit::default();
        e.look_lut.strength = 0.5;
        e.white_balance = WhiteBalance::Custom {
            temperature: 4000.0,
            tint: 0.01,
        };
        e.camera.look_table = !e.camera.look_table;
        e.light.exposure = 1.0;
        e.light.baseline = Some(-0.5);
        e.display_curve = DisplayCurve::Agx;
        e.color.saturation = 0.4;
        e.curves.rgb = vec![[0.0, 0.1], [1.0, 1.0]];
        e.mixer.hue[0] = 0.2;
        e.bw.enabled = !e.bw.enabled;
        e.grading.balance = -0.3;
        e.tint.amount = 0.25;
        e.detail.texture = 0.3;
        e.noise.strength += 0.5;
        e.noise.learned_strength = 0.25;
        e.lens.enabled = !e.lens.enabled;
        e.sharpen.enabled = !e.sharpen.enabled;
        e.output_sharpen.enabled = !e.output_sharpen.enabled;
        e.vignette.amount = -0.4;
        e.grain.amount = 0.3;
        e.demosaic = Demosaic::Rcd;
        e.geometry.crop = Some(Crop {
            x: 0.1,
            y: 0.1,
            w: 0.6,
            h: 0.6,
        });
        e.geometry.aspect = Aspect::Original;
        e.geometry.portrait = true;
        e.geometry.turns = 1;
        e.geometry.flip = true;
        e.geometry.angle = 3.0;
        e.geometry.vertical = 5.0;
        e.geometry.horizontal = -2.0;
        e
    }

    /// Table: every section with a header reset, reset alone on an edit
    /// with them all moved, stands where a new edit has it and leaves
    /// every other section as it was; all of them reset together are the
    /// new edit, so no part of the edit a section shows is left out.
    /// And a mask's: each of its sections back to a new mask's, the
    /// global, the other mask, and the mask's own shapes and switches
    /// left alone.
    #[test]
    fn every_header_reset_puts_back_what_a_new_edit_has() {
        let original = Edit::default();
        let moved = moved();
        for name in SECTIONS {
            assert!(
                !section_at_reset(&moved, &original, None, name),
                "{name} starts moved"
            );
        }
        for name in SECTIONS {
            let mut e = moved.clone();
            assert!(reset_section(&mut e, &original, None, name), "{name}");
            for other in SECTIONS {
                assert_eq!(
                    section_at_reset(&e, &original, None, other),
                    other == name,
                    "{name} reset, then {other}"
                );
            }
        }
        let mut all = moved.clone();
        for name in SECTIONS {
            reset_section(&mut all, &original, None, name);
        }
        assert_eq!(all, original, "every section reset is a new edit");
        // Not a section here: no reset, nothing moved.
        for name in ["mask", "retouch", "proof", "monitor", "metadata"] {
            let mut e = moved.clone();
            assert!(!reset_section(&mut e, &original, None, name), "{name}");
            assert_eq!(e, moved);
        }

        // A mask's.
        let mut masked = moved.clone();
        for id in [1, 2] {
            masked.adjustments.push(Adjustment {
                id,
                name: format!("Mask {id}"),
                look: moved_look(),
                opacity: 0.6,
                ..Adjustment::default()
            });
        }
        for name in MASK_SECTIONS {
            assert!(
                !section_at_reset(&masked, &original, Some(0), name),
                "{name}"
            );
            let mut e = masked.clone();
            assert!(reset_section(&mut e, &original, Some(0), name), "{name}");
            for other in MASK_SECTIONS {
                assert_eq!(
                    section_at_reset(&e, &original, Some(0), other),
                    other == name,
                    "mask: {name} reset, then {other}"
                );
            }
            assert_eq!(e.adjustments[1], masked.adjustments[1], "{name}");
            e.adjustments.clear();
            let mut global = masked.clone();
            global.adjustments.clear();
            assert_eq!(e, global, "{name}: the global look is left alone");
        }
        let mut all = masked.clone();
        for name in MASK_SECTIONS {
            reset_section(&mut all, &original, Some(0), name);
        }
        let a = &all.adjustments[0];
        assert_eq!(a.look, Look::default());
        assert_eq!(a.look.white_balance, LocalWhite::Off);
        assert_eq!((a.id, a.opacity, a.enabled), (1, 0.6, true));
        // A Develop-only section has no reset on a mask; a mask that is
        // not there, none at all.
        let mut e = masked.clone();
        assert!(!reset_section(&mut e, &original, Some(0), "noise"));
        assert!(!reset_section(&mut e, &original, Some(5), "light"));
        assert_eq!(e, masked);
    }

    /// Each header in `app.slint` that draws a reset is wired to its own
    /// section: the instance under `if section == "x"` reads
    /// `section-resets.x` and sends its own name. Develop's sections and
    /// the Crop tab's are [`SECTIONS`], each once; the Masks tab's are
    /// [`MASK_SECTIONS`], each once more where it is an instance of its
    /// own (its white balance and its Detail) or shared with Develop.
    #[test]
    fn the_window_draws_a_reset_on_every_section_that_has_one() {
        let text = std::fs::read_to_string(
            std::path::Path::new(greycard_slint::PANEL_DIR)
                .parent()
                .unwrap()
                .join("app.slint"),
        )
        .unwrap();
        // (section, tab condition, wired field, sends its own name)
        let mut headers: Vec<(String, String, String, bool)> = Vec::new();
        let mut block: Option<(String, String)> = None;
        for line in text.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("if section == \"") {
                let name = rest.split('"').next().unwrap().to_string();
                block = Some((name, line.to_string()));
            } else if let Some(f) = line.strip_prefix("reset-live: root.section-resets.") {
                let (name, cond) = block.clone().expect("a reset inside a section's block");
                headers.push((name, cond, f.trim_end_matches(';').to_string(), false));
            } else if line == "reset-clicked => { root.section-reset(section); }" {
                headers.last_mut().expect("its reset-live first").3 = true;
            }
        }
        for (name, cond, wired, sends) in &headers {
            assert_eq!(name, wired, "{cond}");
            assert!(sends, "{cond}: sends its own name");
        }
        let names = |pick: &dyn Fn(&str) -> bool| -> Vec<String> {
            let mut v: Vec<String> = headers
                .iter()
                .filter(|(_, cond, ..)| pick(cond))
                .map(|(n, ..)| n.clone())
                .collect();
            v.sort_unstable();
            v
        };
        let sorted = |s: &[&str]| {
            let mut v: Vec<String> = s.iter().map(|n| n.to_string()).collect();
            v.sort_unstable();
            v
        };
        let develop = names(&|c| c.contains("\"Develop\"") || !c.contains("shown-tab"));
        assert_eq!(develop, sorted(SECTIONS), "Develop's and the Crop tab's");
        let masks = names(&|c| c.contains("\"Masks\""));
        assert_eq!(masks, sorted(MASK_SECTIONS), "a mask's");
    }

    /// The panel's edit made a state, as the save timer makes it.
    fn save(app: &App, state: &Rc<Shared>) {
        let mut st = state.borrow_mut();
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(&mut st, edit);
        crate::panel::history::show_history(&st, app);
    }

    fn current(state: &Rc<Shared>) -> Edit {
        let st = state.borrow();
        st.line(st.current.unwrap()).unwrap().current.clone()
    }

    fn steps(state: &Rc<Shared>) -> usize {
        let st = state.borrow();
        st.line(st.current.unwrap()).unwrap().history.len()
    }

    fn panel(app: &App, state: &Rc<Shared>) -> Edit {
        let st = state.borrow();
        read_edit(app, &st.develop.edit, st.develop.target)
    }

    fn live(app: &App, name: &str) -> bool {
        *field(&mut app.get_section_resets(), name).expect("a section with a reset")
    }

    /// A window over two frames, the first holding `edit` and open, its
    /// develop landed at 6000 by 4000, so a crop is fitted as it is on a
    /// real frame.
    fn open_with(edit: Edit) -> (App, Rc<Shared>) {
        open_files(edit, folder(2))
    }

    /// [`open_with`] over `files`.
    fn open_files(edit: Edit, files: Vec<std::path::PathBuf>) -> (App, Rc<Shared>) {
        let app = window(files.len());
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, files);
        state.borrow_mut().sidecars[0].record(edit);
        app.invoke_select(0);
        state.borrow_mut().develop.source_size = (6000, 4000);
        (app, state)
    }

    /// A click at the middle of the reset in `title`'s header.
    fn click_reset(app: &App, title: &str) {
        let (at, size) = header_reset(app, title);
        click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    }

    /// A click on `title`'s reset through the window puts back the
    /// section alone (`expect` is the edit it should leave), as one
    /// step, every fold and the order as they were, the icon hidden
    /// after; one undo brings the edit before back, the icon shown again.
    fn resets_through_the_window(
        app: &App,
        state: &Rc<Shared>,
        title: &str,
        name: &str,
        expect: &Edit,
    ) {
        let before = current(state);
        assert!(live(app, name), "{title}: lit before");
        let folds = || -> Vec<bool> {
            crate::panel::edit::FOLDS
                .iter()
                .map(|(_, get, _)| get(app))
                .collect()
        };
        let (folded, order) = (folds(), app.get_develop_order());
        let n = steps(state);
        let other = state.borrow().sidecars[1].clone();

        click_reset(app, title);
        assert_eq!(&current(state), expect, "{title}");
        assert_eq!(steps(state), n + 1, "{title}: one step");
        assert_eq!(&panel(app, state), expect, "{title}: the panel shows it");
        assert_eq!(folds(), folded, "{title}: nothing folded or opened");
        assert!(
            slint::Model::iter(&app.get_develop_order()).eq(slint::Model::iter(&order)),
            "{title}: nothing moved"
        );
        assert!(
            !live(app, name),
            "{title}: dimmed after: {:?}, {:?}",
            app.get_section_resets(),
            panel(app, state).geometry
        );
        assert_eq!(state.borrow().sidecars[1], other, "the other frame");

        app.invoke_undo();
        assert_eq!(current(state), before, "{title}: undone");
        assert_eq!(panel(app, state), before);
        assert!(live(app, name), "{title}: lit again");
    }

    #[test]
    fn light_resets_through_its_header() {
        let mut edit = Edit::default();
        edit.light.exposure = 1.25;
        edit.light.tone.contrast = 1.3;
        edit.light.enabled = false;
        edit.color.saturation = 0.4;
        edit.vignette.amount = -0.3;
        let (app, state) = open_with(edit.clone());
        let mut expect = edit.clone();
        expect.light = Default::default();
        resets_through_the_window(&app, &state, "LIGHT", "light", &expect);
    }

    #[test]
    fn white_balance_resets_to_as_shot_through_its_header() {
        // A tint the panel's f32 slider holds exactly.
        let mut edit = Edit {
            white_balance: WhiteBalance::Custom {
                temperature: 4300.0,
                tint: 0.0078125,
            },
            ..Edit::default()
        };
        edit.light.exposure = 0.5;
        let (app, state) = open_with(edit.clone());
        assert!(!app.get_as_shot());
        let mut expect = edit.clone();
        expect.white_balance = WhiteBalance::AsShot;
        resets_through_the_window(&app, &state, "WHITE BALANCE", "white-balance", &expect);
        click_reset(&app, "WHITE BALANCE");
        assert!(app.get_as_shot());
    }

    #[test]
    fn the_tone_curve_resets_through_its_header() {
        let mut edit = Edit::default();
        edit.curves.rgb = vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]];
        edit.curves.red = vec![[0.0, 0.05], [1.0, 1.0]];
        edit.curves.parametric.darks = 0.3;
        edit.curves.enabled = false;
        edit.mixer.hue[1] = 0.2;
        let (app, state) = open_with(edit.clone());
        let mut expect = edit.clone();
        expect.curves = Default::default();
        resets_through_the_window(&app, &state, "CURVES", "curves", &expect);
    }

    /// Color grading had a Reset button already; the header's does the
    /// whole section, its switch too, and the button stays.
    #[test]
    fn color_grading_resets_through_its_header_beside_its_button() {
        let mut edit = Edit::default();
        edit.grading.enabled = false;
        edit.grading.balance = 0.5;
        edit.grading.shadows.hue = 200.0;
        edit.grading.shadows.saturation = 0.4;
        edit.tint.amount = 0.2;
        let (app, state) = open_with(edit.clone());
        let mut expect = edit.clone();
        expect.grading = Default::default();
        resets_through_the_window(&app, &state, "COLOR GRADING", "grading", &expect);
        let buttons = crate::testing::buttons(&app, "Reset");
        let (header, _) = header_reset(&app, "COLOR GRADING");
        assert!(
            buttons.iter().any(|(at, _)| at.y > header.y + 20.0),
            "the section's own Reset is still in its body"
        );
    }

    /// The Crop tab's CROP: the crop, its aspect and its Portrait back,
    /// the straighten and the keystone left; then PERSPECTIVE alone.
    #[test]
    fn the_crop_resets_through_its_header() {
        let mut edit = Edit::default();
        edit.geometry.crop = Some(Crop {
            x: 0.2,
            y: 0.1,
            w: 0.4,
            h: 0.6,
        });
        edit.geometry.aspect = Aspect::Ratio { w: 1.0, h: 1.0 };
        edit.geometry.angle = 2.0;
        edit.geometry.vertical = 4.0;
        let (app, state) = open_with(edit.clone());
        app.set_panel_tab("Crop".into());
        app.invoke_tab_changed("Crop".into());
        let mut expect = edit.clone();
        expect.geometry.crop = None;
        expect.geometry.aspect = Aspect::Free;
        resets_through_the_window(&app, &state, "CROP", "crop", &expect);
        let mut expect = edit.clone();
        expect.geometry.vertical = 0.0;
        resets_through_the_window(&app, &state, "PERSPECTIVE", "perspective", &expect);
    }

    /// On the Masks tab the chosen mask's white balance goes to none of
    /// its own: off, no light kept, Follow global off. The other mask,
    /// the mask's other sections, and the global are left; then its
    /// Light alone.
    #[test]
    fn a_masks_white_balance_resets_to_none_of_its_own() {
        let mut edit = Edit {
            white_balance: WhiteBalance::Custom {
                temperature: 5000.0,
                tint: 0.0,
            },
            ..Edit::default()
        };
        for id in [1, 2] {
            edit.adjustments.push(Adjustment {
                id,
                name: format!("Lamp {id}"),
                look: moved_look(),
                ..Adjustment::default()
            });
        }
        edit.adjustments[0].look.white_balance = LocalWhite::Relative {
            mired: 20.0,
            tint: 0.0,
        };
        let (app, state) = open_with(edit.clone());
        app.set_panel_tab("Masks".into());
        app.invoke_tab_changed("Masks".into());
        app.invoke_target_changed(1);
        assert!(app.get_local_wb_on() && app.get_local_wb_relative());
        let mut expect = edit.clone();
        expect.adjustments[0].look.white_balance = LocalWhite::Off;
        resets_through_the_window(&app, &state, "WHITE BALANCE", "white-balance", &expect);
        click_reset(&app, "WHITE BALANCE");
        assert!(!app.get_local_wb_on() && !app.get_local_wb_kept() && !app.get_local_wb_relative());
        let mut expect = current(&state);
        expect.adjustments[0].look.light = Default::default();
        click_reset(&app, "LIGHT");
        assert_eq!(current(&state), expect);
        assert_eq!(current(&state).adjustments[1].look, moved_look());
        assert_eq!(current(&state).white_balance, edit.white_balance);
    }

    /// A press on the reset that turns into a drag moves no section and
    /// resets nothing; at a section's defaults there is none, and a
    /// reset arriving anyway does nothing; a slider moved and not yet
    /// saved brings it at once, and its reset records the move first.
    #[test]
    fn the_reset_neither_drags_nor_folds_its_section() {
        let mut edit = Edit::default();
        edit.light.exposure = 0.75;
        let (app, state) = open_with(edit.clone());
        let order = app.get_develop_order();
        let folded = app.get_collapsed_light();
        let n = steps(&state);
        let (at, size) = header_reset(&app, "LIGHT");
        let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
        let path: Vec<(f32, f32)> = (0..=12).map(|i| (x, y + 6.0 * i as f32)).collect();
        crate::testing::drag(&app, &path);
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        slint::platform::update_timers_and_animations();
        assert!(slint::Model::iter(&app.get_develop_order()).eq(slint::Model::iter(&order)));
        assert_eq!(app.get_collapsed_light(), folded);
        assert_eq!(current(&state), edit);
        assert_eq!(steps(&state), n);

        // At its defaults the color section's is not there to click.
        assert!(!live(&app, "color"));
        crate::testing::labeled(&app, "COLOR");
        assert!(
            !i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Reset")
                .any(|e| e.accessible_description().as_deref() == Some("COLOR")),
            "hidden at its defaults"
        );
        // One arriving all the same is nothing.
        app.invoke_section_reset("color".into());
        assert_eq!(steps(&state), n);
        assert_eq!(current(&state), edit);
        let folded = app.get_collapsed_color();

        app.set_saturation(0.5);
        app.invoke_view_changed();
        assert!(live(&app, "color"));
        click_reset(&app, "COLOR");
        assert_eq!(steps(&state), n + 2);
        assert_eq!(current(&state), edit);
        assert_eq!(app.get_collapsed_color(), folded);
        app.invoke_undo();
        assert_eq!(current(&state).color.saturation, 0.5);
        save(&app, &state);
    }

    /// Color's reset turns its switch back on.
    #[test]
    fn color_resets_its_switch_too() {
        let mut edit = Edit::default();
        edit.color.enabled = false;
        edit.color.vibrance = 0.2;
        let (app, state) = open_with(edit.clone());
        let mut expect = edit.clone();
        expect.color = Default::default();
        assert!(expect.color.enabled);
        resets_through_the_window(&app, &state, "COLOR", "color", &expect);
    }

    /// A section whose switch is dead does not reset, though a click
    /// reach it: the global mixer under black and white, and a
    /// monochrome frame's white balance.
    #[test]
    fn a_superseded_section_refuses_its_reset() {
        let mut edit = Edit::default();
        edit.bw.enabled = true;
        edit.mixer.hue[0] = 0.3;
        edit.white_balance = WhiteBalance::Custom {
            temperature: 4000.0,
            tint: 0.0,
        };
        let (app, state) = open_with(edit.clone());
        assert!(!live(&app, "mixer"));
        let n = steps(&state);
        app.invoke_section_reset("mixer".into());
        assert_eq!((current(&state), steps(&state)), (edit.clone(), n));

        assert!(live(&app, "white-balance"));
        {
            let mut st = state.borrow_mut();
            let path = st.path_of(0).clone();
            st.develop.monochromes.insert(path);
        }
        app.invoke_section_reset("white-balance".into());
        assert_eq!((current(&state), steps(&state)), (edit, n));
    }

    /// On a picture the Light section shows no display curve, and its
    /// reset leaves the curve as it is; on a raw it puts it back.
    #[test]
    fn light_leaves_a_pictures_display_curve() {
        let mut edit = Edit::for_picture();
        edit.display_curve = DisplayCurve::Agx;
        edit.light.exposure = 0.5;
        let files = vec![
            std::path::PathBuf::from("/nowhere/a.jpg"),
            std::path::PathBuf::from("/nowhere/b.jpg"),
        ];
        let (app, state) = open_files(edit.clone(), files);
        let mut expect = edit.clone();
        expect.light = Default::default();
        resets_through_the_window(&app, &state, "LIGHT", "light", &expect);

        let edit = Edit {
            display_curve: DisplayCurve::Agx,
            ..Edit::default()
        };
        let (app, state) = open_with(edit.clone());
        assert!(live(&app, "light"), "the curve alone is the section moved");
        resets_through_the_window(&app, &state, "LIGHT", "light", &Edit::default());
    }

    /// Rotate's reset on a crop that is not the whole plane, at a real
    /// frame's size: a straighten taken back refits the crop to the
    /// plane, as the slider would, and keeps it inside.
    #[test]
    fn rotate_refits_a_crop_its_straighten_took_back() {
        let edit = Edit {
            geometry: greycard_edit::geometry::Geometry {
                angle: 4.0,
                crop: Some(Crop {
                    x: 0.2,
                    y: 0.2,
                    w: 0.6,
                    h: 0.6,
                }),
                ..Default::default()
            },
            ..Edit::default()
        };
        let (app, state) = open_with(edit);
        crop_tab(&app);
        click_reset(&app, "ROTATE");
        let after = current(&state).geometry;
        assert_eq!(after.angle, 0.0);
        let crop = after.crop.expect("kept");
        assert!(after.fits(&crop, 6000.0, 4000.0));
        assert!(!live(&app, "rotate") && live(&app, "crop"));
    }

    fn crop_tab(app: &App) {
        app.set_panel_tab("Crop".into());
        app.invoke_tab_changed("Crop".into());
    }

    /// Two crops the same to f32's rounding: a turn taken back is
    /// `1 - (1 - x - w) - w`, which is `x` only to the last bit.
    fn near(a: Option<Crop>, b: Option<Crop>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => [(a.x, b.x), (a.y, b.y), (a.w, b.w), (a.h, b.h)]
                .iter()
                .all(|(p, q)| (p - q).abs() < 1e-6),
            (a, b) => a == b,
        }
    }

    // The review's tests, round one.

    #[test]
    fn review_rotate_reset_after_a_quarter_turn_keeps_the_crops_pixels() {
        let g = greycard_edit::geometry::Geometry {
            crop: Some(Crop {
                x: 0.05,
                y: 0.1,
                w: 0.5,
                h: 0.4,
            }),
            vertical: 6.0,
            ..Default::default()
        };
        let turned = g.turned(1);
        let edit = Edit {
            geometry: turned.clone(),
            ..Edit::default()
        };
        let (app, state) = open_with(edit.clone());
        crop_tab(&app);
        click_reset(&app, "ROTATE");
        let after = current(&state).geometry;
        assert_eq!(after.turns, 0);
        assert!(
            near(after.crop, g.crop),
            "the crop frames what it framed before the turn: {:?}",
            after.crop
        );
        assert_eq!(
            (after.vertical, after.horizontal),
            (g.vertical, g.horizontal),
            "the keystone on its axis"
        );
    }

    #[test]
    fn review_rotate_reset_after_a_flip_keeps_the_crops_pixels() {
        let g = greycard_edit::geometry::Geometry {
            crop: Some(Crop {
                x: 0.05,
                y: 0.1,
                w: 0.4,
                h: 0.4,
            }),
            ..Default::default()
        };
        let flipped = g.flipped(false);
        let edit = Edit {
            geometry: flipped.clone(),
            ..Edit::default()
        };
        let (app, state) = open_with(edit.clone());
        crop_tab(&app);
        click_reset(&app, "ROTATE");
        let after = current(&state).geometry;
        assert!(!after.flip);
        assert!(
            near(after.crop, g.crop),
            "the crop frames what it framed before the flip: {:?}",
            after.crop
        );
    }

    /// A turn and a flip in either order, then the reset: the crop and
    /// the keystone as they were before either.
    #[test]
    fn rotate_takes_back_a_turn_and_a_flip_in_either_order() {
        let g = greycard_edit::geometry::Geometry {
            crop: Some(Crop {
                x: 0.05,
                y: 0.1,
                w: 0.5,
                h: 0.4,
            }),
            vertical: 3.0,
            horizontal: -2.0,
            ..Default::default()
        };
        let original = Edit::default();
        for moved in [
            g.turned(1).flipped(false),
            g.flipped(false).turned(1),
            g.turned(3).flipped(true),
            g.flipped(true).turned(2),
        ] {
            let mut e = Edit {
                geometry: moved,
                ..Edit::default()
            };
            assert!(reset_section(&mut e, &original, None, "rotate"));
            let r = e.geometry;
            assert_eq!((r.turns, r.flip, r.angle), (0, false, 0.0));
            let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
            let (c, o) = (r.crop.unwrap(), g.crop.unwrap());
            assert!(
                close(c.x, o.x) && close(c.y, o.y) && close(c.w, o.w) && close(c.h, o.h),
                "{c:?}"
            );
            assert_eq!((r.vertical, r.horizontal), (g.vertical, g.horizontal));
        }
    }

    #[test]
    fn review_dimming_follows_a_frame_switch_and_undo_redo() {
        let mut e1 = Edit::default();
        e1.light.exposure = 1.0;
        let (app, state) = open_with(Edit::default());
        state.borrow_mut().sidecars[1].record(e1.clone());
        assert!(!live(&app, "light"), "frame 0 at defaults");
        app.invoke_select(1);
        crate::testing::settle(&app);
        assert_eq!(state.borrow().current, Some(1));
        assert!(
            live(&app, "light"),
            "frame 1 moved: {:?}",
            app.get_section_resets()
        );
        app.invoke_select(0);
        crate::testing::settle(&app);
        assert!(!live(&app, "light"), "back on frame 0");
        app.invoke_select(1);
        crate::testing::settle(&app);
        click_reset(&app, "LIGHT");
        assert!(!live(&app, "light"));
        app.invoke_undo();
        assert!(live(&app, "light"));
        app.invoke_redo();
        assert!(!live(&app, "light"), "after redo");
    }

    #[test]
    fn review_dimming_follows_a_mask_switch() {
        let mut edit = Edit::default();
        for id in [1, 2] {
            edit.adjustments.push(Adjustment {
                id,
                name: format!("M{id}"),
                ..Adjustment::default()
            });
        }
        edit.adjustments[0].look.light.exposure = 0.7;
        edit.color.saturation = 0.3;
        let (app, _state) = open_with(edit);
        app.set_panel_tab("Masks".into());
        app.invoke_tab_changed("Masks".into());
        app.invoke_target_changed(1);
        assert!(live(&app, "light"));
        assert!(!live(&app, "color"), "the mask's color is a new mask's");
        app.invoke_target_changed(2);
        assert!(
            !live(&app, "light"),
            "mask 2: {:?}",
            app.get_section_resets()
        );
        app.invoke_target_changed(1);
        assert!(live(&app, "light"));
        app.set_panel_tab("Develop".into());
        app.invoke_tab_changed("Develop".into());
        assert!(!live(&app, "light"));
        assert!(live(&app, "color"), "the global's color");
    }

    #[test]
    fn review_a_reset_records_no_spurious_step_for_a_wb_the_slider_rounds() {
        let mut edit = Edit {
            white_balance: WhiteBalance::Custom {
                temperature: 4321.123,
                tint: 0.0123,
            },
            ..Edit::default()
        };
        edit.light.exposure = 0.5;
        let (app, state) = open_with(edit.clone());
        let n = steps(&state);
        click_reset(&app, "LIGHT");
        assert_eq!(steps(&state), n + 1, "one step");
        assert_eq!(
            current(&state).white_balance,
            edit.white_balance,
            "WB untouched"
        );
        // Nor written later, by a save still to come.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
        slint::platform::update_timers_and_animations();
        assert_eq!(steps(&state), n + 1);
        assert_eq!(current(&state).white_balance, edit.white_balance);
    }

    /// The reset's empty room is the header's whether a reset was ever
    /// shown there or one just went: a click there folds the section,
    /// on a frame whose Light was never moved and right after Light's
    /// reset alike, and folds it back.
    #[test]
    fn the_empty_room_folds_as_the_header_either_way() {
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        let (app, state) = open_with(edit);
        app.set_collapsed_light(false);
        // The switch, found whether or not the reset is there, and the
        // reset's room a fixed way left of it.
        let switch = |app: &App| {
            let (at, size) = crate::testing::labeled_where(app, "LIGHT", |e| {
                e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Switch)
            });
            (at.x, at.y + size.height / 2.0)
        };
        let (at, size) = header_reset(&app, "LIGHT");
        let left = switch(&app).0 - (at.x + size.width / 2.0);
        let room = |app: &App| {
            let (x, y) = switch(app);
            (x - left, y)
        };
        let later = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
            slint::platform::update_timers_and_animations();
        };
        // Found afresh before each click: a reset or a fold can change
        // the panel's height, and with the panel scrolled its scroll is
        // then held to the new end, which carries the header from under
        // a pointer that stays where it was. That, not the reset's
        // touch area, is what a click at the old place missed.
        let (x, y) = room(&app);
        click(&app, x, y);
        assert_eq!(current(&state).light, Default::default());
        assert!(!live(&app, "light"));
        assert!(!app.get_collapsed_light(), "the reset folds nothing");
        later();
        let (x, y) = room(&app);
        click(&app, x, y);
        later();
        assert!(
            app.get_collapsed_light(),
            "the empty room folds, just after"
        );
        let (x, y) = room(&app);
        click(&app, x, y);
        later();
        assert!(!app.get_collapsed_light(), "and opens");
        assert_eq!(current(&state).light, Default::default());

        // A window where Light's reset was never shown: the same room,
        // the same fold.
        let (app, state) = open_with(Edit::default());
        app.set_collapsed_light(false);
        assert!(!live(&app, "light"));
        let (x, y) = room(&app);
        click(&app, x, y);
        later();
        assert!(app.get_collapsed_light(), "the empty room folds, never lit");
        assert_eq!(steps(&state), 0);
    }

    /// A scratch folder for a test that writes sidecars.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("greycard-{name}-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        dir
    }

    // The review's tests, round two.

    /// Every combination: turns 0-3 and flip of the Original, a crop or
    /// none, every aspect, Portrait both ways, a keystone on both axes
    /// or none, a straighten, and every sequence of up to three moves;
    /// Crop's and Perspective's resets leave the turn, and each other's
    /// part, alone.
    #[test]
    fn review_rotate_property() {
        use greycard_edit::geometry::Geometry;
        #[derive(Clone, Copy, Debug)]
        enum M {
            T(i32),
            F(bool),
        }
        let moves = [M::T(1), M::T(2), M::T(3), M::F(false), M::F(true)];
        let mut seqs: Vec<Vec<M>> = vec![vec![]];
        for _ in 0..3 {
            let mut next = seqs.clone();
            for s in &seqs {
                if s.len() < 3 {
                    for m in moves {
                        let mut t = s.clone();
                        t.push(m);
                        next.push(t);
                    }
                }
            }
            seqs = next;
        }
        seqs.sort_by_key(|s| format!("{s:?}"));
        seqs.dedup_by_key(|s| format!("{s:?}"));
        let crops = [
            Some(Crop {
                x: 0.05,
                y: 0.1,
                w: 0.5,
                h: 0.4,
            }),
            None,
        ];
        let aspects = [
            Aspect::Free,
            Aspect::Original,
            Aspect::Ratio { w: 3.0, h: 2.0 },
        ];
        let (mut n, mut bad) = (0, Vec::new());
        for crop in crops {
            for aspect in aspects {
                for portrait in [false, true] {
                    for (v, h) in [(0.0f32, 0.0f32), (4.0, -3.0)] {
                        for angle in [0.0f32, 3.5] {
                            for ot in 0..4u8 {
                                for of in [false, true] {
                                    let mut o = Edit::default();
                                    o.geometry.turns = ot;
                                    o.geometry.flip = of;
                                    let base = Geometry {
                                        turns: ot,
                                        flip: of,
                                        angle: 0.0,
                                        vertical: v,
                                        horizontal: h,
                                        crop,
                                        aspect,
                                        portrait,
                                    };
                                    for s in &seqs {
                                        let mut g = base.clone();
                                        g.angle = angle;
                                        for m in s {
                                            g = match *m {
                                                M::T(k) => g.turned(k),
                                                M::F(vv) => g.flipped(vv),
                                            };
                                        }
                                        let moved = Edit {
                                            geometry: g.clone(),
                                            ..Edit::default()
                                        };
                                        let mut e = moved.clone();
                                        reset_section(&mut e, &o, None, "rotate");
                                        let r = &e.geometry;
                                        n += 1;
                                        let ok = r.turns == base.turns
                                            && r.flip == base.flip
                                            && r.angle == base.angle
                                            && (r.vertical - base.vertical).abs() < 1e-6
                                            && (r.horizontal - base.horizontal).abs() < 1e-6
                                            && near(r.crop, base.crop)
                                            && r.aspect == base.aspect
                                            && r.portrait == base.portrait;
                                        if !ok {
                                            bad.push(format!("{s:?} from {base:?} gave {r:?}"));
                                        }
                                        for other in ["crop", "perspective"] {
                                            let mut e2 = moved.clone();
                                            reset_section(&mut e2, &o, None, other);
                                            let r2 = &e2.geometry;
                                            if (r2.turns, r2.flip, r2.angle)
                                                != (g.turns, g.flip, g.angle)
                                            {
                                                bad.push(format!("{other} moved the turn"));
                                            }
                                            if other == "perspective" && r2.crop != g.crop {
                                                bad.push("perspective moved the crop".into());
                                            }
                                            if other == "crop"
                                                && (r2.vertical, r2.horizontal)
                                                    != (g.vertical, g.horizontal)
                                            {
                                                bad.push("crop moved the keystone".into());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(
            bad.is_empty(),
            "{} of {n} bad, first: {:?}",
            bad.len(),
            &bad[..bad.len().min(5)]
        );
    }

    /// The fast second click of a double-click on the reset folds
    /// nothing.
    #[test]
    fn review_double_click_on_reset_does_not_fold() {
        let mut edit = Edit::default();
        edit.light.exposure = 0.75;
        let (app, state) = open_with(edit);
        let folded = app.get_collapsed_light();
        let (at, size) = header_reset(&app, "LIGHT");
        let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
        click(&app, x, y);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(60));
        slint::platform::update_timers_and_animations();
        click(&app, x, y);
        slint::platform::update_timers_and_animations();
        assert_eq!(current(&state).light, Default::default(), "reset");
        assert_eq!(
            app.get_collapsed_light(),
            folded,
            "the second click folded LIGHT"
        );
    }

    /// A slider of another section moved and not yet saved, then a
    /// reset: two steps, the move kept, the save's timer stopped, the
    /// disk the history's top, and nothing recorded later.
    #[test]
    fn review_pending_then_reset_on_disk() {
        let dir = scratch("pending-reset");
        let files = vec![dir.join("IMG_0001.CR3"), dir.join("IMG_0002.CR3")];
        for f in &files {
            std::fs::write(f, b"not a raw").unwrap();
        }
        let app = window(2);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;
        let mut edit = Edit {
            white_balance: WhiteBalance::Custom {
                temperature: 4_321.123_456_789,
                tint: 0.012_345_678_9,
            },
            ..Edit::default()
        };
        edit.light.exposure = 0.7;
        edit.grading.balance = 0.123_456_7;
        state.borrow_mut().sidecars[0].record(edit.clone());
        app.invoke_select(0);
        state.borrow_mut().develop.source_size = (6000, 4000);
        let n = steps(&state);
        app.set_saturation(0.5);
        app.invoke_view_changed();
        assert!(state.borrow().develop.save_timer.running());
        click_reset(&app, "LIGHT");
        assert!(
            !state.borrow().develop.save_timer.running(),
            "the save's timer stopped"
        );
        assert_eq!(steps(&state), n + 2);
        let got = current(&state);
        assert_eq!(got.color.saturation, 0.5);
        assert_eq!(got.light, greycard_edit::Light::default());
        let saved = greycard_edit::Sidecar::load(&files[0])
            .unwrap()
            .expect("written");
        assert_eq!(saved.current, got, "the disk is the history's top");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
        slint::platform::update_timers_and_animations();
        assert_eq!(steps(&state), n + 2, "nothing recorded later");
        app.invoke_undo();
        assert_eq!(current(&state).color.saturation, 0.5);
        assert_eq!(current(&state).light, edit.light);
        drop(app);
        crate::testing::remove_scratch(state, &dir);
    }

    /// No move waiting: one step, every other field bit for bit, the
    /// disk the history's top.
    #[test]
    fn review_no_pending_one_step_bit_for_bit_on_disk() {
        let dir = scratch("no-pending-reset");
        let files = vec![dir.join("IMG_0001.CR3"), dir.join("IMG_0002.CR3")];
        for f in &files {
            std::fs::write(f, b"not a raw").unwrap();
        }
        let app = window(2);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;
        let mut edit = moved();
        edit.white_balance = WhiteBalance::Custom {
            temperature: 4_321.123_456_789,
            tint: 0.012_345_678_9,
        };
        edit.geometry = Default::default();
        // `moved` turns the look table's switch on over the file's own
        // profile, which a sidecar never writes (no table to switch);
        // here the edit has to be one a sidecar holds.
        edit.camera = Default::default();
        state.borrow_mut().sidecars[0].record(edit.clone());
        app.invoke_select(0);
        state.borrow_mut().develop.source_size = (6000, 4000);
        let n = steps(&state);
        click_reset(&app, "COLOR");
        assert_eq!(steps(&state), n + 1);
        let mut expect = edit.clone();
        expect.color = Default::default();
        assert_eq!(current(&state), expect);
        let saved = greycard_edit::Sidecar::load(&files[0])
            .unwrap()
            .expect("written");
        assert_eq!(saved.current, expect);
        drop(app);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The headers' resets drawn, for a look by eye: the Develop panel,
    /// a mask's sections, and the Crop tab, in Charcoal and Light, the
    /// icon at rest (lit where its section moved, dimmed where not) and
    /// under the pointer with its tip. Written to the folder
    /// `GREYCARD_RESET_CAPTURES` names, and only then.
    #[test]
    fn capture_the_header_resets() {
        let Some(dir) = std::env::var_os("GREYCARD_RESET_CAPTURES") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::testing::drawing_backend();
        let mut edit = Edit::default();
        edit.light.exposure = 0.6;
        edit.curves.parametric.darks = 0.2;
        edit.geometry.crop = Some(Crop {
            x: 0.1,
            y: 0.1,
            w: 0.7,
            h: 0.7,
        });
        edit.adjustments.push(Adjustment {
            id: 1,
            name: "Une lampe au long nom".into(),
            look: moved_look(),
            ..Adjustment::default()
        });
        let (app, _state) = open_with(edit);
        app.set_collapsed_light(true);
        app.set_collapsed_wb(true);
        app.set_collapsed_color(true);
        app.set_collapsed_curves(true);
        app.set_collapsed_look(true);
        app.set_collapsed_camera(true);
        let shot = |name: &str| {
            // Not `testing::settle`, whose pointer move would end a hover.
            for _ in 0..5 {
                i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
                slint::platform::update_timers_and_animations();
            }
            let s = app.window().take_snapshot().expect("a drawing backend");
            let img =
                image::RgbaImage::from_raw(s.width(), s.height(), s.as_bytes().to_vec()).unwrap();
            let view =
                i_slint_backend_testing::ElementHandle::find_by_element_id(&app, "App::panel-view")
                    .next()
                    .expect("the panel");
            let x = (view.absolute_position().x as u32).saturating_sub(8);
            let panel = image::imageops::crop_imm(&img, x, 0, img.width() - x, img.height());
            panel
                .to_image()
                .save(dir.join(format!("{name}.png")))
                .unwrap();
        };
        let hover = |title: &str| {
            let (at, size) = header_reset(&app, title);
            app.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(
                    at.x + size.width / 2.0,
                    at.y + size.height / 2.0,
                ),
            });
            slint::platform::update_timers_and_animations();
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(800));
            slint::platform::update_timers_and_animations();
        };
        let away = || {
            app.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(5.0, 300.0),
            });
            app.set_panel_scroll(0.0);
        };
        for theme in ["Charcoal", "Light"] {
            app.global::<crate::Theme>().set_name(theme.into());
            let t = theme.to_lowercase();
            app.set_panel_tab("Develop".into());
            app.invoke_tab_changed("Develop".into());
            away();
            shot(&format!("develop-{t}-rest"));
            hover("LIGHT");
            shot(&format!("develop-{t}-hover"));
            away();

            app.set_panel_tab("Masks".into());
            app.invoke_tab_changed("Masks".into());
            app.invoke_target_changed(1);
            away();
            shot(&format!("masks-{t}-rest"));
            hover("LIGHT");
            shot(&format!("masks-{t}-hover"));

            app.set_panel_tab("Crop".into());
            app.invoke_tab_changed("Crop".into());
            away();
            shot(&format!("crop-{t}-rest"));
            hover("CROP");
            shot(&format!("crop-{t}-hover"));
            away();
        }

        // The narrowest panel, in French, whose titles run longest.
        app.global::<crate::Theme>().set_name("Charcoal".into());
        slint::select_bundled_translation("fr").expect("French is bundled");
        app.set_right_size(280.0);
        app.set_panel_tab("Develop".into());
        app.invoke_tab_changed("Develop".into());
        away();
        shot("fr-280-develop");
        app.set_panel_scroll(-400.0);
        shot("fr-280-develop-lower");
        app.set_panel_tab("Masks".into());
        app.invoke_tab_changed("Masks".into());
        app.invoke_target_changed(1);
        app.set_collapsed_mask(true);
        away();
        app.set_panel_scroll(-500.0);
        shot("fr-280-masks");
        slint::select_bundled_translation("").unwrap();
    }
}
