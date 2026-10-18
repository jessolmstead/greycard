//! The Landscape button. Pressed, the sky's model runs once on the
//! picture's preview (or not at all, when a Sky or the button already
//! ran it on this develop), and a menu opens of only what it found:
//! the sky when the sky's gate passes, and the water, foliage,
//! mountains and buildings whose own gates pass, the largest first.
//! Picking one makes that shape as the Sky button makes a sky. When
//! nothing passes, nothing opens and the status line says so.

use greycard_ai::landscape::{Kind, Offer};
use greycard_edit::mask::LandscapeGroup;

use crate::panel::assets::offer_model;
use crate::panel::edit::{read_edit, show_edit};
use crate::panel::mask::{Step, name_first_shape, step};
use crate::*;

/// The mask the button was pressed on, by id, and the mode a shape
/// picked from its menu joins by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pending {
    pub(crate) id: u64,
    pub(crate) mode: Mode,
}

/// The shape an offer makes, by the name `Shape::of_kind` takes.
pub(crate) fn kind_of(offer: &Offer) -> &'static str {
    match offer.kind {
        Kind::Sky => "Sky",
        Kind::Group(g) => LandscapeGroup::from(g.name().to_string()).label(),
    }
}

/// An offer on the menu: its name and the share of the frame it is.
pub(crate) fn words_of(offer: &Offer) -> String {
    let percent = (offer.area * 100.0).round().max(1.0);
    format!("{} {percent:.0}%", kind_of(offer))
}

/// The sky's model's step: ask, offer it, or wait. A Landscape shape
/// wants the prior alone.
fn model_step(st: &State) -> Option<Step> {
    let shape = Shape::of_kind("Water");
    let store = st.develop.store.clone();
    let have = |m: &greycard_ai::Model| store.as_ref().is_some_and(|s| s.have(m));
    let sheet_free = st.sheets.fetch.is_none() && !st.sheets.fetching;
    step(
        &shape,
        have,
        crate::ai::providers(),
        &st.sheets.declined,
        &st.sheets.fetch_failed,
        sheet_free,
        |_| false,
    )
}

/// The button, on the chosen mask, the shape it makes to join by
/// `mode`: what the model finds is asked for, the model offered first
/// if it is not downloaded.
pub(crate) fn pressed(st: &mut State, app: &App, mode: &str) {
    let Some(a) = st
        .develop
        .target
        .and_then(|i| st.develop.edit.adjustments.get(i))
    else {
        return;
    };
    // A mask's first shape is always Add, as the other buttons have it.
    let mode = if a.mask.components.is_empty() {
        Mode::Add
    } else {
        Mode::from_name(mode).unwrap_or_default()
    };
    let id = a.id;
    // Whatever else was in hand is put down.
    if st.develop.placing.take().is_some() || st.develop.choosing.take().is_some() {
        app.set_placing("".into());
    }
    st.develop.part_pending = None;
    st.develop.landscape_offer = None;
    st.develop.landscape_pending = Some(Pending { id, mode });
    resume(st, app);
}

/// A press waiting goes on as the model's state allows: asked for with
/// the model in the store; its model offered when the sheet is free;
/// let go, and said why, when it was declined or could not be fetched
/// this session; else it waits for the download or the sheet in hand,
/// and says so. Again whenever a download ends.
pub(crate) fn resume(st: &mut State, app: &App) {
    if st.develop.landscape_pending.is_none() {
        return;
    }
    match model_step(st) {
        // The model is here and the runtime can be fetched: offered
        // first, and this goes on again once it lands.
        Some(Step::Ask(_))
            if crate::panel::assets::runtime_first(
                st,
                app,
                crate::panel::assets::AfterRuntime::Nothing,
            ) => {}
        Some(Step::Ask(_)) => ask(app),
        Some(Step::Offer(model, falls_back)) => offer_model(st, app, model, falls_back),
        _ if st.sheets.declined.contains(&greycard_ai::SKY.id) => {
            st.develop.landscape_pending = None;
            app.set_status(DECLINED.into());
        }
        _ if st.sheets.fetch_failed.contains(&greycard_ai::SKY.id) => {
            st.develop.landscape_pending = None;
            app.set_status("the Sky model could not be fetched this session".into());
        }
        _ => app.set_status(WAITING.into()),
    }
}

/// The status line for a press that waits on a download or a sheet.
pub(crate) const WAITING: &str = "the landscape is found once the download in hand is done";
/// And for one whose model was declined.
const DECLINED: &str = "without the Sky model no landscape can be found";

fn ask(app: &App) {
    WORKER.with(|w| {
        if let Some(w) = &*w.borrow() {
            w.send(Job::Landscape);
        }
    });
    app.set_status("finding the landscape".into());
}

/// The sky's model arrived (or its offer was answered): a press
/// waiting on it goes on, or is let go.
pub(crate) fn model_answered(st: &mut State, app: &App, have: bool) {
    if st.develop.landscape_pending.is_none() {
        return;
    }
    if have {
        ask(app);
    } else {
        st.develop.landscape_pending = None;
        app.set_status(DECLINED.into());
    }
}

/// What the model found, for the press waiting on it: its menu opens
/// over the button, or, with nothing found, the status line says so.
/// An answer for a picture no longer open, or with no press waiting,
/// is let go.
pub(crate) fn found(
    st: &mut State,
    app: &App,
    file: Option<PathBuf>,
    offers: Result<(Vec<Offer>, &'static str, f64), String>,
) {
    let Some(pending) = st.develop.landscape_pending.take() else {
        return;
    };
    let open = st.current.and_then(|c| st.path_at(c)).cloned();
    if file.is_some() && file != open {
        return;
    }
    let (offers, provider, seconds) = match offers {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("Landscape: {e}");
            app.set_status(format!("Landscape: {e}").into());
            return;
        }
    };
    let words: Vec<slint::SharedString> = offers.iter().map(|o| words_of(o).into()).collect();
    let kinds: Vec<slint::SharedString> = offers.iter().map(|o| kind_of(o).into()).collect();
    app.set_landscape_offers(ModelRc::new(VecModel::from(words)));
    app.set_landscape_kinds(ModelRc::new(VecModel::from(kinds)));
    if offers.is_empty() {
        let note = crate::ai::none_found_note("landscape", open.as_deref());
        app.set_status(note.into());
        return;
    }
    st.develop.landscape_offer = Some(pending);
    app.set_landscape_menu_shown(true);
    let n = offers.len();
    app.set_status(
        format!(
            "{n} {} found on {provider} in {seconds:.2} s",
            if n == 1 { "shape" } else { "shapes" }
        )
        .into(),
    );
}

/// A shape picked from the menu, by kind, into the mask the button was
/// pressed on; whether the edit changed.
pub(crate) fn chosen(st: &mut State, app: &App, kind: &str) -> bool {
    let Some(pending) = st.develop.landscape_offer.take() else {
        return false;
    };
    let edit = read_edit(app, &st.develop.edit, st.develop.target);
    st.develop.edit = edit;
    let Some((index, a)) = st
        .develop
        .edit
        .adjustments
        .iter_mut()
        .enumerate()
        .find(|(_, a)| a.id == pending.id)
    else {
        return false;
    };
    let mode = if a.mask.components.is_empty() {
        Mode::Add
    } else {
        pending.mode
    };
    a.mask.components.push(Component {
        shape: Shape::of_kind(kind),
        mode,
        invert: false,
        enabled: true,
    });
    name_first_shape(a, kind);
    let which = a.mask.components.len() - 1;
    st.develop.fresh_mask = None;
    st.develop.target = Some(index);
    app.set_component(which as i32);
    show_edit(st, &st.develop.edit, app, st.develop.target);
    app.set_status(format!("finding the {}", kind.to_lowercase()).into());
    true
}

/// Escape, another mask, another tab or another picture: the press
/// waiting and the menu's offer go.
pub(crate) fn stopped(st: &mut State) {
    st.develop.landscape_pending = None;
    st.develop.landscape_offer = None;
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_find_landscape(move |mode| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            pressed(&mut state.borrow_mut(), &app, mode.as_str());
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_add_landscape(move |kind| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let changed = chosen(&mut state.borrow_mut(), &app, kind.as_str());
            if changed {
                app.invoke_view_changed();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{folder, state_for, window};
    use greycard_ai::landscape::Group;

    /// A window on the Masks tab with one empty mask chosen.
    fn masks() -> (App, Rc<RefCell<State>>, Rc<Worker>) {
        let app = window(1);
        let (state, worker) = state_for(&app, folder(1));
        app.set_panel_tab("Masks".into());
        {
            let mut st = state.borrow_mut();
            st.develop.edit.adjustments.push(greycard_edit::Adjustment {
                id: 1,
                name: "Mask 1".into(),
                enabled: true,
                mask: Mask::default(),
                look: Default::default(),
            });
            st.develop.target = Some(0);
        }
        (app, state, worker)
    }

    fn shapes(st: &State) -> Vec<Shape> {
        st.develop.edit.adjustments[0]
            .mask
            .components
            .iter()
            .map(|c| c.shape.clone())
            .collect()
    }

    fn coast() -> Vec<Offer> {
        vec![
            Offer {
                kind: Kind::Group(Group::Foliage),
                area: 0.33,
            },
            Offer {
                kind: Kind::Sky,
                area: 0.25,
            },
            Offer {
                kind: Kind::Group(Group::Water),
                area: 0.166,
            },
        ]
    }

    fn strings(m: ModelRc<slint::SharedString>) -> Vec<String> {
        (0..m.row_count())
            .map(|i| m.row_data(i).unwrap().to_string())
            .collect()
    }

    #[test]
    fn an_offer_reads_as_its_kind_and_its_share() {
        let words: Vec<String> = coast().iter().map(words_of).collect();
        assert_eq!(words, ["Foliage 33%", "Sky 25%", "Water 17%"]);
        let tiny = Offer {
            kind: Kind::Group(Group::Building),
            area: 0.003,
        };
        assert_eq!(words_of(&tiny), "Building 1%");
        for g in Group::ALL {
            let kind = kind_of(&Offer {
                kind: Kind::Group(g),
                area: 0.1,
            });
            assert!(
                matches!(Shape::of_kind(kind), Shape::Landscape { .. }),
                "{kind}"
            );
        }
    }

    /// What was found goes on the menu, in the order found, and the
    /// menu opens; a pick makes that shape in the mask the button was
    /// pressed on, named for it as a first shape is, and asks for its
    /// raster as the Sky button's does.
    #[test]
    fn what_is_found_is_offered_and_a_pick_makes_it() {
        let (app, state, _worker) = masks();
        let mut st = state.borrow_mut();
        st.develop.landscape_pending = Some(Pending {
            id: 1,
            mode: Mode::Subtract,
        });
        found(&mut st, &app, None, Ok((coast(), "WebGPU", 0.42)));
        assert!(st.develop.landscape_pending.is_none());
        assert_eq!(
            strings(app.get_landscape_offers()),
            ["Foliage 33%", "Sky 25%", "Water 17%"]
        );
        assert_eq!(
            strings(app.get_landscape_kinds()),
            ["Foliage", "Sky", "Water"]
        );
        assert!(app.get_landscape_menu_shown());
        assert_eq!(app.get_status(), "3 shapes found on WebGPU in 0.42 s");
        assert!(chosen(&mut st, &app, "Water"));
        // The mask's first shape is Add, whatever the row said.
        let c = &st.develop.edit.adjustments[0].mask.components[0];
        assert_eq!(c.shape, Shape::of_kind("Water"));
        assert_eq!(c.mode, Mode::Add);
        assert_eq!(st.develop.edit.adjustments[0].name, "Water 1");
        assert_eq!(app.get_status(), "finding the water");
        // The menu's offer is spent: a second pick from a stale menu
        // makes nothing.
        assert!(!chosen(&mut st, &app, "Foliage"));
        assert_eq!(shapes(&st).len(), 1);
        // Pressed again, the sky from it is a sky, joined by the mode
        // the button was pressed with.
        st.develop.landscape_pending = Some(Pending {
            id: 1,
            mode: Mode::Subtract,
        });
        found(&mut st, &app, None, Ok((coast(), "WebGPU", 0.01)));
        assert!(chosen(&mut st, &app, "Sky"));
        let c = &st.develop.edit.adjustments[0].mask.components[1];
        assert_eq!(c.shape, Shape::Sky { picks: Vec::new() });
        assert_eq!(c.mode, Mode::Subtract);
    }

    #[test]
    fn nothing_found_says_so_and_opens_nothing() {
        let (app, state, _worker) = masks();
        let mut st = state.borrow_mut();
        st.develop.landscape_pending = Some(Pending {
            id: 1,
            mode: Mode::Add,
        });
        found(&mut st, &app, None, Ok((Vec::new(), "CPU", 0.3)));
        assert!(!app.get_landscape_menu_shown());
        assert!(st.develop.landscape_offer.is_none());
        assert!(
            app.get_status().starts_with("no landscape found in "),
            "{}",
            app.get_status()
        );
        assert!(shapes(&st).is_empty());
    }

    /// An answer nobody waits on, or for a picture no longer open, is
    /// let go: no menu over another picture.
    #[test]
    fn an_answer_for_another_picture_or_no_press_is_let_go() {
        let (app, state, _worker) = masks();
        let mut st = state.borrow_mut();
        found(&mut st, &app, None, Ok((coast(), "CPU", 0.3)));
        assert!(!app.get_landscape_menu_shown() && st.develop.landscape_offer.is_none());
        st.develop.landscape_pending = Some(Pending {
            id: 1,
            mode: Mode::Add,
        });
        found(
            &mut st,
            &app,
            Some(PathBuf::from("/elsewhere/another.CR3")),
            Ok((coast(), "CPU", 0.3)),
        );
        assert!(!app.get_landscape_menu_shown() && st.develop.landscape_offer.is_none());
        // Escape puts a press down with the rest.
        st.develop.landscape_pending = Some(Pending {
            id: 1,
            mode: Mode::Add,
        });
        crate::panel::parts::stopped(&mut st);
        assert!(st.develop.landscape_pending.is_none());
    }

    /// Delivered as the worker delivers it: the menu's model and its
    /// raise, through the deliver path.
    #[test]
    fn the_worker_s_answer_reaches_the_menu() {
        let (app, state, _worker) = masks();
        state.borrow_mut().develop.landscape_pending = Some(Pending {
            id: 1,
            mode: Mode::Add,
        });
        crate::panel::deliver::deliver(
            &app,
            Outcome::Landscape {
                file: None,
                offers: Ok((coast(), "CPU", 0.2)),
            },
        );
        assert_eq!(app.get_landscape_offers().row_count(), 3);
        assert!(state.borrow().develop.landscape_offer.is_some());
    }

    /// The button sits beside Sky and its menu is what was found, each
    /// entry making its kind.
    #[test]
    fn the_button_is_beside_sky_and_its_menu_is_what_was_found() {
        let slint = greycard_slint::MASK_SLINT;
        let (before, button) = slint
            .split_once("landscape-button := PanelButton {")
            .expect("the button");
        let row = before.rsplit("HorizontalLayout {").next().unwrap();
        assert!(row.contains("root.add-shape(\"Sky\""), "{row}");
        assert!(button.contains("text: \"Landscape\";"));
        assert!(button.contains("mountain.svg"));
        assert!(button.contains("root.find-landscape(root.shape-mode)"));
        let menu = button.split("Menu {").nth(1).unwrap();
        assert!(menu.contains("for offer[i] in root.landscape-offers: MenuItem"));
        assert!(menu.contains("root.add-landscape(root.landscape-kinds[i])"));
    }

    /// Pressed with no model in the store and none to offer this
    /// session: said, and nothing waits.
    #[test]
    fn a_press_without_the_model_says_so() {
        let (app, state, _worker) = masks();
        let mut st = state.borrow_mut();
        st.develop.store = None;
        st.sheets.declined.push(greycard_ai::SKY.id);
        pressed(&mut st, &app, "Add");
        assert!(st.develop.landscape_pending.is_none());
        assert_eq!(app.get_status(), DECLINED);
    }

    /// Pressed while a download runs: the press waits and says so, and
    /// when the download ends it goes on as the model's state allows,
    /// here let go, the Sky model's fetch having failed.
    #[test]
    fn a_press_during_a_download_waits_for_it() {
        let (app, state, _worker) = masks();
        let mut st = state.borrow_mut();
        st.develop.store = None;
        st.sheets.fetching = true;
        pressed(&mut st, &app, "Add");
        assert!(st.develop.landscape_pending.is_some(), "the press waits");
        assert_eq!(app.get_status(), WAITING);
        st.sheets.fetching = false;
        st.sheets.fetch_failed.push(greycard_ai::SKY.id);
        resume(&mut st, &app);
        assert!(st.develop.landscape_pending.is_none());
        assert_eq!(
            app.get_status(),
            "the Sky model could not be fetched this session"
        );
    }
}
