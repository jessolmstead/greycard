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
use std::path::PathBuf;

use greycard_library::{Order, SortBy};
use serde::{Deserialize, Serialize};

use crate::marked;

/// What the browser lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum View {
    /// A folder, or files the desktop asked to open.
    #[default]
    Folder,
    /// Every file under the roots, or under one of them.
    Roots(Option<PathBuf>),
    /// One folder under a root, from the index as a view of the root
    /// is: the frames directly in it, or with `deep` those in the
    /// folders under it too. Still that root's view, narrowed: its
    /// row stays lit.
    Branch {
        root: PathBuf,
        folder: PathBuf,
        deep: bool,
    },
    /// A collection, by its id: its pictures from the index, each
    /// once, in the collection's own order, or in the order chosen for
    /// it (`sort`).
    Collection { id: String },
    /// The export queue: every frame of every entry, each under its
    /// destination's heading, in the queue's order. A frame queued
    /// twice is listed twice: each is an export.
    Queue,
}

impl View {
    /// The list comes from the index's rows, not from a folder's
    /// listing on disk.
    pub fn lists_rows(&self) -> bool {
        !matches!(self, View::Folder)
    }

    /// What the index is asked to list: the files under each of these
    /// folders, or with the flag, those directly in the one folder.
    pub fn listed(&self, all: &[PathBuf]) -> (Vec<PathBuf>, bool) {
        match self {
            View::Folder => (Vec::new(), false),
            View::Roots(None) => (all.to_vec(), false),
            View::Roots(Some(r)) => (vec![r.clone()], false),
            View::Branch { folder, deep, .. } => (vec![folder.clone()], !deep),
            View::Collection { .. } | View::Queue => (Vec::new(), false),
        }
    }

    /// The collection this view shows, by its id.
    pub fn collection(&self) -> Option<&str> {
        match self {
            View::Collection { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// A view's order: a collection's own, by hand, or an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Hand,
    By(Order),
}

impl Choice {
    /// The order, none for a collection's own.
    pub fn order(self) -> Option<Order> {
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
pub enum Slot {
    /// One folder: from the disk, or one of a root's tree alone.
    Folder,
    /// Several folders: every root, one root, a folder with those
    /// under it; a smart collection opens as one of these.
    Roots,
    /// A collection, by its id.
    Collection(String),
    /// The export queue, always in its own order.
    Queue,
}

impl Slot {
    pub fn of(view: &View) -> Slot {
        match view {
            View::Folder | View::Branch { deep: false, .. } => Slot::Folder,
            View::Roots(_) | View::Branch { deep: true, .. } => Slot::Roots,
            View::Collection { id } => Slot::Collection(id.clone()),
            View::Queue => Slot::Queue,
        }
    }

    /// What the view is in until another is chosen.
    pub fn default_choice(&self) -> Choice {
        match self {
            Slot::Folder => Choice::By(Order::NAME),
            Slot::Roots => Choice::By(Order::TAKEN),
            Slot::Collection(_) | Slot::Queue => Choice::Hand,
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
            Slot::Queue => None,
        }
    }

    /// The view's choice: the one kept, else its default. A collection's
    /// hand order is a collection's alone: kept for another view, it is
    /// that view's default.
    pub fn choice(&self, view: &View) -> Choice {
        let slot = Slot::of(view);
        let kept = self.kept(&slot).and_then(|k| Choice::from_key(k));
        match (kept, &slot) {
            (Some(Choice::Hand), Slot::Folder | Slot::Roots) | (None, _) => slot.default_choice(),
            (Some(c), _) => c,
        }
    }

    /// Keep `choice` for the view.
    pub fn set(&mut self, view: &View, choice: Choice) {
        let key = choice.key();
        match Slot::of(view) {
            Slot::Folder => self.folder = Some(key),
            Slot::Roots => self.roots = Some(key),
            Slot::Collection(id) => {
                self.collections.insert(id, key);
            }
            // The queue's order is the order it runs in.
            Slot::Queue => {}
        }
    }

    /// Every view of several folders by name: for a test whose frames
    /// are told apart by their place in a list it built in that order.
    /// Built whatever the build: the binary's tests use it, and they
    /// build this library without cfg(test).
    #[doc(hidden)]
    pub fn by_name() -> Kept {
        Kept {
            roots: Some(Order::NAME.key()),
            ..Kept::default()
        }
    }

    /// A collection deleted: its choice goes with it.
    pub fn forget(&mut self, id: &str) {
        self.collections.remove(id);
    }
}

/// The Sort menu's entries, in order: "Hand order" first in a
/// collection's, then each order. What the menu's index means.
pub fn entries(view: &View) -> Vec<Option<SortBy>> {
    // The queue's order is the order it runs in, and nothing else.
    if Slot::of(view) == Slot::Queue {
        return vec![None];
    }
    let hand = matches!(Slot::of(view), Slot::Collection(_)).then_some(None);
    hand.into_iter()
        .chain(SortBy::ALL.into_iter().map(Some))
        .collect()
}

/// The words on the Sort button.
pub fn button_words(choice: Choice) -> String {
    match choice {
        Choice::Hand => marked::tr!("Hand order").to_string(),
        Choice::By(o) => o.by.words().to_string(),
    }
}

/// The words for each way up of an order: the oldest first, A to Z.
pub fn way_words(by: SortBy) -> [&'static str; 2] {
    match by {
        SortBy::Taken | SortBy::Edited => {
            // Sort menu: the two ways up for capture time and date edited
            [marked::tr!("Oldest first"), marked::tr!("Newest first")]
        }
        // Sort menu: the two ways up for the name
        SortBy::Name => [marked::tr!("A to Z"), marked::tr!("Z to A")],
        SortBy::Rating | SortBy::Focus => {
            // Sort menu: the two ways up for rating and focus
            [marked::tr!("Lowest first"), marked::tr!("Highest first")]
        }
    }
}

/// The order's words as a sentence says them, in lower case, their
/// own entries in the catalog.
const _LOWER_WORDS: [&str; 10] = [
    // The sort in the grid header's 'by {}', lower case
    crate::marked::tr!("hand order"),
    crate::marked::tr!("capture time"),
    crate::marked::tr!("name"),
    crate::marked::tr!("rating"),
    crate::marked::tr!("date edited"),
    crate::marked::tr!("focus"),
    crate::marked::tr!("oldest first"),
    crate::marked::tr!("newest first"),
    crate::marked::tr!("lowest first"),
    crate::marked::tr!("highest first"),
];

/// `SortBy::words`, which greycard-library holds, marked here for the
/// catalog: the menu translates them where it shows them.
const _BY_WORDS: [&str; 5] = [
    // Sort menu: what the frames are sorted by
    marked::tr!("Capture time"),
    marked::tr!("Name"),
    marked::tr!("Rating"),
    marked::tr!("Date edited"),
    marked::tr!("Focus"),
];

/// The choice after the menu's entry `entry` is picked over `now`: an
/// order keeps the way up it had, so a pick of the same order again
/// changes nothing; "Hand order" lays the order aside. Focus comes in
/// the sharpest first, which is what it is picked for.
pub fn picked(view: &View, now: Choice, entry: usize) -> Option<Choice> {
    let by = *entries(view).get(entry)?;
    let was = now.order();
    Some(match by {
        None => Choice::Hand,
        Some(SortBy::Focus) if was.is_none_or(|o| o.by != SortBy::Focus) => {
            Choice::By(Order::new(SortBy::Focus, true))
        }
        Some(by) => Choice::By(Order::new(by, was.is_some_and(|o| o.descending))),
    })
}

/// The choice with its way up set; a collection's hand order has none.
pub fn turned(now: Choice, descending: bool) -> Choice {
    match now {
        Choice::Hand => Choice::Hand,
        Choice::By(o) => Choice::By(Order::new(o.by, descending)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The words marked for the catalog are the library's own.
    #[test]
    fn the_marked_order_words_are_the_librarys() {
        let words: Vec<&str> = SortBy::ALL.iter().map(|b| b.words()).collect();
        assert_eq!(words, _BY_WORDS);
        // Each in lower case, as a sentence says it, is marked too.
        let mut said: Vec<String> = vec![button_words(Choice::Hand).to_lowercase()];
        for by in SortBy::ALL {
            said.push(by.words().to_lowercase());
            for way in way_words(by) {
                if way.chars().nth(1) != Some(' ') {
                    said.push(way.to_lowercase());
                }
            }
        }
        for word in said {
            assert!(_LOWER_WORDS.contains(&word.as_str()), "{word}");
        }
    }

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
    fn the_menu_offers_hand_order_first_in_a_collection_and_keeps_the_way_up() {
        assert_eq!(entries(&View::Folder).len(), 5);
        assert_eq!(entries(&collection("c")).len(), 6);
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
        // Focus comes in the sharpest first from any other order, and
        // keeps its way up once in it.
        let focus = entries(&View::Folder)
            .iter()
            .position(|e| *e == Some(SortBy::Focus))
            .unwrap();
        let sharpest = Choice::By(Order::new(SortBy::Focus, true));
        assert_eq!(
            picked(&View::Folder, Choice::By(Order::NAME), focus),
            Some(sharpest)
        );
        assert_eq!(
            picked(&collection("c"), Choice::Hand, focus + 1),
            Some(sharpest)
        );
        let softest = Choice::By(Order::new(SortBy::Focus, false));
        assert_eq!(picked(&View::Folder, softest, focus), Some(softest));
        assert_eq!(
            Choice::from_key(&sharpest.key()),
            Some(sharpest),
            "kept as focus-desc"
        );
    }
}
