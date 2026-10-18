//! The filter bar's Sort menu: what it says, and a choice taken from
//! it into the view, the settings and the list (`crate::sort`).

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::roots::View;
use crate::sort::{Choice, button_words, entries, picked, turned, way_words};
use crate::worker::Worker;
use crate::{App, SortMenu, State};

/// The menu as the view's choice has it.
pub(crate) fn menu_of(st: &State) -> SortMenu {
    let choice = st.views.sort.choice(&st.views.view);
    let listed = entries(&st.views.view);
    let checked = listed
        .iter()
        .position(|e| *e == choice.order().map(|o| o.by))
        .unwrap_or(0);
    let words: Vec<SharedString> = listed
        .iter()
        .map(|e| match e {
            None => SharedString::from("Hand order"),
            Some(by) => SharedString::from(by.words()),
        })
        .collect();
    let (ways, descending): (Vec<SharedString>, bool) = match choice {
        Choice::Hand => (Vec::new(), false),
        Choice::By(o) => (
            way_words(o.by)
                .into_iter()
                .map(SharedString::from)
                .collect(),
            o.descending,
        ),
    };
    let tip = match (choice, ways.get(usize::from(descending))) {
        // "oldest first" in the middle of the line, "A to Z" as it is.
        (Choice::By(_), Some(way)) => {
            let way = if way.chars().nth(1) == Some(' ') {
                way.to_string()
            } else {
                way.to_lowercase()
            };
            format!("Sort: {}, {way}", button_words(choice))
        }
        _ => format!("Sort: {}", button_words(choice)),
    };
    SortMenu {
        words: button_words(choice).into(),
        label: button_words(choice).to_lowercase().into(),
        tip: tip.into(),
        entries: ModelRc::new(VecModel::from(words)),
        checked: checked as i32,
        ways: ModelRc::new(VecModel::from(ways)),
        descending,
    }
}

/// The menu put on the window, when it has changed: called wherever
/// the browser's rows are made again, which every new view and every
/// choice reaches.
pub(crate) fn show(st: &State, app: &App) {
    let menu = menu_of(st);
    let now = app.get_sort_menu();
    let same = now.words == menu.words
        && now.tip == menu.tip
        && now.label == menu.label
        && now.checked == menu.checked
        && now.descending == menu.descending
        && slint::Model::iter(&now.entries).eq(slint::Model::iter(&menu.entries))
        && slint::Model::iter(&now.ways).eq(slint::Model::iter(&menu.ways));
    if !same {
        app.set_sort_menu(menu);
    }
}

/// `choice` taken for `view`: kept for it, in the settings at once (a
/// session that never closes cleanly keeps it), and the list read again
/// in it when it is the view on screen, which keeps the frame on screen
/// chosen and brings it into view wherever it went; a view not on
/// screen (a collection's row asked for its capture order) is opened in
/// it.
pub(crate) fn choose(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    view: &View,
    choice: Choice,
) {
    let on_screen = {
        let mut st = state.borrow_mut();
        let was = st.views.sort.choice(view);
        st.views.sort.set(view, choice);
        let set = (view.clone(), choice);
        crate::panel::prefs::keep(&st, move |s| s.sort.set(&set.0, set.1));
        show(&st, app);
        crate::panel::collections::show(&st, app);
        if was != choice {
            tracing::info!("sort: {:?} for {:?}", choice, crate::sort::Slot::of(view));
        }
        let on_screen = st.views.view == *view;
        if on_screen && was == choice {
            return;
        }
        on_screen
    };
    if on_screen {
        state.borrow_mut().views.sort_reveal = true;
        if !crate::roots::refresh_view(state, app, worker) {
            let mut st = state.borrow_mut();
            // A view still being opened is read again in the choice as
            // it lands (`roots::landing`), the reveal with it.
            if !st.views.library.loading {
                // Nothing to read the list again from (files the
                // desktop handed over from several folders): the choice
                // is kept, and the next read of the list is in it.
                st.views.sort_reveal = false;
                app.set_status(
                    "the order is kept; it applies the next time the list is read".into(),
                );
            }
        }
    } else {
        crate::roots::open_view(state, app, worker, view.clone());
    }
}

/// A merge read landed: the frame on screen brought into view, when the
/// read was for a sort. The state is not held across the reveal, which
/// tells the window's ranges to the callbacks that read it.
pub(crate) fn landed(state: &Rc<RefCell<State>>, app: &App) {
    let reveal = std::mem::take(&mut state.borrow_mut().views.sort_reveal);
    if reveal {
        app.invoke_reveal_selected();
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    show(&state.borrow(), app);
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_sort_picked(move |entry| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (view, choice) = {
                let st = state.borrow();
                let now = st.views.sort.choice(&st.views.view);
                let Some(choice) = usize::try_from(entry)
                    .ok()
                    .and_then(|e| picked(&st.views.view, now, e))
                else {
                    return;
                };
                (st.views.view.clone(), choice)
            };
            choose(&state, &app, &worker, &view, choice);
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_sort_turned(move |descending| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (view, choice) = {
                let st = state.borrow();
                (
                    st.views.view.clone(),
                    turned(st.views.sort.choice(&st.views.view), descending),
                )
            };
            choose(&state, &app, &worker, &view, choice);
        });
    }
}
