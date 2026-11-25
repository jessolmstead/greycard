//! The whole list of a menu the window cut (`MenuFit`), as a sheet:
//! Edit in's All apps..., and All archives... on the frame menu, the
//! grid header's archive button, and its rejects menu. The window hands
//! over the menu's list as the menu showed it, and we hold a copy: the
//! live lists are refilled as views, counts, and the index land, and an
//! archive menu's is empty in a view with no side. One held model is
//! refilled at each key with the names that hold what is typed, case
//! aside, each with its place in the copy as its id. A pick is that
//! name's place in the live list, so it goes where the menu item of
//! that name now goes, or nowhere when the name is gone.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{Model, ModelRc, SharedString, VecModel};

use crate::{App, CollectionChoice};

/// The names of `list` that hold `typed`, case aside, in its order,
/// each by its place.
fn narrow(list: &[SharedString], typed: &str) -> Vec<CollectionChoice> {
    let typed = typed.trim().to_lowercase();
    list.iter()
        .enumerate()
        .filter(|(_, name)| typed.is_empty() || name.to_lowercase().contains(&typed))
        .map(|(i, name)| CollectionChoice {
            name: name.clone(),
            id: i.to_string().into(),
        })
        .collect()
}

/// Where the name at `held` of the held list is in `live`: the same
/// place while it still holds that name, else its first place, and
/// `None` when it is gone.
fn place(held: &[SharedString], live: &ModelRc<SharedString>, at: usize) -> Option<usize> {
    let name = held.get(at)?;
    if live.row_data(at).as_ref() == Some(name) {
        return Some(at);
    }
    live.iter().position(|n| n == *name)
}

pub(crate) fn install(app: &App) {
    let found: Rc<VecModel<CollectionChoice>> = Rc::default();
    let held: Rc<RefCell<Vec<SharedString>>> = Rc::default();
    app.set_choice_found(ModelRc::from(found.clone()));
    {
        let held = held.clone();
        app.on_choice_hold(move |list| *held.borrow_mut() = list.iter().collect());
    }
    {
        let held = held.clone();
        app.on_choice_search(move |typed| found.set_vec(narrow(&held.borrow(), &typed)));
    }
    app.on_choice_place(move |live, at| {
        let at = usize::try_from(at).ok();
        at.and_then(|at| place(&held.borrow(), &live, at))
            .map_or(-1, |i| i as i32)
    });
}

#[cfg(test)]
mod tests;
