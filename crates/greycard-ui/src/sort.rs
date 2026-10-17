//! The grid's order: one choice a view, kept between sessions.
//!
//! The order itself is `greycard_library::Order`, the one rule the
//! index's queries list by and a list made elsewhere is sorted by. The
//! browser's list is put in that order where it is read, off the
//! window's thread (`roots::Look::run`): a view of the index by its
//! query, a folder from the disk by the same comparison over its rows,
//! a collection by the same comparison over the references it found.
//! Everything that walks the list (the grid, the strip, culling's
//! arrows and its loupe, the lightbox, compare, Home and End, the
//! filter's rows) walks `State::files` and the cells made from it, so
//! it follows without a word of its own. A closed stack is its top's
//! cell where its top sorts, an open one's members follow the top in
//! the stack's order (`stacks::arrange`), a camera's JPEG is the raw's
//! and sorts as the raw, and a version's cell follows its file's own.
//!
//! The choice is kept a view at a time: one for a folder (from the
//! disk, or one folder of a root's tree on its own), one for a view of
//! several folders (every root, one root, a folder with those under it,
//! and so a smart collection, which opens as one of those), and one for
//! each collection. A folder is by name, the camera's numbering, until
//! another is chosen; several folders by capture time, so two bodies'
//! frames of one day read as the day went; a collection in its own
//! hand order, which a choice of order lays aside without rewriting.

use std::collections::BTreeMap;

use greycard_library::{Order, SortBy};
use serde::{Deserialize, Serialize};

use crate::roots::View;

/// A view's order: a collection's own, by hand, or an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Choice {
    Hand,
    By(Order),
}

impl Choice {
    /// The order, none for a collection's own.
    pub(crate) fn order(self) -> Option<Order> {
        match self {
            Choice::Hand => None,
            Choice::By(o) => Some(o),
        }
    }

    /// How the settings file spells it: `hand`, or the order's key.
    fn key(self) -> String {
        match self {
            Choice::Hand => "hand".to_string(),
            Choice::By(o) => o.key(),
        }
    }

    fn from_key(key: &str) -> Option<Choice> {
        match key {
            "hand" => Some(Choice::Hand),
            k => Order::from_key(k).map(Choice::By),
        }
    }
}

/// Which views share a choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Slot {
    /// One folder: from the disk, or one of a root's tree alone.
    Folder,
    /// Several folders: every root, one root, a folder with those
    /// under it; a smart collection opens as one of these.
    Roots,
    /// A collection, by its id.
    Collection(String),
}

impl Slot {
    pub(crate) fn of(view: &View) -> Slot {
        match view {
            View::Folder | View::Branch { deep: false, .. } => Slot::Folder,
            View::Roots(_) | View::Branch { deep: true, .. } => Slot::Roots,
            View::Collection { id } => Slot::Collection(id.clone()),
        }
    }

    /// What the view is in until another is chosen.
    pub(crate) fn default_choice(&self) -> Choice {
        match self {
            Slot::Folder => Choice::By(Order::NAME),
            Slot::Roots => Choice::By(Order::TAKEN),
            Slot::Collection(_) => Choice::Hand,
        }
    }
}

/// The choices kept, in `settings.json` beside the filter as `sort`:
/// `{"folder": "name", "roots": "capture-desc", "collections":
/// {"<id>": "hand"}}`. A view not named is in its default. A file from
/// before the sort has no `sort` and reads as every default; a name
/// this build does not know (a later build's order) reads as the
/// default too, and is kept as it was until a choice here replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub collections: BTreeMap<String, String>,
}

impl Kept {
    fn kept(&self, slot: &Slot) -> Option<&String> {
        match slot {
            Slot::Folder => self.folder.as_ref(),
            Slot::Roots => self.roots.as_ref(),
            Slot::Collection(id) => self.collections.get(id),
        }
    }

    /// The view's choice: the one kept, else its default. A collection's
    /// hand order is a collection's alone: kept for another view, it is
    /// that view's default.
    pub(crate) fn choice(&self, view: &View) -> Choice {
        let slot = Slot::of(view);
        let kept = self.kept(&slot).and_then(|k| Choice::from_key(k));
        match (kept, &slot) {
            (Some(Choice::Hand), Slot::Folder | Slot::Roots) | (None, _) => slot.default_choice(),
            (Some(c), _) => c,
        }
    }

    /// Keep `choice` for the view.
    pub(crate) fn set(&mut self, view: &View, choice: Choice) {
        let key = choice.key();
        match Slot::of(view) {
            Slot::Folder => self.folder = Some(key),
            Slot::Roots => self.roots = Some(key),
            Slot::Collection(id) => {
                self.collections.insert(id, key);
            }
        }
    }

    /// Every view of several folders by name: for a test whose frames
    /// are told apart by their place in a list it built in that order.
    #[cfg(test)]
    pub(crate) fn by_name() -> Kept {
        Kept {
            roots: Some(Order::NAME.key()),
            ..Kept::default()
        }
    }

    /// A collection deleted: its choice goes with it.
    pub(crate) fn forget(&mut self, id: &str) {
        self.collections.remove(id);
    }
}

/// The Sort menu's entries, in order: "Hand order" first in a
/// collection's, then each order. What the menu's index means.
pub(crate) fn entries(view: &View) -> Vec<Option<SortBy>> {
    let hand = matches!(Slot::of(view), Slot::Collection(_)).then_some(None);
    hand.into_iter()
        .chain(SortBy::ALL.into_iter().map(Some))
        .collect()
}

/// The words on the Sort button.
pub(crate) fn button_words(choice: Choice) -> String {
    match choice {
        Choice::Hand => "Hand order".to_string(),
        Choice::By(o) => o.by.words().to_string(),
    }
}

/// The words for each way up of an order: the oldest first, A to Z.
pub(crate) fn way_words(by: SortBy) -> [&'static str; 2] {
    match by {
        SortBy::Taken | SortBy::Edited => ["Oldest first", "Newest first"],
        SortBy::Name => ["A to Z", "Z to A"],
        SortBy::Rating => ["Lowest first", "Highest first"],
    }
}

/// The choice after the menu's entry `entry` is picked over `now`: an
/// order keeps the way up it had, so a pick of the same order again
/// changes nothing; "Hand order" lays the order aside.
pub(crate) fn picked(view: &View, now: Choice, entry: usize) -> Option<Choice> {
    let by = *entries(view).get(entry)?;
    Some(match by {
        None => Choice::Hand,
        Some(by) => Choice::By(Order::new(by, now.order().is_some_and(|o| o.descending))),
    })
}

/// The choice with its way up set; a collection's hand order has none.
pub(crate) fn turned(now: Choice, descending: bool) -> Choice {
    match now {
        Choice::Hand => Choice::Hand,
        Choice::By(o) => Choice::By(Order::new(o.by, descending)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn collection(id: &str) -> View {
        View::Collection { id: id.into() }
    }

    #[test]
    fn each_view_starts_in_the_order_that_suits_it() {
        let kept = Kept::default();
        assert_eq!(kept.choice(&View::Folder), Choice::By(Order::NAME));
        let branch = |deep| View::Branch {
            root: PathBuf::from("/p"),
            folder: PathBuf::from("/p/a"),
            deep,
        };
        assert_eq!(kept.choice(&branch(false)), Choice::By(Order::NAME));
        assert_eq!(kept.choice(&branch(true)), Choice::By(Order::TAKEN));
        assert_eq!(kept.choice(&View::Roots(None)), Choice::By(Order::TAKEN));
        assert_eq!(
            kept.choice(&View::Roots(Some(PathBuf::from("/p")))),
            Choice::By(Order::TAKEN)
        );
        assert_eq!(kept.choice(&collection("c1")), Choice::Hand);
    }

    #[test]
    fn a_choice_is_kept_for_its_view_alone_and_reads_back_from_the_file() {
        let mut kept = Kept::default();
        let newest = Choice::By(Order::new(SortBy::Taken, true));
        let rated = Choice::By(Order::new(SortBy::Rating, true));
        kept.set(&View::Roots(None), newest);
        kept.set(&collection("c1"), rated);
        kept.set(&collection("c2"), Choice::Hand);
        assert_eq!(kept.choice(&View::Roots(None)), newest);
        assert_eq!(kept.choice(&View::Folder), Choice::By(Order::NAME));
        assert_eq!(kept.choice(&collection("c1")), rated);
        assert_eq!(kept.choice(&collection("c2")), Choice::Hand);
        assert_eq!(kept.choice(&collection("c3")), Choice::Hand);
        let text = serde_json::to_string(&kept).unwrap();
        assert_eq!(
            text,
            r#"{"roots":"capture-desc","collections":{"c1":"rating-desc","c2":"hand"}}"#
        );
        let back: Kept = serde_json::from_str(&text).unwrap();
        assert_eq!(back, kept);
        kept.forget("c1");
        assert_eq!(kept.choice(&collection("c1")), Choice::Hand);
    }

    #[test]
    fn a_settings_file_from_before_the_sort_reads_as_every_default() {
        // A file as 0.7.0 wrote it: the filter kept, no sort.
        let old = r#"{"filter": {"stars": 3, "text": "iso>=400"}, "jpegs_apart": true}"#;
        let settings: crate::settings::Settings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.sort, Kept::default());
        assert_eq!(settings.filter.stars, 3);
        assert_eq!(
            settings.sort.choice(&View::Roots(None)),
            Choice::By(Order::TAKEN)
        );
        // A later build's order, and a hand order kept for a view that
        // has none: each view's default, the file's words kept.
        let later: Kept =
            serde_json::from_str(r#"{"folder": "size-desc", "roots": "hand"}"#).unwrap();
        assert_eq!(later.choice(&View::Folder), Choice::By(Order::NAME));
        assert_eq!(later.choice(&View::Roots(None)), Choice::By(Order::TAKEN));
        assert_eq!(later.folder.as_deref(), Some("size-desc"));
        // And the whole settings file round trips with one.
        let mut settings = crate::settings::Settings::default();
        settings
            .sort
            .set(&View::Folder, Choice::By(Order::new(SortBy::Edited, false)));
        let text = serde_json::to_string(&settings).unwrap();
        let back: crate::settings::Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.sort, settings.sort);
        assert_eq!(
            back.sort.choice(&View::Folder),
            Choice::By(Order::new(SortBy::Edited, false))
        );
    }

    #[test]
    fn the_menu_offers_hand_order_first_in_a_collection_and_keeps_the_way_up() {
        assert_eq!(entries(&View::Folder).len(), 4);
        assert_eq!(entries(&collection("c")).len(), 5);
        assert_eq!(entries(&collection("c"))[0], None);
        let newest = Choice::By(Order::new(SortBy::Taken, true));
        // Name picked over capture time, newest first: Z to A.
        assert_eq!(
            picked(&View::Folder, newest, 1),
            Some(Choice::By(Order::new(SortBy::Name, true)))
        );
        assert_eq!(picked(&collection("c"), newest, 0), Some(Choice::Hand));
        assert_eq!(
            picked(&collection("c"), Choice::Hand, 1),
            Some(Choice::By(Order::TAKEN))
        );
        assert_eq!(picked(&View::Folder, newest, 9), None);
        assert_eq!(
            turned(Choice::By(Order::NAME), true),
            Choice::By(Order::new(SortBy::Name, true))
        );
        assert_eq!(turned(Choice::Hand, true), Choice::Hand);
    }
}
