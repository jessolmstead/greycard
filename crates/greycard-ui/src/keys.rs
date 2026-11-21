//! The window's keys: one table, read by the dispatcher and shown on
//! the keys sheet.
//!
//! A press reaches the window's key handler (`keys := FocusScope` in
//! greycard-slint's `ui/app.slint`), which asks [`chord`] for its name: "G",
//! "Ctrl+Shift+S", "Shift+Left", "?". The name comes back only when
//! the table binds it, so a key the table does not hold does nothing
//! however the handler is written, and the handler compares names,
//! never the event's text. What a chord does in which view (S is the
//! proof in the loupe and a stack in the grid) stays in the handler,
//! beside the properties it reads; what the user is told it does is
//! [`TABLE`]'s, and the sheet is built from that alone. A test reads
//! the handler's source and holds the two to each other: every name
//! it compares is bound here, and every chord here is compared there.
//!
//! The names are the platform's own on the sheet: Ctrl reads Cmd on a
//! Mac, where Slint calls the Command key `control`, and the Mac's
//! Backspace (marked delete) opens the delete sheet there alone.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::LazyLock;

use slint::platform::Key;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::i18n::text;
use crate::{App, BindRow, KeyRow, Shared, marked, tr};

/// A line of the sheet: the chords that do it, by [`chord`]'s names,
/// how the sheet writes them when the names alone would read badly,
/// and what they do.
pub(crate) struct Binding {
    pub(crate) chords: &'static [&'static str],
    /// The keys as the sheet shows them, with "Ctrl" for the platform
    /// to turn into "Cmd"; `None` writes each chord, comma between.
    pub(crate) shown: Option<&'static str>,
    pub(crate) does: &'static str,
    /// The name settings.json keeps a rebinding under, for a line the
    /// user may bind to another chord; `None` for one whose keys stay.
    pub(crate) action: Option<&'static str>,
    /// A pair of actions on one line, one a chord: the line's first
    /// chord is the first's default, its second the second's.
    pub(crate) pair: Option<[&'static str; 2]>,
    /// Keys the line holds as pressed, whatever an action with the
    /// same default is bound to: the brush's size on the brackets.
    pub(crate) raw: bool,
}

/// A heading of the sheet and its lines, in the order the guide's
/// Keys chapter has them.
pub(crate) struct Group {
    pub(crate) title: &'static str,
    pub(crate) lines: &'static [Binding],
}

const fn line(chords: &'static [&'static str], does: &'static str) -> Binding {
    Binding {
        chords,
        shown: None,
        does,
        action: None,
        pair: None,
        raw: false,
    }
}

/// A line the user may rebind: one chord, its default, under the name
/// `action` in settings.json. Two lines of one action (J's, one a
/// view) move together.
const fn act(
    action: &'static str,
    chord: &'static [&'static str; 1],
    does: &'static str,
) -> Binding {
    Binding {
        chords: chord,
        shown: None,
        does,
        action: Some(action),
        pair: None,
        raw: false,
    }
}

/// A line on the keys as pressed (`Binding::raw`).
const fn raw(chords: &'static [&'static str], shown: &'static str, does: &'static str) -> Binding {
    Binding {
        chords,
        shown: Some(shown),
        does,
        action: None,
        pair: None,
        raw: true,
    }
}

/// Two actions on one line, a direction each (the turn's left and
/// right), shown as the line's `shown` while neither has moved.
const fn pair(
    actions: [&'static str; 2],
    chords: &'static [&'static str; 2],
    shown: &'static str,
    does: &'static str,
) -> Binding {
    Binding {
        chords,
        shown: Some(shown),
        does,
        action: None,
        pair: Some(actions),
        raw: false,
    }
}

const fn shown(
    chords: &'static [&'static str],
    shown: &'static str,
    does: &'static str,
) -> Binding {
    Binding {
        chords,
        shown: Some(shown),
        does,
        action: None,
        pair: None,
        raw: false,
    }
}

/// The pointer's own, which the sheet lists among the keys since a
/// Lightroom hand looks for them there: not chords, and bound by the
/// strip's and the grid's cells rather than the key handler.
const POINTER: &[Binding] = &[
    shown(
        &[],
        "Ctrl+click",
        marked::tr!("Add a frame to the selection, or take it out"),
    ),
    shown(
        &[],
        "Shift+click",
        marked::tr!("Select every frame from the open one to this one (with Ctrl, added)"),
    ),
];

/// Every key the window binds, grouped as the guide's Keys chapter
/// groups them.
pub(crate) const TABLE: &[Group] = &[
    Group {
        // Keys sheet: a heading
        title: marked::tr!("Moving around"),
        lines: &[
            // The Loupe's step: the grid's and the lightbox's arrows
            // move their selection by the defaults whatever is bound
            // (`LOUPE_ONLY`).
            pair(
                ["previous-frame", "next-frame"],
                &["Left", "Right"],
                "← →",
                marked::tr!("Previous or next frame"),
            ),
            // These follow the pair above (`SHIFT_FORMS`).
            shown(
                &["Shift+Left", "Shift+Right"],
                "Shift+← →",
                marked::tr!("Previous or next frame, added to the selection"),
            ),
            shown(
                &["Up", "Down", "Shift+Up", "Shift+Down"],
                "↑ ↓",
                marked::tr!("In the grid, the frame above or below (with Shift, added)"),
            ),
            shown(
                // With Ctrl too, as master took them.
                &[
                    "Home",
                    "End",
                    "Shift+Home",
                    "Shift+End",
                    "Ctrl+Home",
                    "Ctrl+End",
                    "Ctrl+Shift+Home",
                    "Ctrl+Shift+End",
                ],
                "Home, End",
                marked::tr!("The first or the last frame (with Shift, added)"),
            ),
            // The mouse's back and forward side buttons over the grid
            // (`panel::visited`). Not Cmd+[ and Cmd+] on a Mac as well:
            // Ctrl+[ and Ctrl+] are the brush's opacity.
            pair(
                ["visit-back", "visit-forward"],
                &["Alt+Left", "Alt+Right"],
                "Alt+← →",
                marked::tr!("In the grid, back or forward through the folders visited"),
            ),
            act(
                "select-all",
                &["Ctrl+A"],
                marked::tr!("Select every frame shown"),
            ),
        ],
    },
    Group {
        // Keys sheet: a heading
        title: marked::tr!("Views"),
        lines: &[
            act(
                "library",
                &["G"],
                marked::tr!("Library (the grid), and back to the Loupe as it was left"),
            ),
            act(
                "loupe",
                &["E"],
                marked::tr!("The Loupe (one frame), in the mode it was left in: Cull or Develop"),
            ),
            act(
                "develop",
                &["D"],
                marked::tr!("Develop: the Loupe with the edit panel"),
            ),
            act("lightbox", &["L"], marked::tr!("The lightbox, and back")),
            act(
                "overlays",
                &["J"],
                marked::tr!("In the lightbox, the rating, flag, and label on the tiles, or not"),
            ),
            act(
                "cull",
                &["C"],
                marked::tr!("Cull; in the Loupe, Cull or Develop in turn"),
            ),
            act(
                "compare",
                &["V"],
                marked::tr!("In Cull, compare two or four frames"),
            ),
            shown(
                &["Return"],
                "Enter",
                marked::tr!(
                    "Open the frame chosen in the grid or the lightbox; from Cull, Develop"
                ),
            ),
            shown(
                // The grid's with Ctrl too, as a browser's zoom is.
                &["+", "=", "-", "_", "Ctrl++", "Ctrl+=", "Ctrl+-", "Ctrl+_"],
                "+ −",
                marked::tr!(
                    "Bigger or smaller tiles in the grid; fewer or more across the lightbox; in the Loupe, zoom in or out"
                ),
            ),
            line(
                &["Tab"],
                marked::tr!(
                    "Hide the panels and the filmstrip, and bring them back (in the grid, the left pane)"
                ),
            ),
            shown(
                &["F6", "F7", "F8"],
                "F6, F7, F8",
                marked::tr!("The filmstrip, the left pane or the develop panel alone"),
            ),
            // Not on a Mac (`NOT_MAC`).
            line(&["F11"], marked::tr!("Full screen, and back")),
            act("zoom-1to1", &["Space"], marked::tr!("Fit or 1:1")),
            act("zoom-3to1", &["Z"], marked::tr!("Fit or 3:1")),
            act(
                "mat",
                &["F"],
                marked::tr!("A mat of the canvas around the picture at Fit, or none"),
            ),
            act(
                "overlays",
                &["J"],
                marked::tr!("In the Loupe, shadow and highlight clipping warnings"),
            ),
            act(
                "peaking",
                &["Shift+J"],
                marked::tr!(
                    "In the Loupe, focus peaking: the sharpest parts painted over the picture"
                ),
            ),
            act(
                "proof",
                &["S"],
                marked::tr!("Soft proof (in the grid, a stack: below)"),
            ),
        ],
    },
    Group {
        // Keys sheet: a heading
        title: marked::tr!("Rating and sorting"),
        lines: &[
            shown(
                &["1", "2", "3", "4", "5", "0"],
                // Keys sheet: the key column for the rating keys
                marked::tr!("1 to 5, 0"),
                marked::tr!("Set a rating, or clear it"),
            ),
            // With Shift too: a hand that holds it for the stars of
            // another application still flags.
            shown(
                &["P", "X", "U", "Shift+P", "Shift+X", "Shift+U"],
                "P, X, U",
                marked::tr!("Pick, reject, unflag"),
            ),
            shown(
                &["6", "7", "8", "9"],
                // Keys sheet: the key column for the label keys
                marked::tr!("6 to 9"),
                marked::tr!("Red, yellow, green or blue label (again to clear)"),
            ),
            pair(
                ["turn-left", "turn-right"],
                &["[", "]"],
                "[ ]",
                marked::tr!("Turn the frame a quarter left or right; not part of the edit"),
            ),
            act(
                "collection",
                &["B"],
                marked::tr!("Add the selected frames to a collection, or take them out"),
            ),
            act(
                "stack",
                &["Ctrl+G"],
                marked::tr!("Stack the selected frames"),
            ),
            act("unstack", &["Ctrl+Shift+G"], marked::tr!("Unstack them")),
            shown(
                &["S", "Shift+S"],
                "S, Shift+S",
                marked::tr!("In the grid, open or close a stack; make the frame its top"),
            ),
            // A brace is Shift and a bracket on most boards.
            shown(
                &["{", "}"],
                "Shift+[ Shift+]",
                marked::tr!("In the grid, move the frame up or down in its stack"),
            ),
            act(
                "version",
                &["Ctrl+'"],
                marked::tr!("New version of the frame"),
            ),
            line(
                &["Delete", "Backspace"],
                marked::tr!("Delete the selected frames, after a confirmation"),
            ),
        ],
    },
    Group {
        title: marked::tr!("Editing"),
        lines: &[
            act("undo", &["Ctrl+Z"], marked::tr!("Undo")),
            shown(
                &["Ctrl+Shift+Z", "Ctrl+Y"],
                "Ctrl+Shift+Z or Ctrl+Y",
                marked::tr!("Redo"),
            ),
            act(
                "copy",
                &["Ctrl+C"],
                marked::tr!("Copy the open frame's settings"),
            ),
            act(
                "paste",
                &["Ctrl+V"],
                marked::tr!("Paste them onto the selected frames"),
            ),
            act(
                "sync",
                &["Ctrl+Shift+S"],
                marked::tr!("Sync the selected frames to the open one"),
            ),
            act("crop", &["R"], marked::tr!("Crop, and Done")),
            act(
                "white-balance",
                &["W"],
                marked::tr!("White balance from a neutral patch: the pipette"),
            ),
            act(
                "linear-mask",
                &["M"],
                marked::tr!("New mask with a Linear, placed by the next drag"),
            ),
            act(
                "radial-mask",
                &["Shift+M"],
                marked::tr!("New mask with a Radial, placed by the next drag"),
            ),
            // The brush in hand's, as its sliders: the brackets size
            // Lightroom's brush, and Shift and one feather it. The
            // size is the brackets themselves with a brush or a patch
            // in hand, wherever the turn is bound (`BRUSH`).
            raw(
                &["[", "]"],
                "[ ]",
                marked::tr!("Brush size, smaller and bigger"),
            ),
            shown(
                &["{", "}"],
                "Shift+[ Shift+]",
                marked::tr!("Brush feather, less and more"),
            ),
            shown(
                &["Ctrl+[", "Ctrl+]"],
                "Ctrl+[ Ctrl+]",
                marked::tr!("Brush opacity, less and more"),
            ),
            act("show-mask", &["O"], marked::tr!("Show the mask")),
            // O's other line, as J has two: crop mode is the Crop
            // tab's, where no mask is shown. Lightroom's crop keys.
            act(
                "show-mask",
                &["O"],
                // Keys sheet: O in crop mode, the next composition guide
                marked::tr!("In the crop, the next guide over it"),
            ),
            act(
                "crop-guide-turn",
                &["Shift+O"],
                // Keys sheet: Shift+O in crop mode
                marked::tr!("In the crop, turn the golden spiral or the triangles"),
            ),
            act(
                "original",
                &["\\"],
                marked::tr!("Show the frame as it was, and back"),
            ),
        ],
    },
    // A focused slider's own (`EditSlider` in greycard-slint's
    // `ui/controls.slint`), not the handler's: no chords, and the
    // window has these keys back once no slider has the focus.
    Group {
        // Keys sheet: a heading
        title: marked::tr!("Sliders"),
        lines: &[
            shown(
                &[],
                "↑ ↓ ← →, + −",
                marked::tr!(
                    "Nudge the focused slider, or its typed value with ↑ ↓; Shift on an arrow for more, Ctrl on the slider for less, and Shift on a drag for finer"
                ),
            ),
            shown(&[], ", .", marked::tr!("The previous or the next slider")),
            shown(
                &[],
                "Tab, Shift+Tab",
                marked::tr!("In a slider's typed value, the next or the previous one"),
            ),
        ],
    },
    Group {
        // Keys sheet: a heading
        title: marked::tr!("Files, sheets and help"),
        lines: &[
            shown(
                &["Ctrl+F", "/"],
                "Ctrl+F or /",
                marked::tr!("Filter the folder by rating, flag, label or words"),
            ),
            act("open", &["Ctrl+O"], marked::tr!("Open a folder")),
            act(
                "import",
                &["Ctrl+Shift+I"],
                marked::tr!("Import from a card"),
            ),
            act("export", &["Ctrl+Shift+E"], marked::tr!("Export")),
            act(
                "export-again",
                &["Ctrl+E"],
                marked::tr!("Export with the last settings"),
            ),
            act("settings", &["Ctrl+,"], marked::tr!("Settings")),
            act("keys", &["?"], marked::tr!("The keys sheet")),
            line(
                &["Esc"],
                marked::tr!(
                    "Close a sheet, put a tool down, leave the crop, let go of a set, or go back to one frame"
                ),
            ),
        ],
    },
];

/// The chords bound on one platform only: the Mac's delete key is
/// Backspace, and elsewhere Backspace is nobody's.
const MAC_ONLY: &[&str] = &["Backspace"];

/// The chords bound everywhere but a Mac: F11, where its green button
/// takes the window full screen and F11 shows the desktop. A line of
/// these alone is not on a Mac's sheet.
const NOT_MAC: &[&str] = &["F11"];

/// Whether a chord is bound on this platform.
fn here(chord: &str, mac: bool) -> bool {
    if mac {
        !NOT_MAC.contains(&chord)
    } else {
        !MAC_ONLY.contains(&chord)
    }
}

/// Whether a line is on this platform's sheet: a line of keys bound
/// elsewhere alone is not.
fn shown_here(b: &Binding, mac: bool) -> bool {
    b.chords.is_empty() || b.chords.iter().any(|c| here(c, mac))
}

/// Whether `name` is a chord the table holds on this platform, by
/// the defaults.
fn bound(name: &str, mac: bool) -> bool {
    here(name, mac)
        && TABLE
            .iter()
            .flat_map(|g| g.lines)
            .any(|b| b.chords.contains(&name))
}

/// A line the user may rebind, once however many lines it has: its
/// name in settings.json, its default chord, which is also the name
/// the window's key handler compares it by, what its lines do, and
/// the group of its first line.
#[derive(Debug)]
pub(crate) struct Action {
    pub(crate) id: &'static str,
    pub(crate) default: &'static str,
    pub(crate) does: Vec<&'static str>,
    pub(crate) group: usize,
    /// The second of a pair's actions, on the first: the two are one
    /// row of the Settings sheet.
    pub(crate) second: Option<&'static str>,
    /// Whether it is a pair's second, whose row is its first's.
    pub(crate) paired: bool,
}

/// A line's actions, each with its default chord.
fn actions_of(b: &Binding) -> Vec<(&'static str, &'static str)> {
    match (b.action, b.pair) {
        (Some(id), _) => vec![(id, b.chords[0])],
        (None, Some([one, two])) => vec![(one, b.chords[0]), (two, b.chords[1])],
        (None, None) => Vec::new(),
    }
}

static ACTIONS: LazyLock<Vec<Action>> = LazyLock::new(|| {
    let mut out: Vec<Action> = Vec::new();
    for (group, g) in TABLE.iter().enumerate() {
        for b in g.lines {
            for (i, (id, default)) in actions_of(b).into_iter().enumerate() {
                match out.iter_mut().find(|a| a.id == id) {
                    Some(a) => a.does.push(b.does),
                    None => out.push(Action {
                        id,
                        default,
                        does: vec![b.does],
                        group,
                        second: b.pair.filter(|_| i == 0).map(|p| p[1]),
                        paired: i == 1,
                    }),
                }
            }
        }
    }
    out
});

/// The actions the Loupe alone goes by: in the grid and the lightbox
/// the arrows move the selection by their defaults, whatever is
/// bound here.
const LOUPE_ONLY: &[&str] = &["previous-frame", "next-frame"];

/// The fixed chords that follow an action as its Shift form: the
/// step's Shift adds to the selection, on whatever key steps.
const SHIFT_FORMS: &[(&str, &str)] = &[
    ("Shift+Left", "previous-frame"),
    ("Shift+Right", "next-frame"),
];

/// The chord with Shift on it, `None` for none bound, a chord that
/// holds Shift already, or a key that carries its own.
fn shift_form(chord: &str) -> Option<String> {
    if chord.is_empty() || chord.contains("Shift+") {
        return None;
    }
    parse(&format!("Shift+{chord}"))
}

/// The brackets as pressed with a brush or a patch in hand, and the
/// names the handler reads them by: apart from the turn's `[` and
/// `]`, so the turn's binding never moves the sizing.
const BRUSH: &[(&str, &str)] = &[("[", "Brush["), ("]", "Brush]")];

/// Where a press lands: whether the Loupe has the keys (not the grid
/// or the lightbox, which step by the arrows' defaults), and whether
/// a brush or a repair's patch is in hand in Develop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct View {
    pub(crate) loupe: bool,
    pub(crate) brush: bool,
}

#[cfg(test)]
const LOUPE: View = View {
    loupe: true,
    brush: false,
};

/// A line's own keys in the window's words: two chords either of
/// which does it joined by "or", the chords as they are; a click's
/// chord with the word after it; the rest translated as one.
fn shown_words(s: &str) -> String {
    if let Some((a, b)) = s.split_once(" or ") {
        // Keys sheet: two keys that do the same
        return tr!("{} or {}", a, b);
    }
    match s.rsplit_once('+') {
        // The pointer's word, after its chord: "Ctrl+click".
        Some((mods, "click")) => format!("{mods}+{}", tr!("click")),
        _ => text(s),
    }
}

/// The rebindable actions, in the table's order.
pub(crate) fn actions() -> &'static [Action] {
    &ACTIONS
}

/// Whether `chord` is an action's default: it then reaches the
/// handler only by that action, wherever the action is bound now.
fn is_default(chord: &str) -> bool {
    actions().iter().any(|a| a.default == chord)
}

/// What an action's lines say it does, in one, in the window's words.
fn does_of(a: &Action) -> String {
    let mut out = String::new();
    for (i, d) in a.does.iter().enumerate() {
        let d = text(d);
        if i == 0 {
            out.push_str(&d);
            continue;
        }
        out.push_str("; ");
        let mut chars = d.chars();
        if let Some(c) = chars.next() {
            out.extend(c.to_lowercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// The bindings in force: the actions moved off their defaults, each
/// with its chord. Empty is the table as shipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Keymap {
    moved: BTreeMap<&'static str, String>,
}

impl Keymap {
    /// The chord `a` is bound to, empty for none.
    pub(crate) fn chord_of(&self, a: &Action) -> &str {
        self.moved.get(a.id).map_or(a.default, String::as_str)
    }

    fn chord_of_id(&self, id: &str) -> &str {
        actions()
            .iter()
            .find(|a| a.id == id)
            .map_or("", |a| self.chord_of(a))
    }

    /// Bind action `id` to no key; false for an unknown action.
    pub(crate) fn clear(&mut self, id: &str) -> bool {
        let Some(a) = actions().iter().find(|a| a.id == id) else {
            return false;
        };
        self.moved.insert(a.id, String::new());
        true
    }

    /// The bindings a view goes by: outside the Loupe its own actions
    /// are at their defaults.
    fn in_view(&self, loupe: bool) -> std::borrow::Cow<'_, Self> {
        if loupe || !LOUPE_ONLY.iter().any(|id| self.moved.contains_key(id)) {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut map = self.clone();
        for id in LOUPE_ONLY {
            map.moved.remove(id);
        }
        std::borrow::Cow::Owned(map)
    }

    /// Bind action `id` to `chord`, a chord's name as [`parse`] gives
    /// it; false, and nothing changed, for an unknown action or a
    /// chord nothing can press. Bound to its default, it is unmoved.
    pub(crate) fn set(&mut self, id: &str, chord: &str) -> bool {
        let Some(a) = actions().iter().find(|a| a.id == id) else {
            return false;
        };
        if !bindable(chord) {
            return false;
        }
        if chord == a.default {
            self.moved.remove(a.id);
        } else {
            self.moved.insert(a.id, chord.to_string());
        }
        true
    }

    pub(crate) fn reset(&mut self) {
        self.moved.clear();
    }

    /// From settings.json's `keys`: each action to its chord, parsed,
    /// or to none for an empty one; a name that is no action or a
    /// chord that does not parse is logged and left at its default.
    pub(crate) fn from_saved(saved: &BTreeMap<String, String>) -> Self {
        let mut map = Self::default();
        for (id, text) in saved {
            let taken = if text.trim().is_empty() {
                map.clear(id)
            } else {
                parse(text).is_some_and(|c| map.set(id, &c))
            };
            if !taken {
                tracing::warn!(
                    "keys: settings.json binds {id:?} to {text:?}, not an action and a chord; left as it was"
                );
            }
        }
        map
    }

    /// What settings.json keeps: the moved bindings alone.
    pub(crate) fn saved(&self) -> BTreeMap<String, String> {
        self.moved
            .iter()
            .map(|(id, c)| (id.to_string(), c.clone()))
            .collect()
    }

    /// What the handler reads for a press named `name` in `view`: a
    /// bracket's brush name with a brush in hand; the default of the
    /// action bound to it, the first in the table when two are; an
    /// action's Shift form for that chord with Shift; the name itself
    /// when it is one of the table's fixed chords; empty when nothing
    /// has it.
    fn resolve(&self, name: &str, mac: bool, view: View) -> String {
        if name.is_empty() {
            return String::new();
        }
        if view.brush
            && let Some((_, n)) = BRUSH.iter().find(|(k, _)| *k == name)
        {
            return n.to_string();
        }
        let map = self.in_view(view.loupe);
        let form = SHIFT_FORMS
            .iter()
            .find(|(_, id)| shift_form(map.chord_of_id(id)).as_deref() == Some(name))
            .map(|(form, _)| form.to_string());
        // Outside the Loupe the step is the selection's, on its
        // defaults: its Shift form comes before an action there.
        if !view.loupe
            && let Some(form) = form
        {
            return form;
        }
        if let Some(a) = actions().iter().find(|a| map.chord_of(a) == name) {
            return a.default.to_string();
        }
        if let Some(form) = form {
            return form;
        }
        let follows = SHIFT_FORMS.iter().any(|(form, _)| *form == name);
        if !is_default(name) && !follows && bound(name, mac) {
            name.to_string()
        } else {
            String::new()
        }
    }

    /// A chord of the table as bound now: an action's default as
    /// its action is, a Shift form as its action's chord with Shift,
    /// any other as itself; empty for none.
    fn now(&self, chord: &'static str) -> String {
        if let Some(a) = actions().iter().find(|a| a.default == chord) {
            return self.chord_of(a).to_string();
        }
        match SHIFT_FORMS.iter().find(|(form, _)| *form == chord) {
            Some((_, id)) => shift_form(self.chord_of_id(id)).unwrap_or_default(),
            None => chord.to_string(),
        }
    }

    /// Each action whose chord something else has too, with what the
    /// others do: another action's first line, or a fixed line the
    /// action takes the chord from.
    pub(crate) fn conflicts(&self, mac: bool) -> Vec<(&'static Action, Vec<&'static str>)> {
        // The actions on `chord` but `a`, and the fixed lines on it
        // that `a` would take it from.
        let on = |a: &Action, chord: &str, others: &mut Vec<&'static str>| {
            for b in actions() {
                if b.id != a.id && self.chord_of(b) == chord {
                    others.push(b.does[0]);
                }
            }
            for b in TABLE.iter().flat_map(|g| g.lines) {
                // The Shift forms' line is the step's, below.
                let forms = b
                    .chords
                    .iter()
                    .any(|c| SHIFT_FORMS.iter().any(|(f, _)| f == c));
                if b.action.is_some() || b.pair.is_some() || forms {
                    continue;
                }
                if !b.chords.contains(&chord) {
                    continue;
                }
                // A raw line holds its keys whatever is bound, but an
                // action whose default is one of them (the turn) shares
                // them by design, wherever it is moved among them.
                let taken = if b.raw {
                    !b.chords.contains(&a.default)
                } else {
                    !is_default(chord) && bound(chord, mac)
                };
                if taken {
                    others.push(b.does);
                }
            }
            // A step's Shift form on the chord.
            for (form, id) in SHIFT_FORMS {
                if *id != a.id && shift_form(self.chord_of_id(id)).as_deref() == Some(chord) {
                    others.extend(
                        TABLE
                            .iter()
                            .flat_map(|g| g.lines)
                            .filter(|b| b.chords.contains(form))
                            .map(|b| b.does),
                    );
                }
            }
        };
        let mut out = Vec::new();
        for a in actions() {
            let c = self.chord_of(a);
            // Two actions on no key share nothing.
            if c.is_empty() {
                continue;
            }
            let mut others = Vec::new();
            on(a, c, &mut others);
            // A step's own Shift form takes its chord too.
            if SHIFT_FORMS.iter().any(|(_, id)| *id == a.id)
                && let Some(f) = shift_form(c)
            {
                on(a, &f, &mut others);
            }
            let mut seen = Vec::new();
            others.retain(|o| {
                let fresh = !seen.contains(o);
                seen.push(*o);
                fresh
            });
            if !others.is_empty() {
                out.push((a, others));
            }
        }
        out
    }
}

thread_local! {
    /// The bindings the window's keys go by: the one place a press
    /// is looked up. The defaults at each window's install, then
    /// settings.json's as the start reads them.
    static KEYMAP: RefCell<Keymap> = RefCell::new(Keymap::default());
}

fn with_map<R>(f: impl FnOnce(&Keymap) -> R) -> R {
    KEYMAP.with(|m| f(&m.borrow()))
}

/// The keys Slint names by a private character, by the name a chord
/// gives them.
fn named_keys() -> [(Key, &'static str); 26] {
    [
        (Key::Escape, "Esc"),
        (Key::Return, "Return"),
        (Key::Tab, "Tab"),
        (Key::Delete, "Delete"),
        (Key::Backspace, "Backspace"),
        (Key::LeftArrow, "Left"),
        (Key::RightArrow, "Right"),
        (Key::UpArrow, "Up"),
        (Key::DownArrow, "Down"),
        (Key::Home, "Home"),
        (Key::End, "End"),
        (Key::PageUp, "PageUp"),
        (Key::PageDown, "PageDown"),
        (Key::Insert, "Insert"),
        (Key::F1, "F1"),
        (Key::F2, "F2"),
        (Key::F3, "F3"),
        (Key::F4, "F4"),
        (Key::F5, "F5"),
        (Key::F6, "F6"),
        (Key::F7, "F7"),
        (Key::F8, "F8"),
        (Key::F9, "F9"),
        (Key::F10, "F10"),
        (Key::F11, "F11"),
        (Key::F12, "F12"),
    ]
}

/// Slint's private characters for its named keys, which a chord
/// names only through [`named_keys`].
fn private(c: char) -> bool {
    ('\u{E000}'..='\u{F8FF}').contains(&c)
}

/// A key's name, and whether a chord says Shift on it: a named key, a
/// space, or a letter does; any other character carries its Shift
/// already.
fn key_of(text: &str) -> Option<(String, bool)> {
    if let Some((_, n)) = named_keys()
        .into_iter()
        .find(|(k, _)| SharedString::from(*k).as_str() == text)
    {
        return Some((n.to_string(), true));
    }
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(' '), None) => Some(("Space".to_string(), true)),
        (Some(c), None) if c.is_ascii_alphabetic() => {
            Some((c.to_ascii_uppercase().to_string(), true))
        }
        (Some(c), None) if !c.is_control() && !private(c) => Some((c.to_string(), false)),
        _ => None,
    }
}

/// A chord's name from its parts: the key after whichever of Ctrl,
/// Alt, Meta and Shift are held, in that order, Shift only where the
/// key says it. Escape is "Esc" whatever is held.
fn compose(key: &str, says_shift: bool, ctrl: bool, shift: bool, alt: bool, meta: bool) -> String {
    if key == "Esc" {
        return "Esc".to_string();
    }
    let mut out = String::new();
    for (held, word) in [
        (ctrl, "Ctrl+"),
        (alt, "Alt+"),
        (meta, "Meta+"),
        (shift && says_shift, "Shift+"),
    ] {
        if held {
            out.push_str(word);
        }
    }
    out.push_str(key);
    out
}

/// The name of a press: the key, after whichever of Ctrl, Alt, Meta
/// and Shift are held, in that order. A letter is upper case and says
/// its Shift, so Caps Lock is no Shift; any other character carries
/// its Shift already (a "?" is Shift and "/", a "+" Shift and "=" on
/// most boards) and does not say it again. Escape is "Esc" whatever is
/// held: it closes a sheet or puts a tool down with a hand still on a
/// modifier. Pure: the name whether or not anything binds it, empty
/// for a modifier alone or a key no chord names.
pub(crate) fn name(text: &str, ctrl: bool, shift: bool, alt: bool, meta: bool) -> String {
    match key_of(text) {
        Some((key, says_shift)) => compose(&key, says_shift, ctrl, shift, alt, meta),
        None => String::new(),
    }
}

/// A chord's name from text as settings.json or the sheet writes it:
/// the modifiers in any order and case, Cmd for Ctrl, the key by its
/// name or by the sheet's arrow; `None` for text no press is named by.
pub(crate) fn parse(text: &str) -> Option<String> {
    let (mut ctrl, mut shift, mut alt, mut meta) = (false, false, false, false);
    let mut rest = text.trim();
    while let Some((word, after)) = rest.split_once('+') {
        // "+" alone, or as the key after a modifier ("Ctrl++").
        if word.is_empty() || after.is_empty() {
            break;
        }
        match word.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" => ctrl = true,
            "alt" | "option" => alt = true,
            "meta" | "super" => meta = true,
            "shift" => shift = true,
            _ => return None,
        }
        rest = after;
    }
    let (key, says_shift) = match rest.to_ascii_lowercase().as_str() {
        "escape" => ("Esc".to_string(), true),
        "enter" => ("Return".to_string(), true),
        "space" => ("Space".to_string(), true),
        "←" => ("Left".to_string(), true),
        "→" => ("Right".to_string(), true),
        "↑" => ("Up".to_string(), true),
        "↓" => ("Down".to_string(), true),
        "⌫" => ("Backspace".to_string(), true),
        _ => match named_keys()
            .into_iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(rest))
        {
            Some((_, n)) => (n.to_string(), true),
            None => key_of(rest)?,
        },
    };
    // Shift on a key that carries it: no press is named so.
    if shift && !says_shift {
        return None;
    }
    Some(compose(&key, says_shift, ctrl, shift, alt, meta))
}

/// Whether an action can be bound to `chord`: a name a press has,
/// and not Escape, which closes and cancels whatever else is bound.
fn bindable(chord: &str) -> bool {
    chord != "Esc" && parse(chord).as_deref() == Some(chord)
}

/// What the window's key handler reads: the press by the name the
/// handler compares, by the bindings in force, empty when nothing
/// has it. A rebound action's press reads as the action's default.
/// `view` is where the press lands ([`View`]).
pub(crate) fn chord(
    text: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
    view: View,
) -> String {
    chord_on(
        text,
        ctrl,
        shift,
        alt,
        meta,
        cfg!(target_os = "macos"),
        view,
    )
}

fn chord_on(
    text: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
    mac: bool,
    view: View,
) -> String {
    let n = name(text, ctrl, shift, alt, meta);
    with_map(|m| m.resolve(&n, mac, view))
}

/// A chord as the sheet writes it on this platform; "None" for none.
fn said(chord: &str, mac: bool) -> String {
    if chord.is_empty() {
        // What the sheets write for an action bound to no key.
        return tr!("key binding" => "None");
    }
    // The key is after the last "+", unless "+" is the key.
    let (mods, key) = match chord.strip_suffix("++") {
        Some(m) => (&chord[..m.len() + 1], "+"),
        None => match chord.rsplit_once('+') {
            Some((m, k)) if !k.is_empty() => (&chord[..m.len() + 1], k),
            _ => ("", chord),
        },
    };
    let key = match key {
        "Left" => "←",
        "Right" => "→",
        "Up" => "↑",
        "Down" => "↓",
        "Return" => "Enter",
        "Backspace" => "⌫",
        other => other,
    };
    platform(&format!("{mods}{key}"), mac)
}

/// Ctrl as the platform names it.
fn platform(text: &str, mac: bool) -> String {
    if mac {
        text.replace("Ctrl+", "Cmd+")
    } else {
        text.to_string()
    }
}

/// A line's keys as the sheet writes them, as bound now: a line with
/// a chord moved is written chord by chord.
fn keys_of(b: &Binding, map: &Keymap, mac: bool) -> String {
    let moved = !b.raw && b.chords.iter().any(|c| map.now(c) != *c);
    match b.shown {
        Some(s) if !moved => {
            let s = platform(&shown_words(s), mac);
            // The Mac's own delete beside Delete.
            if mac && b.chords.contains(&"Backspace") {
                format!("{s}, ⌫")
            } else {
                s
            }
        }
        _ => b
            .chords
            .iter()
            .filter(|c| here(c, mac))
            .map(|c| said(&map.now(c), mac))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// The sheet's rows for a search, by the bindings in force.
pub(crate) fn rows(query: &str, mac: bool) -> Vec<KeyRow> {
    with_map(|m| rows_in(m, query, mac))
}

/// The sheet's rows for a search: each group's heading over its
/// lines, a line kept when its keys or its words hold `query` (case
/// aside), a group kept when a line of it is or its title holds it.
/// The pointer's two go under the first group. An empty query keeps
/// everything.
fn rows_in(map: &Keymap, query: &str, mac: bool) -> Vec<KeyRow> {
    let q = query.trim().to_lowercase();
    let holds = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
    let mut out = Vec::new();
    for (i, g) in TABLE.iter().enumerate() {
        let title = text(g.title);
        let whole = !q.is_empty() && holds(&title);
        let pointer = if i == 0 { POINTER } else { &[] };
        let lines: Vec<KeyRow> = g
            .lines
            .iter()
            .filter(|b| shown_here(b, mac))
            .map(|b| (keys_of(b, map, mac), platform(&text(b.does), mac)))
            .chain(pointer.iter().map(|b| {
                (
                    platform(&shown_words(b.shown.unwrap_or_default()), mac),
                    text(b.does),
                )
            }))
            .filter(|(keys, does)| whole || holds(keys) || holds(does))
            .map(|(keys, does)| KeyRow {
                heading: false,
                keys: keys.into(),
                does: does.into(),
            })
            .collect();
        if lines.is_empty() {
            continue;
        }
        out.push(KeyRow {
            heading: true,
            keys: title.into(),
            does: SharedString::new(),
        });
        out.extend(lines);
    }
    out
}

/// The Settings sheet's Keys rows: each group's heading over its
/// actions, each with its chord as bound ("None" for none), what it
/// does, and what else has its chord ("Also: ..."), empty when nothing
/// does. A pair is one row, its second action's chord beside the
/// first's.
pub(crate) fn binding_rows(map: &Keymap, mac: bool) -> Vec<BindRow> {
    let conflicts = map.conflicts(mac);
    let also_of = |id: &str| {
        conflicts
            .iter()
            .find(|(c, _)| c.id == id)
            .map(|(_, others)| others.clone())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for (group, g) in TABLE.iter().enumerate() {
        let mut mine = actions()
            .iter()
            .filter(|a| a.group == group && !a.paired)
            .peekable();
        if mine.peek().is_none() {
            continue;
        }
        out.push(BindRow {
            heading: true,
            does: text(g.title).into(),
            ..Default::default()
        });
        for a in mine {
            let mut others = also_of(a.id);
            let (id2, keys2) = match a.second {
                Some(id) => {
                    for o in also_of(id) {
                        if !others.contains(&o) {
                            others.push(o);
                        }
                    }
                    (id, said(map.chord_of_id(id), mac))
                }
                None => ("", String::new()),
            };
            // The row's own partner on the chord: red, and no words,
            // which would be the row's own.
            let warn = !others.is_empty();
            others.retain(|o| a.second.is_none() || *o != a.does[0]);
            let also = if others.is_empty() {
                String::new()
            } else {
                let others: Vec<String> = others.iter().map(|o| text(o)).collect();
                // Keys sheet: a second key for the same action
                platform(&tr!("Also: {}", others.join("; ")), mac)
            };
            out.push(BindRow {
                heading: false,
                id: a.id.into(),
                keys: said(map.chord_of(a), mac).into(),
                id2: id2.into(),
                keys2: keys2.into(),
                does: platform(&does_of(a), mac).into(),
                also: also.into(),
                warn,
            });
        }
    }
    out
}

/// The guide, at the version this build is: the Keys chapter and the
/// rest, as the release it came from has them.
pub(crate) fn guide_url() -> String {
    format!(
        "{}/blob/v{}/docs/user-guide.md",
        env!("CARGO_PKG_REPOSITORY"),
        env!("CARGO_PKG_VERSION")
    )
}

/// Put the sheet's rows up for `query`.
fn show(app: &App, query: &str) {
    let rows = rows(query, cfg!(target_os = "macos"));
    app.set_keys_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
}

/// Put the Settings sheet's Keys rows up, as bound now.
fn show_bindings(app: &App) {
    let rows = with_map(|m| binding_rows(m, cfg!(target_os = "macos")));
    app.set_key_bindings(ModelRc::from(Rc::new(VecModel::from(rows))));
}

/// Each conflict, to the log.
fn log_conflicts(map: &Keymap) {
    for (a, others) in map.conflicts(cfg!(target_os = "macos")) {
        tracing::warn!(
            "keys: {} is {} and also {}",
            map.chord_of(a),
            a.id,
            others.join("; ")
        );
    }
}

/// The bindings settings.json keeps, put in force and on both sheets.
pub(crate) fn load(app: &App, saved: &BTreeMap<String, String>) {
    let map = Keymap::from_saved(saved);
    if !map.moved.is_empty() {
        tracing::info!("keys: {:?} rebound in settings.json", map.saved());
    }
    log_conflicts(&map);
    KEYMAP.with(|m| *m.borrow_mut() = map);
    show(app, "");
    show_bindings(app);
}

/// The bindings changed from the Settings sheet: logged, written to
/// settings.json at once, and both sheets made again.
fn changed(app: &App, state: &Rc<Shared>) {
    let saved = with_map(|m| {
        log_conflicts(m);
        m.saved()
    });
    crate::panel::prefs::keep(&state.borrow(), |s| s.keys = saved);
    show(app, "");
    show_bindings(app);
}

/// The Settings sheet's Keys section: a press taken as an action's
/// new chord, and Reset to defaults.
pub(crate) fn install_sheet(app: &App, state: &Rc<Shared>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        // True when the press ends the listening: a chord, taken or
        // not; false for a modifier alone, which waits on.
        // Backspace or Delete alone binds the action to no key.
        app.on_key_rebound(move |id, text, ctrl, shift, alt, meta| {
            let n = name(&text, ctrl, shift, alt, meta);
            if n.is_empty() {
                return false;
            }
            let Some(app) = app_weak.upgrade() else {
                return true;
            };
            let none = n == "Backspace" || n == "Delete";
            let taken = KEYMAP.with(|m| {
                let mut m = m.borrow_mut();
                if none { m.clear(&id) } else { m.set(&id, &n) }
            });
            if taken {
                if none {
                    tracing::info!("keys: {id} bound to no key");
                } else {
                    tracing::info!("keys: {id} bound to {n}");
                }
                changed(&app, &state);
            }
            true
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_keys_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            KEYMAP.with(|m| m.borrow_mut().reset());
            tracing::info!("keys: every binding back to its default");
            changed(&app, &state);
        });
    }
}

/// The handler's question, the sheet's search and the Help menu's
/// guide. Wanted by every window, a test's bare one too: without
/// [`chord`] no key does anything. The bindings start at the
/// defaults; [`load`] puts settings.json's in.
pub(crate) fn install(app: &App) {
    KEYMAP.with(|m| m.borrow_mut().reset());
    {
        // The Loupe's keys whenever the grid (the lightbox's too) is
        // not up; the brush's as the handler's tools have it, in
        // Develop with the brush or a repair in hand.
        let app_weak = app.as_weak();
        app.on_key_chord(move |text, ctrl, shift, alt, meta| {
            let view = app_weak.upgrade().map_or(
                View {
                    loupe: true,
                    brush: false,
                },
                |a| View {
                    loupe: !a.get_grid_open(),
                    brush: !a.get_grid_open()
                        && !a.get_cull_mode()
                        && (a.get_placing() == "Brush" || !a.get_retouch_mode().is_empty()),
                },
            );
            chord(&text, ctrl, shift, alt, meta, view).into()
        });
    }
    show(app, "");
    show_bindings(app);
    {
        // A sheet's focus going round its ends: Slint has no focus
        // move of its own for the window's code to call, only the
        // walk a Tab or a Shift+Tab starts.
        let app_weak = app.as_weak();
        app.on_focus_step(move |forward| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let text: SharedString = if forward { Key::Tab } else { Key::Backtab }.into();
            app.window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.clone() });
            app.window()
                .dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_keys_searched(move |query| {
            if let Some(app) = app_weak.upgrade() {
                show(&app, &query);
            }
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_guide_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let url = guide_url();
            tracing::info!("help: opening {url}");
            app.set_status(tr!("the guide is open in the browser").into());
            // As Report a problem's: never a browser from a test.
            if cfg!(test) {
                return;
            }
            let app_weak = app.as_weak();
            std::thread::spawn(move || {
                if let Err(e) = crate::web::open_url(&url) {
                    tracing::warn!("help: no browser would open the guide: {e}");
                    let _ = app_weak.upgrade_in_event_loop(move |app| {
                        app.set_status(
                            tr!("no browser would open: the guide is at {url}", url = url).into(),
                        );
                    });
                }
            });
        });
    }
}

#[cfg(test)]
mod tests;
