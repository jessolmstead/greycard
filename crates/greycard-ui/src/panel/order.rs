//! The panel's sections in the order the user drags them into.
//!
//! Each view's sections are named here, with their first order: the
//! panel (`SectionStack` in app.slint) lays out whatever order it is
//! given, by these names, and a header dragged there names the move
//! back. The order is the panel's alone: the develop runs its ops in
//! its own order, and the edit, the sidecars, the presets, and the
//! exports never see this one.
//!
//! The Masks tab shares Develop's order. The six sections a mask
//! edits too (Light, Color, Curves, Mixer, Grading, and Tint) are one
//! instance on both tabs and keep one place; MASK is the Masks tab's
//! alone, and a mask's own white balance and Detail stand where
//! Develop's do. A section the tab on show does not have takes no room
//! there, so each tab sees its own sections in the shared order.
//!
//! Library and the Loupe share the left pane's one model too, so that
//! FOLDERS, theirs both, is one instance across a switch between them,
//! as it always was: the model holds both views' sections, each view's
//! in its own order and FOLDERS once (`pane_order`), and each slot's
//! `if` shows only the view's own. Each view's order is still its own.
//!
//! The settings keep a view's order only once it differs from the
//! first (`read`), so a section a later version adds lands where that
//! version puts it for everyone who never dragged that view, and after
//! its first-order neighbor for everyone who did (`reconcile`).

use std::collections::BTreeMap;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::App;

/// A view of the panel whose sections are ordered: its name, as the
/// settings and the panel's `section-moved` call it, its sections in
/// their first order, the pane whose model holds them, and the panel's
/// property for that model.
pub(crate) struct View {
    pub(crate) name: &'static str,
    pub(crate) first: &'static [&'static str],
    pane: &'static str,
    get: fn(&App) -> ModelRc<SharedString>,
    set: fn(&App, ModelRc<SharedString>),
}

pub(crate) const VIEWS: &[View] = &[
    View {
        name: "library",
        first: &["roots", "folders", "collections", "queue", "metadata"],
        pane: "left",
        get: App::get_left_order,
        set: App::set_left_order,
    },
    View {
        name: "loupe",
        first: &["folders", "presets", "snapshots", "history"],
        pane: "left",
        get: App::get_left_order,
        set: App::set_left_order,
    },
    View {
        name: "cull",
        first: &["culling", "metadata"],
        pane: "cull",
        get: App::get_cull_order,
        set: App::set_cull_order,
    },
    // Develop's and the Masks tab's: MASK first, which only the Masks
    // tab shows, then Develop's in the order a photo is developed.
    View {
        name: "develop",
        first: &[
            "mask",
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
            "noise",
            "lens",
            "detail",
            "sharpen",
            "output-sharpen",
            "vignette",
            "grain",
            "proof",
            "monitor",
            "demosaic",
            "metadata",
        ],
        pane: "develop",
        get: App::get_develop_order,
        set: App::set_develop_order,
    },
    View {
        name: "crop",
        first: &["crop", "rotate", "perspective"],
        pane: "crop",
        get: App::get_crop_order,
        set: App::set_crop_order,
    },
    View {
        name: "retouch",
        first: &["retouch"],
        pane: "retouch",
        get: App::get_retouch_order,
        set: App::set_retouch_order,
    },
];

fn view(name: &str) -> Option<&'static View> {
    VIEWS.iter().find(|v| v.name == name)
}

/// A saved order made whole against the first: the names it keeps that
/// the view still has, once each and in its order; and a name it does
/// not have, a section new since it was saved, put after its nearest
/// neighbor above it in the first order that is there, or first when
/// none above it is.
pub(crate) fn reconcile(first: &[&str], saved: &[String]) -> Vec<String> {
    let mut order: Vec<String> = Vec::with_capacity(first.len());
    for name in saved {
        if first.contains(&name.as_str()) && !order.contains(name) {
            order.push(name.clone());
        }
    }
    for (at, name) in first.iter().enumerate() {
        if order.iter().any(|n| n == name) {
            continue;
        }
        let after = first[..at]
            .iter()
            .rev()
            .find_map(|above| order.iter().position(|n| n == above));
        order.insert(after.map_or(0, |p| p + 1), name.to_string());
    }
    order
}

/// Where the section at `from` lands when dropped before the one at
/// `slot` (`len` for the end), once it is taken out: none for a drop
/// that moves nothing (onto itself, or just under itself) or that
/// names no place in an order `len` long.
pub(crate) fn destination(len: usize, from: usize, slot: usize) -> Option<usize> {
    if from >= len || slot > len || slot == from || slot == from + 1 {
        return None;
    }
    Some(if slot > from { slot - 1 } else { slot })
}

fn names(model: &ModelRc<SharedString>) -> Vec<String> {
    model.iter().map(|n| n.to_string()).collect()
}

fn pane(name: &str) -> impl Iterator<Item = &'static View> + '_ {
    VIEWS.iter().filter(move |v| v.pane == name)
}

/// A view's order as the panel holds it: its own names in its pane's
/// model, in their order there.
pub(crate) fn order_now(app: &App, view: &View) -> Vec<String> {
    names(&(view.get)(app))
        .into_iter()
        .filter(|n| view.first.contains(&n.as_str()))
        .collect()
}

/// A pane's model from its views' orders. One view's is its order; two
/// that share a section (Library's and the Loupe's, FOLDERS) are each
/// split there, and the model is what each has above it, the shared
/// one, then what each has under it: either view's order is the model
/// with the other's own sections left out.
pub(crate) fn pane_order(orders: &[Vec<String>]) -> Vec<String> {
    match orders {
        [one] => one.clone(),
        [a, b] => {
            let shared = a.iter().position(|n| b.contains(n));
            match shared {
                Some(at) => {
                    let name = &a[at];
                    let bt = b.iter().position(|n| n == name).unwrap_or(0);
                    let mut out = a[..at].to_vec();
                    out.extend_from_slice(&b[..bt]);
                    out.push(name.clone());
                    out.extend_from_slice(&a[at + 1..]);
                    out.extend_from_slice(&b[bt + 1..]);
                    out
                }
                None => a.iter().chain(b).cloned().collect(),
            }
        }
        _ => orders.concat(),
    }
}

/// Of the rows `from` holds, those that keep their places among each
/// other in `to`: the longest run of them in the same order in both,
/// so that the fewest rows move.
pub(crate) fn kept_rows(from: &[String], to: &[String]) -> Vec<String> {
    // Each of `to`'s rows by its place in `from`; the longest increasing
    // run of those places is what stays.
    let at: Vec<(usize, &String)> = to
        .iter()
        .filter_map(|n| from.iter().position(|f| f == n).map(|p| (p, n)))
        .collect();
    let mut best = vec![1usize; at.len()];
    let mut back = vec![usize::MAX; at.len()];
    for i in 0..at.len() {
        for j in 0..i {
            if at[j].0 < at[i].0 && best[j] + 1 > best[i] {
                best[i] = best[j] + 1;
                back[i] = j;
            }
        }
    }
    let mut kept = Vec::new();
    let mut i = (0..at.len()).max_by_key(|&i| best[i]);
    while let Some(k) = i {
        kept.push(at[k].1.clone());
        i = (back[k] != usize::MAX).then_some(back[k]);
    }
    kept.reverse();
    kept
}

/// `rows` made `to` with the fewest rows moved (`kept_rows`): the
/// others taken out and put back where `to` has them, so every row
/// that keeps its place among the rest keeps its section, made as it
/// was. A move down by one row is one row out and one in, as a move up.
/// Where moving `dragged` alone would do, it is the one moved, never a
/// neighbor it passed (FOLDERS, say, which a section crossing it could
/// as well be said to have crossed).
fn morph(rows: &VecModel<SharedString>, to: &[String], dragged: Option<&str>) {
    let now: Vec<String> = rows.iter().map(|n| n.to_string()).collect();
    let others = |list: &[String]| -> Vec<String> {
        list.iter()
            .filter(|n| Some(n.as_str()) != dragged)
            .cloned()
            .collect()
    };
    let alone = kept_rows(&others(&now), &others(to));
    let kept = if dragged.is_some() && alone.len() + 1 >= to.len() {
        alone
    } else {
        kept_rows(&now, to)
    };
    for i in (0..now.len()).rev() {
        if !kept.contains(&now[i]) {
            rows.remove(i);
        }
    }
    for (i, name) in to.iter().enumerate() {
        if !kept.contains(name) {
            rows.insert(i, name.into());
        }
    }
}

/// `view`'s order put in its pane's model with the pane's other views'
/// as they are; `fresh`, the model replaced whole.
fn put(app: &App, view: &View, order: &[String], fresh: bool, dragged: Option<&str>) {
    let orders: Vec<Vec<String>> = pane(view.pane)
        .map(|v| {
            if v.name == view.name {
                order.to_vec()
            } else {
                order_now(app, v)
            }
        })
        .collect();
    let model = pane_order(&orders);
    let held = (view.get)(app);
    match held.as_any().downcast_ref::<VecModel<SharedString>>() {
        Some(rows) if !fresh => morph(rows, &model, dragged),
        _ => {
            let model: Vec<SharedString> = model.iter().map(SharedString::from).collect();
            (view.set)(app, ModelRc::new(VecModel::from(model)));
        }
    }
}

/// Each view's order, from the settings: a view saved made whole by
/// `reconcile`, one not saved its first order.
pub(crate) fn show(app: &App, saved: &BTreeMap<String, Vec<String>>) {
    let mut panes: Vec<&str> = VIEWS.iter().map(|v| v.pane).collect();
    panes.dedup();
    for name in panes {
        let views: Vec<&View> = pane(name).collect();
        let orders: Vec<Vec<String>> = views
            .iter()
            .map(|v| {
                reconcile(
                    v.first,
                    saved.get(v.name).map(Vec::as_slice).unwrap_or_default(),
                )
            })
            .collect();
        let model: Vec<SharedString> = pane_order(&orders).iter().map(SharedString::from).collect();
        (views[0].set)(app, ModelRc::new(VecModel::from(model)));
    }
    app.set_section_order_moved(!read(app).is_empty());
}

/// The orders to keep: only a view's that is not its first, so a
/// file whose sections were never moved says nothing of them.
pub(crate) fn read(app: &App) -> BTreeMap<String, Vec<String>> {
    VIEWS
        .iter()
        .map(|view| (view.name.to_string(), order_now(app, view)))
        .filter(|(name, order)| view(name).is_some_and(|v| *order != v.first))
        .collect()
}

/// The orders as the panel holds them, written as they change.
fn keep(app: &App) {
    let orders = read(app);
    app.set_section_order_moved(!orders.is_empty());
    crate::settings::Settings::change(|settings| settings.section_orders = orders);
}

/// The section at `from` in the model `name`'s pane shows moved to
/// before the one at `slot` (the model's length for the end): rows
/// moved, not the model made anew, so the stack's other sections stay
/// as they are made, with their state. `next` is the first slot on show
/// under `from` (the model's length for none): a drop there is just
/// under itself on screen, whatever hidden sections lie between, and
/// moves nothing.
pub(crate) fn moved(app: &App, name: &str, from: usize, slot: usize, next: usize) -> bool {
    if slot == next || slot == from {
        return false;
    }
    let Some(view) = view(name) else {
        return false;
    };
    let model = names(&(view.get)(app));
    let order = order_now(app, view);
    // In the view's own order: the section dragged, and the one it was
    // dropped on, one of the view's, since only those are on show.
    let Some(from) = model
        .get(from)
        .and_then(|n| order.iter().position(|o| o == n))
    else {
        return false;
    };
    let slot = if slot >= model.len() {
        order.len()
    } else {
        match order.iter().position(|o| *o == model[slot]) {
            Some(at) => at,
            None => return false,
        }
    };
    let Some(to) = destination(order.len(), from, slot) else {
        return false;
    };
    let mut order = order;
    let section = order.remove(from);
    order.insert(to, section.clone());
    put(app, view, &order, false, Some(&section));
    // The moved section is made anew, a turn slider in it and its
    // timer with it: the grid it laid on is let go here, since moved
    // rows set no new model for the window to see.
    if view.pane == "crop" {
        app.invoke_turn_sliders_gone();
    }
    keep(app);
    true
}

/// Every view back in its first order.
pub(crate) fn reset(app: &App) {
    for view in VIEWS {
        if order_now(app, view) != view.first {
            let first: Vec<String> = view.first.iter().map(|n| n.to_string()).collect();
            put(app, view, &first, true, None);
        }
    }
    keep(app);
}

pub(crate) fn install(app: &App) {
    let weak = app.as_weak();
    app.on_section_moved(move |view, from, slot, next| {
        let Some(app) = weak.upgrade() else { return };
        if from >= 0 && slot >= 0 && next >= 0 {
            moved(&app, &view, from as usize, slot as usize, next as usize);
        }
    });
    let weak = app.as_weak();
    app.on_section_order_reset(move || {
        if let Some(app) = weak.upgrade() {
            reset(&app);
        }
    });
}

#[cfg(test)]
mod tests;
