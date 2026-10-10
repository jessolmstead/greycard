//! The People menu and the person a part is of. A part chosen from the
//! masks panel's menu on a picture of one person is made at once, as
//! Subject is, and is that person's, remembered as a pick is; on a
//! picture of no one it is made at once and is everyone's. On a
//! picture of more, the people are outlined and the next click picks
//! one, or All people takes everyone. A part that lands on a picture
//! where its person is not settled, pasted or synced there, asks the
//! same way, its guess outlined more strongly; one whose person is not
//! there at all asks too, with no guess, saying it was made for someone
//! else. A click there picks the person for this picture's copy alone.

use greycard_edit::mask::{PARTS, PartKind, Person, Route};

use crate::ai::{Ask, Asking, Candidate};
use crate::panel::assets::offer_model;
use crate::panel::edit::{read_edit, show_edit};
use crate::panel::mask::{Step, name_first_shape, step};
use crate::panel::viewport::{source_to_view, view_to_source};
use crate::*;

/// What the placing state is called while a person is to be picked.
pub(crate) const PICKING: &str = "Part";

/// A part chosen from the menu, waiting on the people on the picture
/// (or on its model's download first).
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    /// The adjustment it goes into, by id.
    pub(crate) id: u64,
    pub(crate) part: &'static PartKind,
    pub(crate) mode: Mode,
}

/// A person to be picked: for a new part, or for a placed one asking.
#[derive(Debug, Clone)]
pub(crate) struct Choosing {
    /// The adjustment, by id.
    pub(crate) id: u64,
    /// The part asking, by its index in the mask; none for a new part.
    pub(crate) component: Option<usize>,
    /// The new part's entry and mode.
    pub(crate) part: &'static PartKind,
    pub(crate) mode: Mode,
    pub(crate) ask: Ask,
}

/// The menu's entry by its label.
pub(crate) fn by_label(label: &str) -> Option<&'static PartKind> {
    PARTS.iter().find(|p| p.label == label)
}

/// What the status line says before a part is made: on the CPU, that
/// it takes a while; nothing otherwise.
fn before_run(route: &Route) -> Option<&'static str> {
    crate::ai::parts_on_cpu().then(|| crate::ai::part_on_cpu(route))
}

/// The People model's step for a part: ask, offer it, or wait.
fn part_step(st: &State, part: &PartKind) -> Option<Step> {
    let shape = Shape::of_kind(&format!("Part:{}", part.phrase));
    let store = st.store.clone();
    let have = |m: &greycard_ai::Model| store.as_ref().is_some_and(|s| s.have(m));
    let sheet_free = st.fetch.is_none() && !st.fetching;
    step(
        &shape,
        have,
        crate::ai::providers(),
        &st.declined,
        &st.fetch_failed,
        sheet_free,
        |_| false,
    )
}

/// A part chosen from the menu, by its label, into the chosen mask:
/// the people on the picture are found first, the model offered first
/// if it is not downloaded.
pub(crate) fn chosen(st: &mut State, app: &App, label: &str, mode: &str) {
    let Some(part) = by_label(label) else {
        app.set_status(format!("{label} is not on the People menu").into());
        return;
    };
    let Some(index) = st.target else {
        return;
    };
    let Some(a) = st.edit.adjustments.get(index) else {
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
    if st.placing.take().is_some() || st.choosing.take().is_some() {
        app.set_placing("".into());
    }
    st.part_pending = Some(Pending { id, part, mode });
    match part_step(st, part) {
        Some(Step::Ask(_)) => ask_people(app, part),
        Some(Step::Offer(model, falls_back)) => offer_model(st, app, model, falls_back),
        _ => {
            st.part_pending = None;
            app.set_status("the People model is not downloaded".into());
        }
    }
}

fn ask_people(app: &App, part: &PartKind) {
    WORKER.with(|w| {
        if let Some(w) = &*w.borrow() {
            w.send(Job::People);
        }
    });
    let status = before_run(&part.route).unwrap_or("finding the people");
    app.set_status(status.into());
}

/// The People model arrived (or its offer was answered): a part waiting
/// on it goes on, or is let go.
pub(crate) fn model_answered(st: &mut State, app: &App, have: bool) {
    let Some(pending) = st.part_pending.as_ref() else {
        return;
    };
    if have {
        ask_people(app, pending.part);
    } else {
        st.part_pending = None;
        app.set_status("without the People model no person or part can be found".into());
    }
}

/// The people on the picture, for the part waiting on them: with one
/// the part is made at once and is theirs, kept as a person picked on a
/// group is (their signature, where their face is and the picture), so
/// pasted onto a picture of someone else it finds no one rather than
/// them; with none, at once and everyone's, there being no one to
/// remember; with more, the picking starts. Whether the edit changed,
/// for the caller to say so once it has let go of the state.
pub(crate) fn people_found(
    st: &mut State,
    app: &App,
    file: Option<PathBuf>,
    people: Result<(Vec<Candidate>, &'static str, f64), String>,
) -> bool {
    let Some(pending) = st.part_pending.take() else {
        return false;
    };
    let open = st.current.and_then(|c| st.files.get(c)).cloned();
    if file.is_some() && file != open {
        // Another picture since: its people are not this one's.
        return false;
    }
    let (people, provider, seconds) = match people {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("People mask: {e}");
            app.set_status(format!("People mask: {e}").into());
            return false;
        }
    };
    tracing::info!(
        "{} people found on {provider} in {seconds:.2} s",
        people.len()
    );
    match &people[..] {
        [] => return add(st, app, &pending, None),
        [one] => return add(st, app, &pending, Some(one.person())),
        _ => {}
    }
    st.choosing = Some(Choosing {
        id: pending.id,
        component: None,
        part: pending.part,
        mode: pending.mode,
        ask: Ask {
            people,
            guess: None,
            why: Asking::Unsure,
        },
    });
    arm(app, "click the person, or All people");
    false
}

fn arm(app: &App, status: &str) {
    app.set_placing_into(true);
    app.set_placing(PICKING.into());
    app.set_status(status.into());
    app.window().request_redraw();
}

/// The part into its adjustment, of `person` or of everyone; whether
/// it went in.
fn add(st: &mut State, app: &App, pending: &Pending, person: Option<Person>) -> bool {
    let edit = read_edit(app, &st.edit, st.target);
    st.edit = edit;
    let Some((index, a)) = st
        .edit
        .adjustments
        .iter_mut()
        .enumerate()
        .find(|(_, a)| a.id == pending.id)
    else {
        return false;
    };
    a.mask.components.push(Component {
        shape: Shape::Part {
            phrase: pending.part.phrase.to_string(),
            route: pending.part.route.clone(),
            person,
        },
        mode: pending.mode,
        invert: false,
        enabled: true,
    });
    name_first_shape(a, pending.part.label);
    let which = a.mask.components.len() - 1;
    st.fresh_mask = None;
    st.target = Some(index);
    app.set_component(which as i32);
    show_edit(st, &st.edit, app, st.target);
    let status = before_run(&pending.part.route)
        .map(str::to_string)
        .unwrap_or_else(|| format!("finding the {}", pending.part.label.to_lowercase()));
    app.set_status(status.into());
    true
}

/// Put the person `person` (none: everyone) into the placed part
/// `component` of adjustment `id`; whether it went in.
fn repick(st: &mut State, app: &App, id: u64, component: usize, person: Option<Person>) -> bool {
    let edit = read_edit(app, &st.edit, st.target);
    st.edit = edit;
    let Some(c) = st
        .edit
        .adjustments
        .iter_mut()
        .find(|a| a.id == id)
        .and_then(|a| a.mask.components.get_mut(component))
    else {
        return false;
    };
    if let Shape::Part {
        person: p,
        route,
        phrase,
    } = &mut c.shape
    {
        *p = person;
        let route = route.clone();
        let label = greycard_edit::mask::part_of(phrase).map_or("part", |k| k.label);
        let status = before_run(&route)
            .map(str::to_string)
            .unwrap_or_else(|| format!("finding the {}", label.to_lowercase()));
        st.part_asks.remove(&(id, component));
        show_edit(st, &st.edit, app, st.target);
        app.set_status(status.into());
        return true;
    }
    false
}

/// A click on the picture while a person is to be picked: the person
/// under it, if any. None when no person is being picked; else whether
/// the edit changed.
pub(crate) fn pressed(st: &mut State, app: &App, x: f32, y: f32) -> Option<bool> {
    let choosing = st.choosing.as_ref()?;
    let at = view_to_source(st, app, x, y);
    let (w, h) = st.source_size;
    let aspect = if w > 0 { h as f32 / w as f32 } else { 1.0 };
    let Some(i) = crate::ai::picked(&choosing.ask.people, at, aspect) else {
        app.set_status("no one there: click a person, or All people".into());
        return Some(false);
    };
    let person = choosing.ask.people[i].person();
    let choosing = st.choosing.take().expect("checked above");
    app.set_placing("".into());
    Some(match choosing.component {
        None => add(
            st,
            app,
            &Pending {
                id: choosing.id,
                part: choosing.part,
                mode: choosing.mode,
            },
            Some(person),
        ),
        Some(c) => repick(st, app, choosing.id, c, Some(person)),
    })
}

/// All people, while a person is to be picked; whether the edit
/// changed.
pub(crate) fn all_people(st: &mut State, app: &App) -> bool {
    let Some(choosing) = st.choosing.take() else {
        return false;
    };
    app.set_placing("".into());
    match choosing.component {
        None => add(
            st,
            app,
            &Pending {
                id: choosing.id,
                part: choosing.part,
                mode: choosing.mode,
            },
            None,
        ),
        Some(c) => repick(st, app, choosing.id, c, None),
    }
}

/// Escape, or anything else that puts the tool down: the picking and
/// any part waiting on its people go with it.
pub(crate) fn stopped(st: &mut State) {
    st.choosing = None;
    st.part_pending = None;
}

/// A placed part asked which person it is of on this picture: kept,
/// and the picking armed when it is the chosen mask's on the Masks tab.
pub(crate) fn asked(st: &mut State, app: &App, key: Key, ask: Ask) {
    st.part_asks.insert(key, ask);
    rearm(st, app);
}

/// The mask, by index, to choose for the picking when the Masks tab is
/// shown: the chosen one when a part of it asks, else the first with a
/// part that asks; none when nothing on the picture asks.
pub(crate) fn asking_mask(st: &State) -> Option<usize> {
    let asks = |i: usize, dismissed: bool| {
        st.edit.adjustments.get(i).is_some_and(|a| {
            st.part_asks
                .keys()
                .any(|k| k.0 == a.id && (dismissed || !st.part_dismissed.contains(k)))
        })
    };
    // The mask chosen asks: it, whatever was put down. Else another
    // only for an ask not put down with Escape.
    st.target
        .filter(|&t| asks(t, true))
        .or_else(|| (0..st.edit.adjustments.len()).find(|&i| asks(i, false)))
}

/// Escape while a placed part asks: not brought back on its own.
pub(crate) fn dismissed(st: &mut State) {
    if let Some(Choosing {
        id,
        component: Some(c),
        ..
    }) = st.choosing
    {
        st.part_dismissed.insert((id, c));
    }
}

/// The picking for the chosen mask's part that asks, when there is one
/// and nothing else is in hand: on choosing the mask or the shape.
pub(crate) fn rearm(st: &mut State, app: &App) {
    if st.placing.is_some() || st.choosing.is_some() || app.get_panel_tab() != "Masks" {
        return;
    }
    let Some(a) = st.target.and_then(|i| st.edit.adjustments.get(i)) else {
        return;
    };
    let chosen = app.get_component().max(0) as usize;
    // The chosen shape first, then any of the mask's that asks.
    let key = std::iter::once(chosen)
        .chain(0..a.mask.components.len())
        .map(|c| (a.id, c))
        .find(|k| st.part_asks.contains_key(k));
    let Some(key) = key else {
        return;
    };
    let Some(Shape::Part { phrase, .. }) = a.mask.components.get(key.1).map(|c| &c.shape) else {
        st.part_asks.remove(&key);
        return;
    };
    let part = greycard_edit::mask::part_of(phrase).unwrap_or(&PARTS[0]);
    let ask = st.part_asks[&key].clone();
    let words = ask.words();
    st.choosing = Some(Choosing {
        id: key.0,
        component: Some(key.1),
        part,
        mode: Mode::Add,
        ask,
    });
    arm(app, words);
}

/// The people's outlines on the view while one is to be picked.
pub(crate) fn boxes(st: &State, app: &App) -> Vec<PersonBox> {
    let Some(choosing) = &st.choosing else {
        return Vec::new();
    };
    choosing
        .ask
        .people
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (x0, y0) = source_to_view(st, app, p.face[0], p.face[1]);
            let (x1, y1) = source_to_view(st, app, p.face[2], p.face[3]);
            PersonBox {
                x: x0.min(x1),
                y: y0.min(y1),
                w: (x1 - x0).abs(),
                h: (y1 - y0).abs(),
                guess: choosing.ask.guess == Some(i),
            }
        })
        .collect()
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_add_part(move |label, mode| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            chosen(&mut state.borrow_mut(), &app, label.as_str(), mode.as_str());
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_all_people(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let changed = all_people(&mut state.borrow_mut(), &app);
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

    /// Two people side by side, faces only, each known by a signature
    /// of their own.
    fn two() -> Vec<Candidate> {
        [[0.2, 0.1, 0.3, 0.25], [0.6, 0.1, 0.7, 0.25]]
            .into_iter()
            .enumerate()
            .map(|(i, face)| Candidate {
                face,
                at: [(face[0] + face[2]) / 2.0, (face[1] + face[3]) / 2.0],
                signature: greycard_edit::mask::Signature {
                    kind: "colors-1".into(),
                    values: vec![i as f32; 22],
                },
                body: None,
                picture: None,
            })
            .collect()
    }

    /// A window on the Masks tab with one empty mask chosen, over a
    /// picture 1200 by 800.
    fn masks() -> (App, Rc<RefCell<State>>, Rc<Worker>) {
        let app = window(1);
        let (state, worker) = state_for(&app, folder(1));
        app.set_panel_tab("Masks".into());
        {
            let mut st = state.borrow_mut();
            st.edit.adjustments.push(greycard_edit::Adjustment {
                id: 1,
                name: "Mask 1".into(),
                enabled: true,
                mask: Mask::default(),
                look: Default::default(),
            });
            st.target = Some(0);
            st.source_size = (1200, 800);
        }
        (app, state, worker)
    }

    fn pending(st: &mut State, label: &str) {
        st.part_pending = Some(Pending {
            id: 1,
            part: by_label(label).unwrap(),
            mode: Mode::Add,
        });
    }

    fn shapes(st: &State) -> Vec<Shape> {
        st.edit.adjustments[0]
            .mask
            .components
            .iter()
            .map(|c| c.shape.clone())
            .collect()
    }

    /// The view's point over a face's center.
    fn over(st: &State, app: &App, p: &Candidate) -> (f32, f32) {
        source_to_view(st, app, p.at[0], p.at[1])
    }

    /// A picture of one person never asks, and the part is hers: her
    /// signature, her face's place and her picture, as a pick on a
    /// group keeps them, never the everyone All people stores.
    #[test]
    fn a_part_on_a_picture_of_one_is_made_at_once_and_is_hers() {
        let (app, state, _worker) = masks();
        let mut st = state.borrow_mut();
        pending(&mut st, "Iris");
        let mut her = two().remove(0);
        her.picture = Some("5e1d".into());
        assert!(people_found(
            &mut st,
            &app,
            None,
            Ok((vec![her.clone()], "CPU", 0.1))
        ));
        let Shape::Part {
            phrase,
            route,
            person: Some(p),
        } = &shapes(&st)[0]
        else {
            panic!("a part of her: {:?}", shapes(&st));
        };
        assert_eq!((phrase.as_str(), route), ("iris of the eye", &Route::Eye));
        assert_eq!(*p, her.person());
        assert_eq!(p.picture.as_deref(), Some("5e1d"));
        assert_eq!(p.at, her.at);
        assert_ne!(shapes(&st)[0], Shape::of_kind("Part:iris of the eye"));
        assert!(st.choosing.is_none() && st.part_pending.is_none());
        assert_eq!(app.get_placing(), "");
        assert!(boxes(&st, &app).is_empty(), "no one outlined");
        assert_eq!(st.edit.adjustments[0].name, "Iris 1");
        // A face no body went with is remembered the same way.
        pending(&mut st, "Lips");
        let bodiless = two().remove(1);
        assert!(bodiless.body.is_none());
        assert!(people_found(
            &mut st,
            &app,
            None,
            Ok((vec![bodiless.clone()], "CPU", 0.1))
        ));
        assert!(matches!(
            &shapes(&st)[1],
            Shape::Part { person: Some(p), .. } if *p == bodiless.person()
        ));
        // Nobody at all: no one to remember, everyone's.
        pending(&mut st, "Hair");
        assert!(people_found(
            &mut st,
            &app,
            None,
            Ok((Vec::new(), "CPU", 0.1))
        ));
        assert_eq!(shapes(&st)[2], Shape::of_kind("Part:hair"));
    }

    #[test]
    fn a_part_on_a_picture_of_two_waits_for_the_person_picked() {
        let (app, state, _worker) = masks();
        let mut st = state.borrow_mut();
        pending(&mut st, "Lips");
        assert!(!people_found(&mut st, &app, None, Ok((two(), "CPU", 0.1))));
        assert!(shapes(&st).is_empty(), "nothing made before the pick");
        assert_eq!(app.get_placing(), PICKING);
        assert_eq!(app.get_status(), "click the person, or All people");
        let outlined = boxes(&st, &app);
        assert_eq!(outlined.len(), 2);
        assert!(outlined.iter().all(|b| !b.guess && b.w > 0.0 && b.h > 0.0));
        // A click on no one keeps the picking.
        let (x, y) = source_to_view(&st, &app, 0.95, 0.6);
        assert_eq!(pressed(&mut st, &app, x, y), Some(false));
        assert!(st.choosing.is_some());
        // A click on the second face picks her.
        let people = two();
        let (x, y) = over(&st, &app, &people[1]);
        assert_eq!(pressed(&mut st, &app, x, y), Some(true));
        assert_eq!(
            shapes(&st),
            [Shape::Part {
                phrase: "lips".into(),
                route: Route::Mouth,
                person: Some(people[1].person()),
            }]
        );
        assert!(st.choosing.is_none());
        assert_eq!(app.get_placing(), "");
        assert!(boxes(&st, &app).is_empty());
        // Not picking: a click is not the picking's.
        assert_eq!(pressed(&mut st, &app, x, y), None);
    }

    #[test]
    fn all_people_and_escape_while_a_person_is_to_be_picked() {
        let (app, state, _worker) = masks();
        {
            let mut st = state.borrow_mut();
            pending(&mut st, "Top");
            people_found(&mut st, &app, None, Ok((two(), "CPU", 0.1)));
            assert!(all_people(&mut st, &app));
            assert_eq!(shapes(&st), [Shape::of_kind("Part:upper body clothing")]);
            assert_eq!(app.get_placing(), "");
            pending(&mut st, "Hands");
            people_found(&mut st, &app, None, Ok((two(), "CPU", 0.1)));
        }
        // Escape puts it down, and nothing is made.
        app.invoke_stop_placing();
        let st = state.borrow();
        assert!(st.choosing.is_none() && st.part_pending.is_none());
        assert_eq!(app.get_placing(), "");
        assert_eq!(shapes(&st).len(), 1);
    }

    /// A placed part that asks on this picture: the people outlined,
    /// the guess more strongly, and a click picks the person for this
    /// picture's copy of the shape.
    #[test]
    fn a_part_that_asks_is_picked_again_on_its_picture() {
        let (app, state, _worker) = masks();
        let mut st = state.borrow_mut();
        let people = two();
        let elsewhere = Shape::Part {
            phrase: "lips".into(),
            route: Route::Mouth,
            person: Some(greycard_edit::mask::Person {
                signature: greycard_edit::mask::Signature {
                    kind: "colors-1".into(),
                    values: vec![7.0; 22],
                },
                at: [0.9, 0.9],
                picture: None,
            }),
        };
        st.edit.adjustments[0].mask.components.push(Component {
            shape: elsewhere,
            ..Default::default()
        });
        asked(
            &mut st,
            &app,
            (1, 0),
            Ask {
                people: people.clone(),
                guess: Some(0),
                why: Asking::Unsure,
            },
        );
        assert_eq!(app.get_placing(), PICKING);
        assert_eq!(app.get_status(), "which person? click one, or All people");
        let guessed: Vec<bool> = boxes(&st, &app).iter().map(|b| b.guess).collect();
        assert_eq!(guessed, [true, false], "the guess highlighted, not picked");
        assert!(matches!(
            &shapes(&st)[0],
            Shape::Part { person: Some(p), .. } if p.at == [0.9, 0.9]
        ));
        let (x, y) = over(&st, &app, &people[1]);
        assert_eq!(pressed(&mut st, &app, x, y), Some(true));
        assert!(matches!(
            &shapes(&st)[0],
            Shape::Part { person: Some(p), .. } if *p == people[1].person()
        ));
        assert!(st.part_asks.is_empty());
        // Asked again, All people takes everyone on this copy.
        asked(
            &mut st,
            &app,
            (1, 0),
            Ask {
                people,
                guess: None,
                why: Asking::Unsure,
            },
        );
        assert!(all_people(&mut st, &app));
        assert!(matches!(&shapes(&st)[0], Shape::Part { person: None, .. }));
    }

    /// A placed part asking, as the worker says it, on `file`.
    fn ask_from(file: Option<PathBuf>) -> Outcome {
        Outcome::Mask {
            key: (1, 0),
            shape: Shape::of_kind("Part:lips"),
            file,
            raster: Arc::new(Raster::from_data(1.0, 4, vec![0; 16])),
            provider: Some("CPU"),
            seconds: 0.1,
            note: Some("which person? click one, or All people".into()),
            part: Some(crate::ai::Unresolved::Ask(Ask {
                people: two(),
                guess: None,
                why: Asking::Unsure,
            })),
        }
    }

    fn with_a_part(state: &Rc<RefCell<State>>) {
        let mut st = state.borrow_mut();
        st.edit.adjustments[0].mask.components.push(Component {
            shape: Shape::of_kind("Part:lips"),
            ..Default::default()
        });
        st.current = Some(0);
    }

    /// An ask made on a picture no longer open is that picture's: it
    /// arms nothing on the one open now.
    #[test]
    fn an_ask_from_another_picture_is_dropped() {
        let (app, state, _worker) = masks();
        with_a_part(&state);
        let open = state.borrow().files[0].clone();
        crate::panel::deliver::deliver(&app, ask_from(Some(PathBuf::from("/nowhere/else.CR3"))));
        assert!(state.borrow().part_asks.is_empty());
        assert_eq!(app.get_placing(), "");
        crate::panel::deliver::deliver(&app, ask_from(Some(open)));
        assert_eq!(state.borrow().part_asks.len(), 1);
        assert_eq!(app.get_placing(), PICKING);
    }

    /// An ask that lands off the Masks tab waits; showing the tab
    /// chooses its mask and arms the picking.
    #[test]
    fn an_ask_off_the_masks_tab_is_armed_when_the_tab_is_shown() {
        let (app, state, _worker) = masks();
        with_a_part(&state);
        app.set_panel_tab("Develop".into());
        app.invoke_tab_changed("Develop".into());
        assert_eq!(state.borrow().target, None);
        let open = state.borrow().files[0].clone();
        crate::panel::deliver::deliver(&app, ask_from(Some(open)));
        assert_eq!(app.get_placing(), "", "not off the Masks tab");
        app.set_panel_tab("Masks".into());
        app.invoke_tab_changed("Masks".into());
        assert_eq!(state.borrow().target, Some(0));
        assert_eq!(app.get_placing(), PICKING);
        assert_eq!(boxes(&state.borrow(), &app).len(), 2);
        // Put down with Escape, it is not brought back by the tab: the
        // mask the user goes to is theirs to choose.
        app.invoke_stop_placing();
        app.set_panel_tab("Develop".into());
        app.invoke_tab_changed("Develop".into());
        app.set_panel_tab("Masks".into());
        app.invoke_tab_changed("Masks".into());
        assert_eq!(state.borrow().target, None);
        assert_eq!(app.get_placing(), "");
        // Choosing its mask brings it back.
        app.invoke_target_changed(1);
        assert_eq!(app.get_placing(), PICKING);
    }

    /// A lips mask made on one woman's portrait, as pasted onto
    /// another picture: her signature, her face's place there.
    fn made_for_her(st: &mut State) {
        st.edit.adjustments[0].mask.components.push(Component {
            shape: Shape::Part {
                phrase: "lips".into(),
                route: Route::Mouth,
                person: Some(greycard_edit::mask::Person {
                    signature: greycard_edit::mask::Signature {
                        kind: "colors-1".into(),
                        values: vec![7.0; 22],
                    },
                    at: [0.9, 0.9],
                    picture: Some("her portrait".into()),
                }),
            },
            ..Default::default()
        });
        st.current = Some(0);
    }

    /// A part whose person is not on the open picture, as the worker
    /// says it (`ai::decided`): asking, no guess, made for someone else.
    fn someone_else(people: Vec<Candidate>, file: PathBuf) -> Outcome {
        let ask = Ask {
            people,
            guess: None,
            why: Asking::SomeoneElse,
        };
        Outcome::Mask {
            key: (1, 0),
            shape: Shape::of_kind("Part:lips"),
            file: Some(file),
            raster: Arc::new(Raster::from_data(1.0, 4, vec![0; 16])),
            provider: Some("CPU"),
            seconds: 0.1,
            note: Some(ask.words().into()),
            part: Some(crate::ai::Unresolved::Ask(ask)),
        }
    }

    /// The status line while a part made for someone else asks.
    fn someone_else_words() -> &'static str {
        Ask {
            why: Asking::SomeoneElse,
            ..Default::default()
        }
        .words()
    }

    /// Pasted onto a portrait of someone else, a part asks as an unsure
    /// one does, with no guess and its own words; a click on her makes
    /// this picture's copy hers, as she is known here.
    #[test]
    fn a_part_made_for_someone_else_asks_on_a_portrait() {
        let (app, state, _worker) = masks();
        made_for_her(&mut state.borrow_mut());
        let open = state.borrow().files[0].clone();
        let mut stranger = two().remove(1);
        stranger.picture = Some("this picture".into());
        crate::panel::deliver::deliver(&app, someone_else(vec![stranger.clone()], open));
        let mut st = state.borrow_mut();
        assert_eq!(app.get_placing(), PICKING);
        assert_eq!(app.get_status(), someone_else_words());
        let outlined = boxes(&st, &app);
        assert_eq!(outlined.len(), 1);
        assert!(!outlined[0].guess, "outlined, never a guess");
        assert!(matches!(
            &shapes(&st)[0],
            Shape::Part { person: Some(p), .. } if p.at == [0.9, 0.9]
        ));
        let (x, y) = over(&st, &app, &stranger);
        assert_eq!(pressed(&mut st, &app, x, y), Some(true));
        assert!(matches!(
            &shapes(&st)[0],
            Shape::Part { person: Some(p), .. }
                if *p == stranger.person() && p.picture.as_deref() == Some("this picture")
        ));
        assert!(st.part_asks.is_empty() && st.choosing.is_none());
        assert_eq!(app.get_placing(), "");
    }

    /// On a group, every face is outlined, none as a guess; All people
    /// makes this picture's copy everyone's, and Escape leaves it as it
    /// was, its mask empty, until its mask is chosen again.
    #[test]
    fn a_part_made_for_someone_else_on_a_group_all_people_and_escape() {
        let (app, state, _worker) = masks();
        made_for_her(&mut state.borrow_mut());
        let open = state.borrow().files[0].clone();
        crate::panel::deliver::deliver(&app, someone_else(two(), open.clone()));
        {
            let mut st = state.borrow_mut();
            assert_eq!(app.get_status(), someone_else_words());
            let guessed: Vec<bool> = boxes(&st, &app).iter().map(|b| b.guess).collect();
            assert_eq!(guessed, [false, false]);
            assert!(all_people(&mut st, &app));
            assert!(matches!(&shapes(&st)[0], Shape::Part { person: None, .. }));
            // Back to hers, asked again.
            st.edit.adjustments[0].mask.components.clear();
            made_for_her(&mut st);
        }
        crate::panel::deliver::deliver(&app, someone_else(two(), open));
        assert_eq!(app.get_placing(), PICKING);
        app.invoke_stop_placing();
        {
            let st = state.borrow();
            assert_eq!(app.get_placing(), "");
            assert!(st.choosing.is_none());
            assert!(matches!(
                &shapes(&st)[0],
                Shape::Part { person: Some(p), .. } if p.at == [0.9, 0.9]
            ));
            assert!(
                st.part_asks.contains_key(&(1, 0)),
                "still asking, not armed"
            );
        }
        // Coming back to the Masks tab and choosing its mask asks again,
        // in its own words.
        app.set_panel_tab("Develop".into());
        app.invoke_tab_changed("Develop".into());
        app.set_panel_tab("Masks".into());
        app.invoke_tab_changed("Masks".into());
        app.invoke_target_changed(1);
        assert_eq!(app.get_placing(), PICKING);
        assert_eq!(app.get_status(), someone_else_words());
        assert_eq!(boxes(&state.borrow(), &app).len(), 2);
    }

    /// On a picture of no one, nothing is outlined: the mask stays
    /// empty and the status line says so, as before.
    #[test]
    fn a_part_on_a_picture_of_no_one_asks_nothing() {
        let (app, state, _worker) = masks();
        made_for_her(&mut state.borrow_mut());
        let open = state.borrow().files[0].clone();
        let note = "no one is in 0.CR3";
        crate::panel::deliver::deliver(
            &app,
            Outcome::Mask {
                key: (1, 0),
                shape: Shape::of_kind("Part:lips"),
                file: Some(open),
                raster: Arc::new(Raster::from_data(1.0, 4, vec![0; 16])),
                provider: Some("CPU"),
                seconds: 0.1,
                note: Some(note.into()),
                part: Some(crate::ai::Unresolved::Nobody),
            },
        );
        let st = state.borrow();
        assert_eq!(app.get_placing(), "");
        assert_eq!(app.get_status(), note);
        assert!(st.part_asks.is_empty() && st.choosing.is_none());
        assert!(boxes(&st, &app).is_empty());
    }

    /// The whole person goes as every part does: made at once and hers
    /// on a picture of one, the mask named for it; on a picture of two,
    /// the one clicked, or All people.
    #[test]
    fn a_whole_person_is_made_and_picked_as_a_part_is() {
        let (app, state, _worker) = masks();
        let mut st = state.borrow_mut();
        let person = |p: Option<greycard_edit::mask::Person>| Shape::Part {
            phrase: "person".into(),
            route: Route::Whole,
            person: p,
        };
        pending(&mut st, "Whole person");
        let her = two().remove(0);
        assert!(people_found(
            &mut st,
            &app,
            None,
            Ok((vec![her.clone()], "CPU", 0.1))
        ));
        assert_eq!(shapes(&st), [person(Some(her.person()))]);
        assert_eq!(st.edit.adjustments[0].name, "Whole person 1");
        // Two: the second picked.
        pending(&mut st, "Whole person");
        assert!(!people_found(&mut st, &app, None, Ok((two(), "CPU", 0.1))));
        assert_eq!(app.get_placing(), PICKING);
        assert_eq!(boxes(&st, &app).len(), 2);
        let people = two();
        let (x, y) = over(&st, &app, &people[1]);
        assert_eq!(pressed(&mut st, &app, x, y), Some(true));
        assert_eq!(shapes(&st)[1], person(Some(people[1].person())));
        // Two again: All people.
        pending(&mut st, "Whole person");
        people_found(&mut st, &app, None, Ok((two(), "CPU", 0.1)));
        assert!(all_people(&mut st, &app));
        assert_eq!(shapes(&st)[2], person(None));
        assert_eq!(app.get_placing(), "");
    }

    #[test]
    fn a_label_the_menu_does_not_have_says_so() {
        let (app, state, _worker) = masks();
        chosen(&mut state.borrow_mut(), &app, "Ears", "Add");
        assert!(app.get_status().contains("Ears"));
        assert!(state.borrow().part_pending.is_none());
    }

    /// The menu in `mask.slint` and the table are the same labels, so
    /// no entry asks for a part the table does not have; the whole
    /// person first, on its own above the groups.
    #[test]
    fn the_menu_is_the_tables_labels() {
        let slint = include_str!("../../ui/panel/mask.slint");
        let mut menu: Vec<&str> = slint
            .split("root.add-part(\"")
            .skip(1)
            .map(|s| s.split('"').next().unwrap())
            .collect();
        let table: Vec<&str> = PARTS.iter().map(|p| p.label).collect();
        assert_eq!(menu, table, "in the menu's order");
        menu.dedup();
        assert_eq!(menu.len(), PARTS.len());
        for p in PARTS {
            assert_eq!(by_label(p.label), Some(p));
        }
        assert_eq!(menu[0], "Whole person");
        // The button says People, under a two-person icon Subject's
        // one-person icon is told from.
        let button = slint.split("parts-button := PanelButton {").nth(1).unwrap();
        assert!(button.contains("text: \"People\";"));
        assert!(button.contains("users-round.svg"));
        let first = slint.split("Menu {").nth(1).unwrap();
        let (top, rest) = first.split_once("MenuSeparator").unwrap();
        assert_eq!(top.matches("MenuItem").count(), 1, "{top}");
        assert!(top.contains("add-part(\"Whole person\""));
        let group = rest.split("MenuItem").nth(1).unwrap();
        assert!(group.contains("title: \"Face\"; enabled: false"), "{group}");
    }

    /// The view as it stands: the zoom asked for, the center and the
    /// frame's size.
    fn view(state: &Rc<RefCell<State>>) -> (f32, (f32, f32), (u32, u32)) {
        let st = state.borrow();
        (st.zoom, st.center, st.image_size)
    }

    /// The part's raster and the develop after it, as the worker
    /// delivers them.
    fn land(app: &App, state: &Rc<RefCell<State>>) {
        let (shape, generation) = {
            let st = state.borrow();
            (shapes(&st)[0].clone(), st.generation)
        };
        crate::panel::deliver::deliver(
            app,
            Outcome::Mask {
                key: (1, 0),
                shape,
                file: None,
                raster: Arc::new(Raster::from_data(1.0, 4, vec![255; 16])),
                provider: Some("CPU"),
                seconds: 0.1,
                note: None,
                part: None,
            },
        );
        crate::panel::deliver::deliver(
            app,
            Outcome::Developed {
                generation,
                turn: 0,
                image: crate::worker::Developed::Halves(Arc::new(crate::worker::Halves {
                    width: 1200,
                    height: 800,
                    pixels: Vec::new(),
                })),
                guide: Arc::new(crate::finish::Guide::NONE),
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 0.1,
                detail: None,
                sharpen: None,
                dehaze: None,
                sources: Vec::new(),
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport::default(),
            },
        );
    }

    /// Zoomed in on a group, a person picked with a click on her face
    /// is the pick's click and not the zoom's: the press takes the
    /// picking out of hand, and its release used to read as a plain
    /// click on the picture and go back to Fit. The view stays where
    /// it was through the pick, the part's raster landing and the
    /// develop after it; and on a picture of one, where the part is
    /// made with no click, the same.
    #[test]
    fn a_person_picked_on_a_zoomed_view_keeps_the_view() {
        use i_slint_backend_testing::ElementHandle;
        let (app, state, _worker) = masks();
        {
            let mut st = state.borrow_mut();
            st.image_size = (1200, 800);
            st.zoom = 2.0;
            st.center = (330.0, 230.0);
            pending(&mut st, "Lips");
        }
        let before = view(&state);
        crate::panel::deliver::deliver(
            &app,
            Outcome::People {
                file: None,
                people: Ok((two(), "CPU", 0.1)),
            },
        );
        assert_eq!(app.get_placing(), PICKING);
        assert_eq!(view(&state), before, "the people found move nothing");

        // The click, inside the first face's own outline on the
        // viewport.
        let viewport = ElementHandle::find_by_element_type_name(&app, "Viewport")
            .next()
            .expect("the viewport is on screen");
        let (base, size) = (viewport.absolute_position(), viewport.size());
        let people = two();
        let (x, y) = over(&state.borrow(), &app, &people[0]);
        assert!(
            x > 20.0 && y > 60.0 && x < size.width - 20.0 && y < size.height - 20.0,
            "the face is on the view, clear of All people: {x}, {y} in {size:?}"
        );
        crate::testing::click(&app, base.x + x, base.y + y);
        assert_eq!(
            shapes(&state.borrow()),
            [Shape::Part {
                phrase: "lips".into(),
                route: Route::Mouth,
                person: Some(people[0].person()),
            }],
            "the click picked her"
        );
        assert_eq!(view(&state), before, "the pick's release is not a zoom");
        land(&app, &state);
        assert_eq!(view(&state), before, "nor is the mask landing");

        // A picture of one: made at once, landed, the view where it was.
        {
            let mut st = state.borrow_mut();
            st.edit.adjustments[0].mask.components.clear();
            pending(&mut st, "Lips");
        }
        crate::panel::deliver::deliver(
            &app,
            Outcome::People {
                file: None,
                people: Ok((vec![people[1].clone()], "CPU", 0.1)),
            },
        );
        assert_eq!(shapes(&state.borrow()).len(), 1);
        land(&app, &state);
        assert_eq!(view(&state), before);

        // A plain click on the picture still goes back to Fit: what
        // is latched is the press's, not a rule against zooming.
        crate::testing::click(&app, base.x + x, base.y + y);
        assert_eq!(state.borrow().zoom, 0.0, "a plain click goes back to Fit");
    }

    /// Every phrase the menu asks for is in the table the model ships
    /// (`prompts.py`'s SHIPPED, which `table.py` writes), and so are
    /// the ones the routes and the people ask on their own.
    #[test]
    fn every_phrase_is_shipped() {
        let prompts = include_str!("../../../../tools/ai/sam3_trial/prompts.py");
        let shipped = prompts
            .split("SHIPPED = [")
            .nth(1)
            .and_then(|s| s.split(']').next())
            .expect("prompts.py has SHIPPED");
        let phrases: Vec<&str> = shipped
            .lines()
            .map(|l| l.split('#').next().unwrap_or(""))
            .flat_map(|l| l.split(','))
            .map(|p| p.trim().trim_matches('"'))
            .filter(|p| !p.is_empty())
            .collect();
        for p in PARTS {
            assert!(phrases.contains(&p.phrase), "{} ({})", p.label, p.phrase);
        }
        for own in [
            greycard_ai::sam3::FACE,
            greycard_ai::sam3::EYES,
            greycard_ai::sam3::MOUTH,
            greycard_ai::sam3::PERSON,
        ] {
            assert!(phrases.contains(&own), "{own}");
        }
    }
}
