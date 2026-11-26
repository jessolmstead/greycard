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
    "grain",
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
            "grain" => look.grain = fresh.grain,
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
        "demosaic" => {
            edit.demosaic = original.demosaic;
            edit.highlight_fill = original.highlight_fill.clone();
        }
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
            "grain" => look.grain == fresh.grain,
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
        "demosaic" => {
            edit.demosaic == original.demosaic && edit.highlight_fill == original.highlight_fill
        }
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
    let Some(edit) = st.line(c).map(|l| l.current().clone()) else {
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
    if !st.line_mut(c).is_some_and(|mut line| line.record(reset)) {
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
mod tests;
