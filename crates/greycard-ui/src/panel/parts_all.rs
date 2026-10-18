//! "Use this person on all N pictures". A paste or a sync that takes a
//! People shape of one person onto many frames leaves each frame to
//! find that person again, and where it cannot (a group, someone else
//! there) to ask, one frame at a time. A person picked for the shape on
//! one of those frames is offered for the rest, on a line under that
//! mask's shapes: taken, each other frame that still holds the shape as
//! it came is looked at on the worker, its reduced develop under its
//! own edit, and given the person where they are found there by their
//! colors and their place on the frame picked on (`ai::taken`), as
//! that frame's own pick of them. A frame whose own person is decided
//! alone already is left as it is; one where they are not found is
//! left asking, and counted in the status line's word for the run.
//!
//! Each frame takes the person as one step of its own history, "Person
//! from" the frame picked on, written to its sidecar as a paste's step
//! is; undone there as any step is. A frame whose shape changed between
//! the press and its answer (picked by hand meanwhile, or the shape
//! gone) is left as it is and nothing is written. No face is kept
//! anywhere but the signature a pick stores already.

use std::collections::HashSet;

use greycard_edit::history::person_label;
use greycard_edit::mask::Person;
use greycard_edit::preset::Preset;

use crate::ai::{PersonOn, Picked};
use crate::panel::browser::{show_badges, show_thumb, thumb_turns};
use crate::panel::edit::{read_edit, show_edit};
use crate::versions::Key as Frame;
use crate::worker::PersonJob;
use crate::*;

/// What the window keeps of it, on the develop's state.
#[derive(Debug, Default)]
pub(crate) struct All {
    /// The frames the last paste or sync that carried a People shape
    /// of a person took it to, and those shapes as they went.
    pub(crate) reached: Option<Reached>,
    /// The offer standing on the open frame, after a pick there.
    pub(crate) offer: Option<Offer>,
    /// The run taken from an offer, until each of its frames answers.
    pub(crate) batch: Option<Batch>,
    /// The last run's number, so an answer for an older one is let go.
    runs: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct Reached {
    pub(crate) frames: Vec<Frame>,
    pub(crate) shapes: Vec<Shape>,
}

#[derive(Debug, Clone)]
pub(crate) struct Offer {
    /// The frame the person was picked on.
    pub(crate) frame: Frame,
    /// The mask the shape is in, by id.
    pub(crate) mask: u64,
    /// The shape as the paste or the sync left it, before the pick.
    pub(crate) carried: Shape,
    /// The person picked, and how.
    pub(crate) wanted: Picked,
    /// The other frames that hold `carried` still.
    pub(crate) others: Vec<Frame>,
}

#[derive(Debug, Clone)]
pub(crate) struct Batch {
    pub(crate) id: u64,
    pub(crate) carried: Shape,
    /// The frame picked on, by name, for each step's words.
    pub(crate) from: String,
    /// The frames still to answer.
    pub(crate) waiting: HashSet<Frame>,
    /// Every frame of it, the one picked on among them.
    pub(crate) count: usize,
    /// The frames with the person: the one picked on, and each found.
    pub(crate) used: usize,
    /// The frames whose own person was decided already.
    pub(crate) settled: usize,
    pub(crate) not_found: usize,
    /// The frames whose shape is not as it came any more by the time
    /// their answer landed: picked by hand meanwhile, or taken away.
    pub(crate) changed: usize,
    /// The frames that could not be looked at: their read or develop
    /// failed, or they are no longer listed.
    pub(crate) failed: usize,
}

impl Offer {
    /// The offer's words: every frame, the one picked on too.
    pub(crate) fn words(&self) -> String {
        format!("Use this person on all {} pictures", self.others.len() + 1)
    }

    /// The shape as the pick left it.
    fn given(&self) -> Shape {
        with_person(&self.carried, &self.wanted.person)
    }
}

impl Batch {
    /// What the status line says of it once every frame has answered.
    pub(crate) fn words(&self) -> String {
        let mut said = format!(
            "used this person on {} of {} pictures",
            self.used, self.count
        );
        let mut and = |n: usize, what: &str| {
            if n > 0 {
                said.push_str(&format!("; {n} {what}"));
            }
        };
        and(self.settled, "kept its own person");
        and(self.not_found, "not found");
        and(self.changed, "changed since");
        and(self.failed, "could not be looked at");
        said
    }
}

/// `shape` of `person`.
fn with_person(shape: &Shape, person: &Person) -> Shape {
    let mut shape = shape.clone();
    if let Shape::Part { person: p, .. } = &mut shape {
        *p = Some(person.clone());
    }
    shape
}

/// The People shapes of a person `edit` holds.
fn people_shapes(edit: &Edit) -> Vec<Shape> {
    edit.adjustments
        .iter()
        .flat_map(|a| a.mask.components.iter())
        .filter(|c| {
            matches!(
                c.shape,
                Shape::Part {
                    person: Some(_),
                    ..
                }
            )
        })
        .map(|c| c.shape.clone())
        .collect()
}

/// A paste or a sync laid `preset` over `cells`. One that carried a
/// People shape of a person makes those frames the ones a pick on any
/// of them is offered for, in place of the last such set; onto a
/// single frame, it leaves none. One that carried none (no masks, or
/// masks with no such shape) leaves the last set as it was.
pub(crate) fn reached(st: &mut State, app: &App, preset: &Preset, cells: &[usize]) {
    if !preset
        .sections
        .contains(&greycard_edit::preset::Section::Adjustments)
    {
        return;
    }
    let shapes = people_shapes(&preset.edit);
    if shapes.is_empty() {
        return;
    }
    let frames: Vec<Frame> = cells.iter().filter_map(|&c| st.key_of(c)).collect();
    let all = &mut st.develop.people_all;
    all.reached = (frames.len() > 1).then_some(Reached { frames, shapes });
    all.offer = None;
    show(st, app);
}

/// Whether cell `c`'s line holds `shape`.
fn holds(st: &State, c: usize, shape: &Shape) -> bool {
    st.line(c).is_some_and(|l| {
        l.current
            .adjustments
            .iter()
            .any(|a| a.mask.components.iter().any(|k| k.shape == *shape))
    })
}

/// A person picked on the open frame for its People shape in mask
/// `mask` that was `before` (as it came) and is of `wanted` now, out of
/// an ask of two or more when `group`: offered for the other frames the
/// last paste or sync took that shape to, those that hold it still; no
/// offer with none, for a shape that came some other way, or for All
/// people (`wanted` none).
pub(crate) fn picked(
    st: &mut State,
    app: &App,
    mask: u64,
    before: &Shape,
    wanted: Option<&Person>,
    group: bool,
) {
    let all = &st.develop.people_all;
    let open = st.current.and_then(|c| st.key_of(c));
    let (w, h) = st.develop.source_size;
    let offer = match (&all.reached, open, wanted) {
        (Some(r), Some(open), Some(wanted))
            if r.frames.contains(&open) && r.shapes.contains(before) =>
        {
            let others: Vec<Frame> = r
                .frames
                .iter()
                .filter(|f| **f != open)
                .filter(|f| st.find_cell(f).is_some_and(|c| holds(st, c, before)))
                .cloned()
                .collect();
            (!others.is_empty()).then(|| Offer {
                frame: open,
                mask,
                carried: before.clone(),
                wanted: Picked {
                    person: wanted.clone(),
                    group,
                    turn: crate::panel::edit::current_turn(st) % 4,
                    aspect: if w > 0 { h as f32 / w as f32 } else { 1.0 },
                },
                others,
            })
        }
        _ => None,
    };
    st.develop.people_all.offer = offer;
    show(st, app);
}

/// Where the offer stands on `edit`, the open frame's: the index of the
/// mask whose shape still has the person picked, or none when it is
/// another frame or the shape no longer has them (the pick undone, the
/// shape taken away or picked again).
fn stands(st: &State, offer: &Offer, edit: &Edit) -> Option<usize> {
    let open = st.current.and_then(|c| st.key_of(c))?;
    if open != offer.frame {
        return None;
    }
    let given = offer.given();
    edit.adjustments
        .iter()
        .position(|a| a.id == offer.mask && a.mask.components.iter().any(|k| k.shape == given))
}

/// The offer on the panel as `edit`, the open frame's edit being shown,
/// has it: its words and its mask while it stands there, else nothing.
pub(crate) fn shown(st: &State, edit: &Edit, app: &App) {
    let standing = st
        .develop
        .people_all
        .offer
        .as_ref()
        .and_then(|o| Some((o.words(), stands(st, o, edit)?)));
    let (words, mask) = match standing {
        // As the panel's `target` counts the masks: Global is 0.
        Some((words, i)) => (words, i as i32 + 1),
        None => (String::new(), -1),
    };
    if app.get_people_offer() != words.as_str() {
        app.set_people_offer(words.into());
    }
    if app.get_people_offer_mask() != mask {
        app.set_people_offer_mask(mask);
    }
}

/// The offer on the panel, let go first when it no longer stands on the
/// open frame's edit.
fn show(st: &mut State, app: &App) {
    if let Some(o) = &st.develop.people_all.offer
        && stands(st, o, &st.develop.edit).is_none()
    {
        st.develop.people_all.offer = None;
    }
    shown(st, &st.develop.edit, app);
}

/// Another frame is open: an offer was the last one's.
pub(crate) fn another_picture(st: &mut State, app: &App) {
    st.develop.people_all.offer = None;
    show(st, app);
}

/// The panel takes its line's state again (an undo, a redo, a history
/// row or a snapshot): the person was picked on the state it left.
pub(crate) fn history_moved(st: &mut State, app: &App) {
    if st.develop.people_all.offer.take().is_some() {
        show(st, app);
    }
}

/// The offer put away: not made again for this paste or sync.
pub(crate) fn dismissed(st: &mut State, app: &App) {
    let all = &mut st.develop.people_all;
    all.offer = None;
    all.reached = None;
    show(st, app);
}

/// The offer taken: each other frame that holds the shape still is
/// sent to be looked at, and the run waits on their answers. Whatever
/// a run before it still had out is let go. An offer whose shape no
/// longer has the person picked is let go instead.
pub(crate) fn used(st: &mut State, app: &App, worker: &Worker) {
    let Some(offer) = st.develop.people_all.offer.take() else {
        return;
    };
    if stands(st, &offer, &st.develop.edit).is_none() {
        show(st, app);
        app.set_status("this mask no longer has the person picked".into());
        return;
    }
    st.develop.people_all.reached = None;
    show(st, app);
    let carried = match &offer.carried {
        Shape::Part { person, .. } => person.clone(),
        _ => None,
    };
    let jobs: Vec<PersonJob> = offer
        .others
        .iter()
        .filter_map(|f| {
            let c = st.find_cell(f)?;
            holds(st, c, &offer.carried).then_some(())?;
            Some(PersonJob {
                batch: 0,
                frame: f.clone(),
                edit: st.line(c)?.current.clone(),
                turn: st.sidecar_at(c)?.turn,
                carried: carried.clone(),
                wanted: offer.wanted.clone(),
            })
        })
        .collect();
    if jobs.is_empty() {
        app.set_status("the other pictures no longer have this mask".into());
        return;
    }
    let all = &mut st.develop.people_all;
    all.runs += 1;
    let id = all.runs;
    let from = offer.frame.said();
    all.batch = Some(Batch {
        id,
        carried: offer.carried.clone(),
        from,
        waiting: jobs.iter().map(|j| j.frame.clone()).collect(),
        count: jobs.len() + 1,
        used: 1,
        settled: 0,
        not_found: 0,
        changed: 0,
        failed: 0,
    });
    let n = jobs.len();
    for mut job in jobs {
        job.batch = id;
        worker.send(Job::PersonOn(Box::new(job)));
    }
    app.set_status(
        format!(
            "looking for this person on {n} more picture{}",
            if n == 1 { "" } else { "s" }
        )
        .into(),
    );
}

/// `shape` in `edit` given `person`, wherever it stands; whether any did.
fn give(edit: &mut Edit, shape: &Shape, person: &Person) -> bool {
    let mut gave = false;
    for c in edit
        .adjustments
        .iter_mut()
        .flat_map(|a| a.mask.components.iter_mut())
        .filter(|c| c.shape == *shape)
    {
        if let Shape::Part { person: p, .. } = &mut c.shape {
            *p = Some(person.clone());
            gave = true;
        }
    }
    gave
}

/// What a frame's answer did to the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Landed {
    /// Nothing the panel shows.
    Elsewhere,
    /// The frame open in the editor took the person on its panel: the
    /// caller says the view changed, once it has let go of the state,
    /// and the panel's save records it.
    Open,
}

/// The person given on `frame`, where its line still holds `carried`:
/// on the panel when it is the frame open there, else as a step of its
/// line, labeled `label`, its sidecar written; where it took them, or
/// how it is counted when it did not. Nothing is touched or written
/// when it did not.
fn put(
    st: &mut State,
    app: &App,
    frame: &Frame,
    carried: &Shape,
    person: &Person,
    label: &str,
) -> Result<Landed, Count> {
    let c = st.find_cell(frame).ok_or(Count::Failed)?;
    let on_panel =
        st.current == Some(c) && st.culling.cull.is_none() && crate::rows::panel_is_frames(st, c);
    if on_panel {
        let mut edit = read_edit(app, &st.develop.edit, st.develop.target);
        if !give(&mut edit, carried, person) {
            return Err(Count::Changed);
        }
        // Its shapes that asked ask no more, as a click's pick has it.
        let given = with_person(carried, person);
        let keys: Vec<(u64, usize)> = edit
            .adjustments
            .iter()
            .flat_map(|a| {
                a.mask
                    .components
                    .iter()
                    .enumerate()
                    .filter(|(_, k)| k.shape == given)
                    .map(move |(i, _)| (a.id, i))
            })
            .collect();
        for key in &keys {
            st.develop.part_asks.remove(key);
        }
        if st
            .develop
            .choosing
            .as_ref()
            .is_some_and(|ch| ch.component.is_some_and(|i| keys.contains(&(ch.id, i))))
        {
            st.develop.choosing = None;
            app.set_placing("".into());
        }
        st.develop.edit = edit;
        show_edit(st, &st.develop.edit, app, st.develop.target);
        return Ok(Landed::Open);
    }
    let line = st.line_mut(c).ok_or(Count::Failed)?;
    let mut edit = line.current.clone();
    if !give(&mut edit, carried, person) {
        return Err(Count::Changed);
    }
    line.record_as(edit, Some(label.to_string()));
    if let Some(file) = st.file_at(c) {
        crate::panel::edit::write_file(st, file);
    }
    let (turns, flip) = thumb_turns(st, app, c);
    show_thumb(st, app, c, turns, flip);
    show_badges(st, app, c);
    Ok(Landed::Elsewhere)
}

/// A frame of run `batch` answered: the person given where found, the
/// answer counted, and the run's word said once the last is in.
pub(crate) fn landed(
    st: &mut State,
    app: &App,
    batch: u64,
    frame: Frame,
    found: Result<PersonOn, String>,
) -> Landed {
    let Some(b) = st.develop.people_all.batch.as_mut() else {
        return Landed::Elsewhere;
    };
    if b.id != batch || !b.waiting.remove(&frame) {
        return Landed::Elsewhere;
    }
    let (carried, label) = (b.carried.clone(), person_label(&b.from));
    let mut where_ = Landed::Elsewhere;
    let counted = match found {
        Ok(PersonOn::Found(person)) => match put(st, app, &frame, &carried, &person, &label) {
            Ok(at) => {
                where_ = at;
                Count::Used
            }
            Err(counted) => counted,
        },
        Ok(PersonOn::Settled) => Count::Settled,
        Ok(PersonOn::NotFound) => Count::NotFound,
        Err(why) => {
            tracing::warn!("the person picked, on {}: {why}", frame.said());
            Count::Failed
        }
    };
    let Some(b) = st.develop.people_all.batch.as_mut() else {
        return where_;
    };
    match counted {
        Count::Used => b.used += 1,
        Count::Settled => b.settled += 1,
        Count::NotFound => b.not_found += 1,
        Count::Changed => b.changed += 1,
        Count::Failed => b.failed += 1,
    }
    if b.waiting.is_empty() {
        let said = b.words();
        tracing::info!("{said}");
        st.develop.people_all.batch = None;
        app.set_status(said.into());
    } else {
        let looked = b.count - 1 - b.waiting.len();
        app.set_status(
            format!(
                "looking for this person: {looked} of {} pictures looked at",
                b.count - 1
            )
            .into(),
        );
    }
    where_
}

/// How a frame's answer is counted.
enum Count {
    Used,
    Settled,
    NotFound,
    Changed,
    Failed,
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_people_offer_used(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            used(&mut state.borrow_mut(), &app, &worker);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_people_offer_dismissed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            dismissed(&mut state.borrow_mut(), &app);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{Ask, Asking, Candidate};
    use crate::panel::viewport::source_to_view;
    use crate::testing::{folder, state_for, window};
    use greycard_edit::mask::{Route, Signature};
    use greycard_edit::preset::Section;

    /// The woman a lips mask was made for on the first frame.
    fn her() -> Person {
        Person {
            signature: Signature {
                kind: "colors-1".into(),
                values: vec![7.0; 22],
            },
            at: [0.9, 0.9],
            picture: Some("the first frame".into()),
        }
    }

    /// Her lips mask, as it came to the other frames.
    fn carried() -> Shape {
        Shape::Part {
            phrase: "lips".into(),
            route: Route::Mouth,
            person: Some(her()),
        }
    }

    /// Two people side by side on frame `n`, as the People model finds
    /// them there, each known by a signature of their own.
    fn two(n: usize) -> Vec<Candidate> {
        [[0.2, 0.1, 0.3, 0.25], [0.6, 0.1, 0.7, 0.25]]
            .into_iter()
            .enumerate()
            .map(|(i, face)| Candidate {
                face,
                at: [(face[0] + face[2]) / 2.0, (face[1] + face[3]) / 2.0],
                signature: Signature {
                    kind: "colors-1".into(),
                    values: vec![(10 * n + i) as f32; 22],
                },
                body: None,
                picture: Some(format!("frame {n}")),
            })
            .collect()
    }

    /// The frames `files`, the first with her lips mask, synced onto the
    /// `onto` after it; with `write` the sidecars go to disk.
    fn synced_over(
        files: Vec<PathBuf>,
        onto: usize,
        write: bool,
    ) -> (App, Rc<RefCell<State>>, Rc<Worker>) {
        let app = window(files.len());
        let (state, worker) = state_for(&app, files);
        {
            let mut st = state.borrow_mut();
            if write {
                st.window.write_sidecars = true;
                st.window.placement = greycard_edit::Placement::Folder;
            }
            let mut edit = Edit::default();
            let mut mask = Mask::default();
            mask.components.push(Component {
                shape: carried(),
                ..Default::default()
            });
            edit.adjustments.push(greycard_edit::Adjustment {
                id: 1,
                name: "Lips 1".into(),
                enabled: true,
                mask,
                look: Default::default(),
            });
            st.sidecars[0].record(edit);
        }
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            st.views.picked = (0..=onto).collect();
            let synced = crate::panel::sync::sync_selection(
                &mut st,
                &app,
                &[Section::Adjustments],
                &[],
                |_, _| None,
            );
            assert_eq!(synced.moved, (1..=onto).collect::<Vec<_>>());
        }
        (app, state, worker)
    }

    /// `frames` frames nothing writes, as [`synced_over`].
    fn synced(frames: usize, onto: usize) -> (App, Rc<RefCell<State>>, Rc<Worker>) {
        synced_over(folder(frames), onto, false)
    }

    /// Frame `c` opened on the Masks tab, its lips asking which of the
    /// two there she is, as the worker says it.
    fn asking_on(app: &App, state: &Rc<RefCell<State>>, c: usize) {
        app.invoke_select(c as i32);
        let file = state.borrow().files[c].clone();
        app.set_panel_tab("Develop".into());
        app.invoke_tab_changed("Develop".into());
        crate::panel::deliver::deliver(
            app,
            Outcome::Mask {
                key: (1, 0),
                shape: carried(),
                file: Some(file),
                raster: Arc::new(Raster::from_data(1.0, 4, vec![0; 16])),
                provider: Some("CPU"),
                seconds: 0.1,
                note: Some("which person? click one, or All people".into()),
                part: Some(crate::ai::Unresolved::Ask(Ask {
                    people: two(c),
                    guess: None,
                    why: Asking::Unsure,
                })),
            },
        );
        app.set_panel_tab("Masks".into());
        app.invoke_tab_changed("Masks".into());
        state.borrow_mut().develop.source_size = (1200, 800);
        assert_eq!(app.get_placing(), crate::panel::parts::PICKING);
    }

    /// The second person on frame `c` clicked.
    fn pick_second(app: &App, state: &Rc<RefCell<State>>, c: usize) {
        let mut st = state.borrow_mut();
        let p = &two(c)[1];
        let (x, y) = source_to_view(&st, app, p.at[0], p.at[1]);
        assert_eq!(crate::panel::parts::pressed(&mut st, app, x, y), Some(true));
    }

    /// The person of the lips in `edit`.
    fn person_in(edit: &Edit) -> Option<Person> {
        match &edit.adjustments[0].mask.components[0].shape {
            Shape::Part { person, .. } => person.clone(),
            _ => None,
        }
    }

    /// The person of the lips on cell `c`'s line.
    fn person_on(st: &State, c: usize) -> Option<Person> {
        person_in(&st.line(c)?.current)
    }

    fn answer(app: &App, state: &Rc<RefCell<State>>, c: usize, found: PersonOn) {
        let (batch, frame) = {
            let st = state.borrow();
            (
                st.develop.people_all.batch.as_ref().expect("a run").id,
                st.key_of(c).unwrap(),
            )
        };
        crate::panel::deliver::deliver(
            app,
            Outcome::PersonOn {
                batch,
                frame,
                found: Ok(found),
            },
        );
    }

    /// The panel's save, once its timer has run.
    fn saved() {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ));
        slint::platform::update_timers_and_animations();
    }

    /// Synced onto three frames, a person picked on one offers her for
    /// all three, under the mask she was picked in; taken, the two
    /// others are looked at, each given her as found there, one step of
    /// its own history named for the frame picked on, and the status
    /// line says how it went.
    #[test]
    fn a_pick_after_a_sync_onto_three_is_offered_for_all_and_used() {
        let (app, state, _worker) = synced(4, 3);
        asking_on(&app, &state, 1);
        assert_eq!(app.get_people_offer(), "", "nothing before the pick");
        pick_second(&app, &state, 1);
        assert_eq!(app.get_people_offer(), "Use this person on all 3 pictures");
        assert_eq!(app.get_people_offer_mask(), 1, "under the first mask");
        let picked = two(1)[1].person();
        app.invoke_people_offer_used();
        assert_eq!(app.get_people_offer(), "", "the offer goes once taken");
        assert_eq!(
            app.get_status(),
            "looking for this person on 2 more pictures"
        );
        let on_two = two(2)[1].person();
        answer(&app, &state, 2, PersonOn::Found(on_two.clone()));
        assert_eq!(
            app.get_status(),
            "looking for this person: 1 of 2 pictures looked at"
        );
        let on_three = two(3)[1].person();
        answer(&app, &state, 3, PersonOn::Found(on_three.clone()));
        assert_eq!(app.get_status(), "used this person on 3 of 3 pictures");
        // A second press has no offer to take: nothing is sent again.
        app.invoke_people_offer_used();
        assert_eq!(app.get_status(), "used this person on 3 of 3 pictures");
        let st = state.borrow();
        assert_eq!(person_on(&st, 2), Some(on_two));
        assert_eq!(person_on(&st, 3), Some(on_three));
        for c in [2, 3] {
            assert_eq!(
                st.line(c).unwrap().current_label.as_deref(),
                Some("Person from IMG_0001.CR3")
            );
        }
        // The frame picked on has its own pick; the frame synced from
        // keeps hers.
        assert_eq!(person_in(&st.develop.edit), Some(picked));
        assert_eq!(person_on(&st, 0), Some(her()));
    }

    /// Where she is not found the frame is left asking, and counted;
    /// one whose own person was decided already is left as it is; one
    /// open in the editor when its answer lands takes her on the panel;
    /// an answer for a run let go is let go.
    #[test]
    fn frames_she_is_not_found_on_are_left_waiting_and_counted() {
        let (app, state, _worker) = synced(5, 4);
        asking_on(&app, &state, 1);
        pick_second(&app, &state, 1);
        assert_eq!(app.get_people_offer(), "Use this person on all 4 pictures");
        app.invoke_people_offer_used();
        let stale = {
            let st = state.borrow();
            Outcome::PersonOn {
                batch: st.develop.people_all.batch.as_ref().unwrap().id + 1,
                frame: st.key_of(2).unwrap(),
                found: Ok(PersonOn::Found(two(2)[0].person())),
            }
        };
        crate::panel::deliver::deliver(&app, stale);
        assert_eq!(person_on(&state.borrow(), 2), Some(her()));
        answer(&app, &state, 2, PersonOn::NotFound);
        answer(&app, &state, 3, PersonOn::Settled);
        // Frame 4 open now: its answer is the panel's.
        app.invoke_select(4);
        answer(&app, &state, 4, PersonOn::Found(two(4)[1].person()));
        assert_eq!(
            app.get_status(),
            "used this person on 2 of 4 pictures; 1 kept its own person; 1 not found"
        );
        let st = state.borrow();
        assert_eq!(person_in(&st.develop.edit), Some(two(4)[1].person()));
        // Not found and decided already: as the sync left them, no step.
        for c in [2, 3] {
            assert_eq!(person_on(&st, c), Some(her()));
            assert_eq!(
                st.line(c).unwrap().current_label.as_deref(),
                Some("Sync from IMG_0000.CR3")
            );
        }
    }

    /// The run's safety: a frame picked by hand between the press and
    /// its answer is left alone, its own pick and its other edits as
    /// they were and nothing written; a frame only edited otherwise
    /// takes the person over what it has now, its edits kept.
    #[test]
    fn a_frame_changed_since_the_press_is_left_alone() {
        let dir = crate::testing::scratch_dir("people-all-changed");
        let files: Vec<PathBuf> = (0..4)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let (app, state, _worker) = synced_over(files.clone(), 3, true);
        asking_on(&app, &state, 1);
        pick_second(&app, &state, 1);
        app.invoke_people_offer_used();
        let on_disk = |i: usize| {
            std::fs::read(greycard_edit::Sidecar::path_in(
                &files[i],
                greycard_edit::Placement::Folder,
            ))
            .unwrap()
        };
        let before = on_disk(2);
        let mine = two(2)[0].person();
        let steps = {
            let mut st = state.borrow_mut();
            // Frame 2: the other person picked by hand, and brighter.
            let mut edit = st.line(2).unwrap().current.clone();
            give(&mut edit, &carried(), &mine);
            edit.light.exposure = 0.5;
            st.line_mut(2).unwrap().record(edit);
            // Frame 3: only brighter.
            let mut edit = st.line(3).unwrap().current.clone();
            edit.light.exposure = 0.7;
            st.line_mut(3).unwrap().record(edit);
            st.line(2).unwrap().history.len()
        };
        answer(&app, &state, 2, PersonOn::Found(two(2)[1].person()));
        answer(&app, &state, 3, PersonOn::Found(two(3)[1].person()));
        assert_eq!(
            app.get_status(),
            "used this person on 2 of 3 pictures; 1 changed since"
        );
        {
            let st = state.borrow();
            let line = st.line(2).unwrap();
            assert_eq!(person_in(&line.current), Some(mine));
            assert_eq!(line.current.light.exposure, 0.5);
            assert_eq!(line.history.len(), steps, "no step");
            assert_eq!(on_disk(2), before, "nothing written");
            let line = st.line(3).unwrap();
            assert_eq!(person_in(&line.current), Some(two(3)[1].person()));
            assert_eq!(line.current.light.exposure, 0.7);
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// The open frame's answer while its own ask is armed: the picking
    /// is put down, and the panel's save records the person as a step
    /// and writes the sidecar.
    #[test]
    fn the_open_frames_answer_puts_its_ask_down_and_is_saved() {
        let dir = crate::testing::scratch_dir("people-all-open");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let (app, state, _worker) = synced_over(files.clone(), 2, true);
        asking_on(&app, &state, 1);
        pick_second(&app, &state, 1);
        app.invoke_people_offer_used();
        asking_on(&app, &state, 2);
        let steps = state.borrow().line(2).unwrap().history.len();
        let hers = two(2)[1].person();
        answer(&app, &state, 2, PersonOn::Found(hers.clone()));
        {
            let st = state.borrow();
            assert!(st.develop.choosing.is_none());
            assert!(st.develop.part_asks.is_empty());
            assert_eq!(person_in(&st.develop.edit), Some(hers.clone()));
        }
        assert_eq!(app.get_placing(), "");
        assert_eq!(app.get_status(), "used this person on 2 of 2 pictures");
        saved();
        let st = state.borrow();
        let line = st.line(2).unwrap();
        assert_eq!(line.history.len(), steps + 1);
        assert_eq!(person_in(&line.current), Some(hers.clone()));
        let back = greycard_edit::Sidecar::load(&files[2]).unwrap().unwrap();
        assert_eq!(person_in(&back.current), Some(hers));
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A paste onto many is a sync with the clipboard between: a pick
    /// on one of the frames it reached is offered for the rest.
    #[test]
    fn a_paste_onto_three_offers_it_too() {
        let (app, state, worker) = synced(4, 0);
        {
            let mut st = state.borrow_mut();
            crate::panel::sync::copy_settings(&mut st, &app);
            crate::panel::sync::paste_onto(
                &mut st,
                &app,
                &worker,
                &[1, 2, 3],
                &greycard_edit::preset::Choice::whole(&[Section::Adjustments]),
                &[],
                |_, _| None,
            );
            assert_eq!(person_on(&st, 3), Some(her()));
        }
        asking_on(&app, &state, 1);
        pick_second(&app, &state, 1);
        assert_eq!(app.get_people_offer(), "Use this person on all 3 pictures");
        // A sync after it of no masks leaves those frames as they were.
        {
            let mut st = state.borrow_mut();
            st.views.picked = vec![1, 2, 3];
            crate::panel::sync::sync_selection(&mut st, &app, &[Section::Light], &[], |_, _| None);
        }
        asking_on(&app, &state, 2);
        pick_second(&app, &state, 2);
        assert_eq!(app.get_people_offer(), "Use this person on all 2 pictures");
    }

    /// A sync onto one frame offers nothing, nor does All people, nor a
    /// pick on a shape that came some other way.
    #[test]
    fn no_offer_for_one_frame_or_for_all_people() {
        let (app, state, _worker) = synced(2, 1);
        asking_on(&app, &state, 1);
        pick_second(&app, &state, 1);
        assert_eq!(app.get_people_offer(), "");

        let (app, state, _worker) = synced(3, 2);
        asking_on(&app, &state, 1);
        assert!(crate::panel::parts::all_people(
            &mut state.borrow_mut(),
            &app
        ));
        assert_eq!(app.get_people_offer(), "");
        // The frame synced from: its shape is the one carried, but it
        // was never reached.
        asking_on(&app, &state, 0);
        pick_second(&app, &state, 0);
        assert_eq!(app.get_people_offer(), "");
    }

    /// The offer put away with the cross is not made again for that
    /// sync, on this frame or another; it goes with the frame it was
    /// made on, too.
    #[test]
    fn the_offer_dismisses_and_goes_with_its_frame() {
        let (app, state, _worker) = synced(4, 3);
        asking_on(&app, &state, 1);
        pick_second(&app, &state, 1);
        assert_ne!(app.get_people_offer(), "");
        // Another frame: the offer was this one's.
        app.invoke_select(2);
        assert_eq!(app.get_people_offer(), "");
        // Picked on there too, the first frame has its own person now:
        // the offer is for the frames that hold the mask as it came.
        asking_on(&app, &state, 2);
        pick_second(&app, &state, 2);
        assert_eq!(app.get_people_offer(), "Use this person on all 2 pictures");
        app.invoke_people_offer_dismissed();
        assert_eq!(app.get_people_offer(), "");
        let said = app.get_status();
        app.invoke_people_offer_used();
        assert_eq!(app.get_status(), said, "nothing to take");
        asking_on(&app, &state, 3);
        pick_second(&app, &state, 3);
        assert_eq!(app.get_people_offer(), "", "not for this sync again");
    }

    /// The offer goes with the pick: undone, it is gone and stays gone
    /// after a redo; and a shape that no longer has the person picked
    /// shows no offer, and a press on one let stand says so and sends
    /// nothing.
    #[test]
    fn the_offer_goes_when_the_pick_is_undone_or_the_shape_changes() {
        let (app, state, _worker) = synced(4, 3);
        asking_on(&app, &state, 1);
        pick_second(&app, &state, 1);
        assert_ne!(app.get_people_offer(), "");
        app.invoke_undo();
        assert_eq!(person_in(&state.borrow().develop.edit), Some(her()));
        assert_eq!(app.get_people_offer(), "");
        app.invoke_redo();
        assert_eq!(
            person_in(&state.borrow().develop.edit),
            Some(two(1)[1].person())
        );
        assert_eq!(app.get_people_offer(), "");

        asking_on(&app, &state, 2);
        pick_second(&app, &state, 2);
        assert_ne!(app.get_people_offer(), "");
        {
            let mut st = state.borrow_mut();
            let mut edit = st.develop.edit.clone();
            give(
                &mut edit,
                &with_person(&carried(), &two(2)[1].person()),
                &her(),
            );
            st.develop.edit = edit;
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
        }
        assert_eq!(app.get_people_offer(), "", "hidden");
        app.invoke_people_offer_used();
        assert_eq!(
            app.get_status(),
            "this mask no longer has the person picked"
        );
        assert!(state.borrow().develop.people_all.batch.is_none());
    }

    /// Each frame's step is its own: opened, an undo takes the person
    /// back to the one the sync brought, and leaves the others alone.
    #[test]
    fn each_frame_undoes_its_own_person() {
        let (app, state, _worker) = synced(4, 3);
        asking_on(&app, &state, 1);
        pick_second(&app, &state, 1);
        app.invoke_people_offer_used();
        answer(&app, &state, 2, PersonOn::Found(two(2)[1].person()));
        answer(&app, &state, 3, PersonOn::Found(two(3)[1].person()));
        app.invoke_select(2);
        app.invoke_undo();
        let st = state.borrow();
        assert_eq!(person_on(&st, 2), Some(her()));
        assert_eq!(person_in(&st.develop.edit), Some(her()));
        assert_eq!(person_on(&st, 3), Some(two(3)[1].person()));
    }
}
