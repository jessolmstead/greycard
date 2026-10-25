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

use crate::{App, BindRow, KeyRow, State};

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
    }
}

/// The pointer's own, which the sheet lists among the keys since a
/// Lightroom hand looks for them there: not chords, and bound by the
/// strip's and the grid's cells rather than the key handler.
const POINTER: &[Binding] = &[
    line(
        &[],
        "Ctrl+click: Add a frame to the selection, or take it out",
    ),
    line(
        &[],
        "Shift+click: Select every frame from the open one to this one (with Ctrl, added)",
    ),
];

/// Every key the window binds, grouped as the guide's Keys chapter
/// groups them.
pub(crate) const TABLE: &[Group] = &[
    Group {
        title: "Moving around",
        lines: &[
            shown(&["Left", "Right"], "← →", "Previous or next frame"),
            shown(
                &["Shift+Left", "Shift+Right"],
                "Shift+← →",
                "Previous or next frame, added to the selection",
            ),
            shown(
                &["Up", "Down", "Shift+Up", "Shift+Down"],
                "↑ ↓",
                "In the grid, the frame above or below (with Shift, added)",
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
                "The first or the last frame (with Shift, added)",
            ),
            act("select-all", &["Ctrl+A"], "Select every frame shown"),
        ],
    },
    Group {
        title: "Views",
        lines: &[
            act(
                "library",
                &["G"],
                "Library (the grid), and back to the Loupe as it was left",
            ),
            act(
                "loupe",
                &["E"],
                "The Loupe (one frame), in the mode it was left in: Cull or Develop",
            ),
            act("develop", &["D"], "Develop: the Loupe with the edit panel"),
            act("lightbox", &["L"], "The lightbox, and back"),
            act(
                "overlays",
                &["J"],
                "In the lightbox, the rating, flag, and label on the tiles, or not",
            ),
            act(
                "cull",
                &["C"],
                "Cull; in the Loupe, Cull or Develop in turn",
            ),
            act("compare", &["V"], "In Cull, compare two or four frames"),
            shown(
                &["Return"],
                "Enter",
                "Open the frame chosen in the grid or the lightbox; from Cull, Develop",
            ),
            shown(
                // The grid's with Ctrl too, as a browser's zoom is.
                &["+", "=", "-", "_", "Ctrl++", "Ctrl+=", "Ctrl+-", "Ctrl+_"],
                "+ −",
                "Bigger or smaller tiles in the grid; fewer or more across the lightbox",
            ),
            line(
                &["Tab"],
                "Hide the panels and the filmstrip, and bring them back (in the grid, the left pane)",
            ),
            shown(
                &["F6", "F7", "F8"],
                "F6, F7, F8",
                "The filmstrip, the left pane or the develop panel alone",
            ),
            act("zoom-1to1", &["Space"], "Fit or 1:1"),
            act("zoom-3to1", &["Z"], "Fit or 3:1"),
            act(
                "mat",
                &["F"],
                "A mat of the canvas around the picture at Fit, or none",
            ),
            act(
                "overlays",
                &["J"],
                "In the Loupe, shadow and highlight clipping warnings",
            ),
            act("proof", &["S"], "Soft proof (in the grid, a stack: below)"),
        ],
    },
    Group {
        title: "Rating and sorting",
        lines: &[
            shown(
                &["1", "2", "3", "4", "5", "0"],
                "1 to 5, 0",
                "Set a rating, or clear it",
            ),
            // With Shift too: a hand that holds it for the stars of
            // another application still flags.
            shown(
                &["P", "X", "U", "Shift+P", "Shift+X", "Shift+U"],
                "P, X, U",
                "Pick, reject, unflag",
            ),
            shown(
                &["6", "7", "8", "9"],
                "6 to 9",
                "Red, yellow, green or blue label (again to clear)",
            ),
            shown(
                &["[", "]"],
                "[ ]",
                "Turn the frame a quarter left or right; not part of the edit",
            ),
            act(
                "collection",
                &["B"],
                "Add the selected frames to a collection, or take them out",
            ),
            act("stack", &["Ctrl+G"], "Stack the selected frames"),
            act("unstack", &["Ctrl+Shift+G"], "Unstack them"),
            shown(
                &["S", "Shift+S"],
                "S, Shift+S",
                "In the grid, open or close a stack; make the frame its top",
            ),
            act("version", &["Ctrl+'"], "New version of the frame"),
            line(
                &["Delete", "Backspace"],
                "Delete the selected frames, after a confirmation",
            ),
        ],
    },
    Group {
        title: "Editing",
        lines: &[
            act("undo", &["Ctrl+Z"], "Undo"),
            shown(
                &["Ctrl+Shift+Z", "Ctrl+Y"],
                "Ctrl+Shift+Z or Ctrl+Y",
                "Redo",
            ),
            act("copy", &["Ctrl+C"], "Copy the open frame's settings"),
            act("paste", &["Ctrl+V"], "Paste them onto the selected frames"),
            act(
                "sync",
                &["Ctrl+Shift+S"],
                "Sync the selected frames to the open one",
            ),
            act("crop", &["R"], "Crop, and Done"),
            act(
                "white-balance",
                &["W"],
                "White balance from a neutral patch: the pipette",
            ),
            act(
                "linear-mask",
                &["M"],
                "New mask with a Linear, placed by the next drag",
            ),
            act(
                "radial-mask",
                &["Shift+M"],
                "New mask with a Radial, placed by the next drag",
            ),
        ],
    },
    // A focused slider's own (`EditSlider` in greycard-slint's
    // `ui/controls.slint`), not the handler's: no chords, and the
    // window has these keys back once no slider has the focus.
    Group {
        title: "Sliders",
        lines: &[
            shown(
                &[],
                "↑ ↓ ← →, + −",
                "Nudge the focused slider, or its typed value with ↑ ↓; Shift on an arrow for more, Ctrl on the slider for less, and Shift on a drag for finer",
            ),
            shown(&[], ", .", "The previous or the next slider"),
            shown(
                &[],
                "Tab, Shift+Tab",
                "In a slider's typed value, the next or the previous one",
            ),
        ],
    },
    Group {
        title: "Files, sheets and help",
        lines: &[
            shown(
                &["Ctrl+F", "/"],
                "Ctrl+F or /",
                "Filter the folder by rating, flag, label or words",
            ),
            act("open", &["Ctrl+O"], "Open a folder"),
            act("import", &["Ctrl+Shift+I"], "Import from a card"),
            act("export", &["Ctrl+Shift+E"], "Export"),
            act("settings", &["Ctrl+,"], "Settings"),
            act("keys", &["?"], "The keys sheet"),
            line(
                &["Esc"],
                "Close a sheet, put a tool down, leave the crop, let go of a set, or go back to one frame",
            ),
        ],
    },
];

/// The chords bound on one platform only: the Mac's delete key is
/// Backspace, and elsewhere Backspace is nobody's.
const MAC_ONLY: &[&str] = &["Backspace"];

/// Whether `name` is a chord the table holds on this platform, by
/// the defaults.
fn bound(name: &str, mac: bool) -> bool {
    (mac || !MAC_ONLY.contains(&name))
        && TABLE
            .iter()
            .flat_map(|g| g.lines)
            .any(|b| b.chords.contains(&name))
}

/// A line the user may rebind, once however many lines it has: its
/// name in settings.json, its default chord, which is also the name
/// the window's key handler compares it by, what its lines do, and
/// the group of its first line.
pub(crate) struct Action {
    pub(crate) id: &'static str,
    pub(crate) default: &'static str,
    pub(crate) does: Vec<&'static str>,
    pub(crate) group: usize,
}

static ACTIONS: LazyLock<Vec<Action>> = LazyLock::new(|| {
    let mut out: Vec<Action> = Vec::new();
    for (group, g) in TABLE.iter().enumerate() {
        for b in g.lines {
            let Some(id) = b.action else { continue };
            match out.iter_mut().find(|a| a.id == id) {
                Some(a) => a.does.push(b.does),
                None => out.push(Action {
                    id,
                    default: b.chords[0],
                    does: vec![b.does],
                    group,
                }),
            }
        }
    }
    out
});

/// The rebindable actions, in the table's order.
pub(crate) fn actions() -> &'static [Action] {
    &ACTIONS
}

/// Whether `chord` is an action's default: it then reaches the
/// handler only by that action, wherever the action is bound now.
fn is_default(chord: &str) -> bool {
    actions().iter().any(|a| a.default == chord)
}

/// What an action's lines say it does, in one.
fn does_of(a: &Action) -> String {
    let mut out = String::new();
    for (i, d) in a.does.iter().enumerate() {
        if i == 0 {
            out.push_str(d);
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
    /// The chord `a` is bound to.
    pub(crate) fn chord_of(&self, a: &Action) -> &str {
        self.moved.get(a.id).map_or(a.default, String::as_str)
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

    /// From settings.json's `keys`: each action to its chord, parsed;
    /// a name that is no action or a chord that does not parse is
    /// logged and left at its default.
    pub(crate) fn from_saved(saved: &BTreeMap<String, String>) -> Self {
        let mut map = Self::default();
        for (id, text) in saved {
            let taken = parse(text).is_some_and(|c| map.set(id, &c));
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

    /// What the handler reads for a press named `name`: the default
    /// of the action bound to it, the first in the table when two
    /// are; the name itself when it is one of the table's fixed
    /// chords; empty when nothing has it.
    fn resolve(&self, name: &str, mac: bool) -> String {
        if name.is_empty() {
            return String::new();
        }
        if let Some(a) = actions().iter().find(|a| self.chord_of(a) == name) {
            return a.default.to_string();
        }
        if !is_default(name) && bound(name, mac) {
            name.to_string()
        } else {
            String::new()
        }
    }

    /// A chord of the table as bound now: an action's default as
    /// its action is, any other as itself.
    fn now(&self, chord: &'static str) -> &str {
        match actions().iter().find(|a| a.default == chord) {
            Some(a) => self.chord_of(a),
            None => chord,
        }
    }

    /// Each action whose chord something else has too, with what the
    /// others do: another action's first line, or a fixed line the
    /// action takes the chord from.
    pub(crate) fn conflicts(&self, mac: bool) -> Vec<(&'static Action, Vec<&'static str>)> {
        let mut out = Vec::new();
        for a in actions() {
            let c = self.chord_of(a);
            let mut others: Vec<&'static str> = actions()
                .iter()
                .filter(|b| b.id != a.id && self.chord_of(b) == c)
                .map(|b| b.does[0])
                .collect();
            if !is_default(c) && bound(c, mac) {
                others.extend(
                    TABLE
                        .iter()
                        .flat_map(|g| g.lines)
                        .filter(|b| b.action.is_none() && b.chords.contains(&c))
                        .map(|b| b.does),
                );
            }
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
pub(crate) fn chord(text: &str, ctrl: bool, shift: bool, alt: bool, meta: bool) -> String {
    chord_on(text, ctrl, shift, alt, meta, cfg!(target_os = "macos"))
}

fn chord_on(text: &str, ctrl: bool, shift: bool, alt: bool, meta: bool, mac: bool) -> String {
    let n = name(text, ctrl, shift, alt, meta);
    with_map(|m| m.resolve(&n, mac))
}

/// A chord as the sheet writes it on this platform.
fn said(chord: &str, mac: bool) -> String {
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
    let moved = b.chords.iter().any(|c| map.now(c) != *c);
    match b.shown {
        Some(s) if !moved => {
            let s = platform(s, mac);
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
            .filter(|c| mac || !MAC_ONLY.contains(c))
            .map(|c| said(map.now(c), mac))
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
        let whole = !q.is_empty() && holds(g.title);
        let pointer = if i == 0 { POINTER } else { &[] };
        let lines: Vec<KeyRow> = g
            .lines
            .iter()
            .map(|b| (keys_of(b, map, mac), platform(b.does, mac)))
            .chain(pointer.iter().map(|b| {
                let (keys, does) = b
                    .does
                    .split_once(": ")
                    .expect("a pointer line names its click");
                (platform(keys, mac), does.to_string())
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
            keys: g.title.into(),
            does: SharedString::new(),
        });
        out.extend(lines);
    }
    out
}

/// The Settings sheet's Keys rows: each group's heading over its
/// actions, each with its chord as bound, what it does, and what else
/// has its chord ("Also: ..."), empty when nothing does.
pub(crate) fn binding_rows(map: &Keymap, mac: bool) -> Vec<BindRow> {
    let conflicts = map.conflicts(mac);
    let mut out = Vec::new();
    for (group, g) in TABLE.iter().enumerate() {
        let mut mine = actions().iter().filter(|a| a.group == group).peekable();
        if mine.peek().is_none() {
            continue;
        }
        out.push(BindRow {
            heading: true,
            does: g.title.into(),
            ..Default::default()
        });
        for a in mine {
            let also = conflicts
                .iter()
                .find(|(c, _)| c.id == a.id)
                .map(|(_, others)| platform(&format!("Also: {}", others.join("; ")), mac))
                .unwrap_or_default();
            out.push(BindRow {
                heading: false,
                id: a.id.into(),
                keys: said(map.chord_of(a), mac).into(),
                does: platform(&does_of(a), mac).into(),
                also: also.into(),
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
fn changed(app: &App, state: &Rc<RefCell<State>>) {
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
pub(crate) fn install_sheet(app: &App, state: &Rc<RefCell<State>>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        // True when the press ends the listening: a chord, taken or
        // not; false for a modifier alone, which waits on.
        app.on_key_rebound(move |id, text, ctrl, shift, alt, meta| {
            let n = name(&text, ctrl, shift, alt, meta);
            if n.is_empty() {
                return false;
            }
            let Some(app) = app_weak.upgrade() else {
                return true;
            };
            if KEYMAP.with(|m| m.borrow_mut().set(&id, &n)) {
                tracing::info!("keys: {id} bound to {n}");
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
    app.on_key_chord(|text, ctrl, shift, alt, meta| chord(&text, ctrl, shift, alt, meta).into());
    show(app, "");
    show_bindings(app);
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
            app.set_status("the guide is open in the browser".into());
            // As Report a problem's: never a browser from a test.
            if cfg!(test) {
                return;
            }
            let app_weak = app.as_weak();
            std::thread::spawn(move || {
                if let Err(e) = webbrowser::open(&url) {
                    tracing::warn!("help: no browser would open the guide: {e}");
                    let _ = app_weak.upgrade_in_event_loop(move |app| {
                        app.set_status(
                            format!("no browser would open: the guide is at {url}").into(),
                        );
                    });
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{press, window};
    use slint::Model;
    use slint::platform::WindowEvent;
    use std::collections::BTreeSet;

    /// Every chord the table holds.
    fn table_chords() -> BTreeSet<&'static str> {
        TABLE
            .iter()
            .flat_map(|g| g.lines)
            .flat_map(|b| b.chords.iter().copied())
            .collect()
    }

    /// Slint source with its comments taken out and each string
    /// literal's contents put aside: the code with every literal as
    /// `"#n"`, and the literals by `n`. A brace, a `chord == "` or an
    /// `e.text ==` in a comment or a string is then no part of the code.
    fn code_of(source: &str) -> (String, Vec<String>) {
        let mut code = String::new();
        let mut literals = Vec::new();
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '/' if chars.peek() == Some(&'/') => {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            code.push('\n');
                            break;
                        }
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    let mut last = ' ';
                    for c in chars.by_ref() {
                        if last == '*' && c == '/' {
                            break;
                        }
                        last = c;
                    }
                }
                '"' => {
                    let mut lit = String::new();
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => {
                                if let Some(n) = chars.next() {
                                    lit.push(n);
                                }
                            }
                            '"' => break,
                            c => lit.push(c),
                        }
                    }
                    code.push_str(&format!("\"#{}\"", literals.len()));
                    literals.push(lit);
                }
                c => code.push(c),
            }
        }
        (code, literals)
    }

    /// The key handler in `source`, as code ([`code_of`]): from
    /// `keys := FocusScope` to the brace that closes it.
    fn handler_in(source: &str) -> (String, Vec<String>) {
        let (code, literals) = code_of(source);
        let start = code
            .find("keys := FocusScope {")
            .expect("the window's key handler");
        let mut depth = 0;
        for (i, c) in code[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return (code[start..start + i + 1].to_string(), literals);
                    }
                }
                _ => {}
            }
        }
        panic!("the handler's braces do not close");
    }

    fn handler() -> String {
        handler_in(greycard_slint::APP_SLINT).0
    }

    /// The names a handler compares a chord with, less the empty one
    /// it turns away first.
    fn chords_in(source: &str) -> BTreeSet<String> {
        let (h, literals) = handler_in(source);
        let mut out = BTreeSet::new();
        let mut rest = h.as_str();
        while let Some(at) = rest.find("chord == \"#") {
            rest = &rest[at + "chord == \"#".len()..];
            let end = rest.find('"').expect("a closed string");
            let n: usize = rest[..end].parse().expect("a literal's number");
            if !literals[n].is_empty() {
                out.insert(literals[n].clone());
            }
            rest = &rest[end..];
        }
        out
    }

    fn handler_chords() -> BTreeSet<String> {
        chords_in(greycard_slint::APP_SLINT)
    }

    /// A comment or a string that looks like a comparison, or holds a
    /// brace, is not read as one, and does not end the handler early.
    #[test]
    fn the_handlers_reading_skips_comments_and_strings() {
        let source = r#"
            keys := FocusScope {
                // chord == "Q" } and e.text == "q"
                /* chord == "R" { */
                key-pressed(e) => {
                    let s = "chord == \"W\" } {";
                    if (chord == "G") { return accept; }
                    if (chord == "Ctrl+'") { }
                }
            }
            if (chord == "Outside") { }
        "#;
        let (code, _) = handler_in(source);
        assert!(!code.contains("e.text =="), "{code}");
        assert!(
            code.contains("return accept"),
            "the handler ends at its own brace"
        );
        let got: Vec<String> = chords_in(source).into_iter().collect();
        assert_eq!(got, ["Ctrl+'", "G"]);
    }

    #[test]
    fn every_key_the_handler_binds_is_on_the_sheet_and_every_line_is_bound() {
        let handler = handler();
        // The handler reads the press only through the table's name.
        assert!(
            !handler.contains("e.text =="),
            "the key handler compares the event's text: bind the key in keys::TABLE and compare the chord"
        );
        let table: BTreeSet<String> = table_chords().into_iter().map(String::from).collect();
        let compared = handler_chords();
        let unbound: Vec<_> = compared.difference(&table).collect();
        assert!(
            unbound.is_empty(),
            "compared in the handler, not in the table: {unbound:?}"
        );
        let idle: Vec<_> = table.difference(&compared).collect();
        assert!(
            idle.is_empty(),
            "on the sheet, never compared in the handler: {idle:?}"
        );
        // Every chord on the sheet is one the window answers, and is
        // on the sheet: each line's keys are its chords, said.
        for mac in [false, true] {
            let sheet = rows("", mac);
            for b in TABLE.iter().flat_map(|g| g.lines) {
                let keys = keys_of(b, &Keymap::default(), mac);
                assert!(
                    sheet.iter().any(
                        |r| r.keys == keys.as_str() && r.does == platform(b.does, mac).as_str()
                    ),
                    "{keys} is not on the sheet"
                );
                for c in b.chords {
                    if !mac && MAC_ONLY.contains(c) {
                        continue;
                    }
                    assert!(bound(c, mac), "{c}");
                }
            }
            // The lines and the headings, and nothing else.
            let lines = TABLE.iter().map(|g| g.lines.len()).sum::<usize>() + POINTER.len();
            assert_eq!(sheet.len(), lines + TABLE.len());
        }
    }

    #[test]
    fn a_press_is_named_as_the_table_names_it() {
        let esc = SharedString::from(Key::Escape);
        let left = SharedString::from(Key::LeftArrow);
        assert_eq!(name("g", false, false, false, false), "G");
        assert_eq!(
            name("G", false, false, false, false),
            "G",
            "Caps Lock is no Shift"
        );
        assert_eq!(name("S", true, true, false, false), "Ctrl+Shift+S");
        assert_eq!(name("s", true, true, false, false), "Ctrl+Shift+S");
        assert_eq!(name("?", false, true, false, false), "?");
        assert_eq!(name("+", false, true, false, false), "+");
        assert_eq!(
            name("1", false, true, false, false),
            "1",
            "a digit typed with Shift"
        );
        assert_eq!(name(&left, false, true, false, false), "Shift+Left");
        assert_eq!(name(&esc, false, false, false, false), "Esc");
        assert_eq!(
            name(&esc, true, true, true, false),
            "Esc",
            "Esc whatever is held"
        );
        assert_eq!(name(" ", false, false, false, false), "Space");
        assert_eq!(name("'", true, false, false, false), "Ctrl+'");
        assert_eq!(name("g", false, false, true, false), "Alt+G");
        // Bound or not.
        assert_eq!(chord_on("g", false, false, false, false, false), "G");
        assert_eq!(
            chord_on("g", false, false, true, false, false),
            "",
            "Alt+G is nobody's"
        );
        assert_eq!(chord_on("q", false, false, false, false, false), "");
        // What master took, kept: the flags with Shift, the tiles' size
        // and the ends with Ctrl.
        assert_eq!(chord_on("P", false, true, false, false, false), "Shift+P");
        assert_eq!(chord_on("=", true, false, false, false, false), "Ctrl+=");
        let home = SharedString::from(Key::Home);
        assert_eq!(
            chord_on(&home, true, false, false, false, false),
            "Ctrl+Home"
        );
        // And what is gone on purpose.
        assert_eq!(chord_on("l", false, true, false, false, false), "");
        assert_eq!(chord_on("z", true, false, true, false, false), "");
        let back = SharedString::from(Key::Backspace);
        assert_eq!(chord_on(&back, false, false, false, false, false), "");
        assert_eq!(
            chord_on(&back, false, false, false, false, true),
            "Backspace"
        );
    }

    #[test]
    fn the_sheet_says_the_platforms_modifier() {
        let find = |mac, does: &str| {
            rows("", mac)
                .into_iter()
                .find(|r| r.does == does)
                .map(|r| r.keys.to_string())
        };
        assert_eq!(find(false, "Undo").as_deref(), Some("Ctrl+Z"));
        assert_eq!(find(true, "Undo").as_deref(), Some("Cmd+Z"));
        assert_eq!(
            find(false, "Redo").as_deref(),
            Some("Ctrl+Shift+Z or Ctrl+Y")
        );
        assert_eq!(find(true, "Redo").as_deref(), Some("Cmd+Shift+Z or Cmd+Y"));
        let delete = "Delete the selected frames, after a confirmation";
        assert_eq!(find(false, delete).as_deref(), Some("Delete"));
        assert_eq!(find(true, delete).as_deref(), Some("Delete, ⌫"));
        let pointer = "Add a frame to the selection, or take it out";
        assert_eq!(find(true, pointer).as_deref(), Some("Cmd+click"));
    }

    #[test]
    fn a_search_keeps_the_lines_that_hold_it_under_their_headings() {
        let sheet = rows("stack", false);
        let words: Vec<(bool, String)> = sheet
            .iter()
            .map(|r| (r.heading, r.keys.to_string()))
            .collect();
        assert_eq!(
            words,
            [
                (true, "Views".to_string()),
                (false, "S".to_string()),
                (true, "Rating and sorting".to_string()),
                (false, "Ctrl+G".to_string()),
                (false, "Ctrl+Shift+G".to_string()),
                (false, "S, Shift+S".to_string()),
            ]
        );
        // Keys are searched too, and a heading brings its group.
        assert!(
            rows("ctrl+shift+s", false)
                .iter()
                .any(|r| r.does.contains("Sync"))
        );
        let editing = rows("editing", false);
        assert_eq!(editing.len(), 1 + TABLE[3].lines.len());
        assert!(rows("nothing binds this", false).is_empty());
    }

    fn row_count(app: &App) -> usize {
        app.get_keys_rows().row_count()
    }

    #[test]
    fn question_mark_opens_the_sheet_and_escape_closes_it() {
        let app = window(3);
        // A bare window, as a test's: the chords still answer.
        assert!(!app.get_keys_open());
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
        press(&app, "?");
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
        assert!(app.get_keys_open());
        assert_eq!(row_count(&app), rows("", cfg!(target_os = "macos")).len());
        // The search narrows it, and an emptied search brings it back.
        app.invoke_keys_searched("lightbox".into());
        assert!(row_count(&app) < 6, "{}", row_count(&app));
        app.invoke_keys_searched("".into());
        assert_eq!(row_count(&app), rows("", cfg!(target_os = "macos")).len());
        // Under the sheet the window's keys are nobody's: G does not
        // open the grid behind it.
        press(&app, "g");
        assert!(!app.get_grid_open());
        press(&app, Key::Escape);
        assert!(!app.get_keys_open());
        // And they are the window's again, once the sheet's closing
        // has handed them back.
        slint::platform::update_timers_and_animations();
        press(&app, "g");
        assert!(app.get_grid_open());
    }

    /// `key` pressed with Shift held.
    fn shifted(app: &App, key: &str) {
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
        press(app, key);
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    }

    /// A window with a frame open in Develop, as the tools' keys want.
    fn develop_window() -> (
        App,
        std::rc::Rc<std::cell::RefCell<crate::State>>,
        std::rc::Rc<crate::worker::Worker>,
    ) {
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        app.invoke_select(0);
        (app, state, worker)
    }

    /// R is the Crop button: its tab, crop mode in, and out again with
    /// the level put down, as the button's Done leaves it.
    #[test]
    fn r_takes_the_crop_in_and_out_as_its_button_does() {
        let (app, _state, _worker) = develop_window();
        assert_eq!(app.get_panel_tab(), "Develop");
        press(&app, "r");
        assert_eq!(app.get_panel_tab(), "Crop");
        assert!(app.get_crop_mode());
        app.set_level_mode(true);
        press(&app, "r");
        assert!(!app.get_crop_mode());
        assert!(!app.get_level_mode(), "put down as the button does");
        assert_eq!(app.get_panel_tab(), "Crop");
        // The button and the key are one: in by the key, out by the
        // button, which reads Done.
        press(&app, "r");
        assert!(app.get_crop_mode());
        let (at, size) = crate::testing::labeled(&app, "Done");
        crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert!(!app.get_crop_mode());
    }

    /// F puts the mat on the fit and takes it off; the settings keep
    /// it, and a zoom asked for is not touched.
    #[test]
    fn f_mats_the_fitted_picture_and_again_takes_it_off() {
        let (app, state, _worker) = develop_window();
        state.borrow_mut().develop.image_size = (6000, 4000);
        let fit = |st: &crate::State| crate::panel::viewport::effective_zoom(st, 1000, 800);
        let bare = fit(&state.borrow());
        press(&app, "f");
        assert!(state.borrow().develop.mat);
        let matted = fit(&state.borrow());
        assert!(matted < bare, "{matted} inside the mat, {bare} without");
        let mut kept = crate::settings::Settings::default();
        crate::panel::startup::take_state_choices(&mut kept, &state.borrow());
        assert!(kept.mat, "kept for the next run");
        state.borrow_mut().develop.zoom = 1.0;
        assert_eq!(fit(&state.borrow()), 1.0, "1:1 fills the view");
        state.borrow_mut().develop.zoom = 0.0;
        press(&app, "f");
        assert!(!state.borrow().develop.mat);
        assert_eq!(fit(&state.borrow()), bare);
    }

    /// W is the white balance's pipette, from any tab; again, put
    /// down. From the crop, it leaves the crop for the Develop tab.
    #[test]
    fn w_arms_the_white_balance_pipette() {
        let (app, _state, _worker) = develop_window();
        press(&app, "w");
        assert_eq!(app.get_picking(), "White");
        press(&app, "w");
        assert_eq!(app.get_picking(), "", "again, put down");
        press(&app, "r");
        assert!(app.get_crop_mode());
        press(&app, "w");
        assert_eq!(app.get_panel_tab(), "Develop");
        assert!(!app.get_crop_mode(), "the frame shown whole again");
        assert_eq!(app.get_picking(), "White");
    }

    /// M starts a mask with a Linear in hand, Shift+M one with a
    /// Radial, and the next drag places it; M or Shift+M before the
    /// drag changes the shape in hand rather than leave an empty mask
    /// behind, and the same key again puts it down.
    #[test]
    fn m_and_shift_m_start_a_mask_placed_by_the_next_drag() {
        let (app, state, _worker) = develop_window();
        let masks = |state: &std::rc::Rc<std::cell::RefCell<crate::State>>| {
            state.borrow().develop.edit.adjustments.len()
        };
        press(&app, "m");
        assert_eq!(app.get_panel_tab(), "Masks");
        assert_eq!(app.get_placing(), "Linear");
        assert!(app.get_placing_into());
        assert_eq!(masks(&state), 1);
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_dragged(600.0, 500.0);
        app.invoke_place_released();
        assert_eq!(app.get_component_kind(), "Linear");
        assert_eq!(
            state.borrow().develop.edit.adjustments[0]
                .mask
                .components
                .len(),
            1
        );

        shifted(&app, "M");
        assert_eq!(app.get_placing(), "Radial");
        assert_eq!(masks(&state), 2);
        press(&app, "m");
        assert_eq!(app.get_placing(), "Linear", "the shape in hand changed");
        assert_eq!(masks(&state), 2, "in the same mask");
        shifted(&app, "M");
        assert_eq!(app.get_placing(), "Radial");
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_dragged(600.0, 500.0);
        app.invoke_place_released();
        assert_eq!(app.get_component_kind(), "Radial");
        assert_eq!(masks(&state), 2);

        // Again before a drag: down, as the button again.
        press(&app, "m");
        assert_eq!(app.get_placing(), "Linear");
        assert_eq!(masks(&state), 3);
        press(&app, "m");
        assert_eq!(app.get_placing(), "");
    }

    /// R in the middle of a crop's drag: the crop as dragged so far is
    /// the edit, saved as the release would have, and the release
    /// after it adds nothing.
    #[test]
    fn r_in_the_middle_of_a_crop_drag_keeps_the_drag() {
        let (app, _state, _worker) = develop_window();
        let changes = std::rc::Rc::new(std::cell::Cell::new(0));
        {
            let changes = changes.clone();
            app.on_view_changed(move || changes.set(changes.get() + 1));
        }
        press(&app, "r");
        assert!(app.get_crop_mode());
        let entered = changes.get();
        app.invoke_crop_pressed(0);
        press(&app, "r");
        assert!(!app.get_crop_mode());
        assert_eq!(changes.get(), entered + 1, "the drag made the edit");
        app.invoke_crop_released();
        assert_eq!(changes.get(), entered + 1, "and the release nothing more");
        // Without a drag, leaving makes no edit.
        press(&app, "r");
        let entered = changes.get();
        press(&app, "r");
        assert_eq!(changes.get(), entered);
    }

    /// The pipette in hand, then M or R: the pipette is put down, so
    /// the next drag is the shape's or the crop's alone.
    #[test]
    fn a_tool_key_puts_the_pipette_down() {
        let (app, state, _worker) = develop_window();
        press(&app, "w");
        assert_eq!(app.get_picking(), "White");
        press(&app, "m");
        assert_eq!(app.get_picking(), "");
        assert_eq!(app.get_placing(), "Linear");
        // Placed by the drag, and only placed.
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_dragged(600.0, 500.0);
        app.invoke_place_released();
        assert_eq!(app.get_component_kind(), "Linear");
        assert_eq!(app.get_picking(), "");

        press(&app, "w");
        assert_eq!(app.get_picking(), "White");
        press(&app, "r");
        assert_eq!(app.get_picking(), "", "put down for the crop");
        assert!(app.get_crop_mode());
        press(&app, "r");
        press(&app, "m");
        assert_eq!(app.get_picking(), "");
        assert_eq!(app.get_placing(), "Linear");
        assert_eq!(state.borrow().develop.edit.adjustments.len(), 2);
    }

    /// With no frame open, the tools' keys do nothing.
    #[test]
    fn the_tool_keys_do_nothing_with_no_frame_open() {
        let app = window(0);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        assert_eq!(app.get_selected(), -1);
        press(&app, "r");
        press(&app, "w");
        press(&app, "m");
        shifted(&app, "M");
        assert!(!app.get_crop_mode());
        assert_eq!(app.get_picking(), "");
        assert_eq!(app.get_placing(), "");
        assert_eq!(app.get_panel_tab(), "Develop");
        assert!(state.borrow().develop.edit.adjustments.is_empty());
    }

    /// In the grid, the lightbox, and culling the tools' keys are
    /// nobody's.
    #[test]
    fn the_tool_keys_do_nothing_in_the_grid_or_culling() {
        let (app, state, _worker) = develop_window();
        // The tools pressed: nothing in hand, no mask, the tab as it
        // was (culling's own panel is its own tab).
        let tools_do_nothing = |app: &App| {
            let tab = app.get_panel_tab();
            for k in ["r", "w", "m"] {
                press(app, k);
            }
            shifted(app, "M");
            assert!(!app.get_crop_mode());
            assert_eq!(app.get_picking(), "");
            assert_eq!(app.get_placing(), "");
            assert_eq!(app.get_panel_tab(), tab);
            assert!(state.borrow().develop.edit.adjustments.is_empty());
        };
        press(&app, "g");
        assert!(app.get_grid_open());
        tools_do_nothing(&app);
        press(&app, "l");
        assert!(app.get_lightbox());
        tools_do_nothing(&app);
        press(&app, "l");
        press(&app, "g");
        assert!(!app.get_grid_open());
        press(&app, "c");
        assert!(app.get_cull_mode());
        tools_do_nothing(&app);
    }

    /// Every chord the table holds, and a few of the user's, read back
    /// from what they print as and from how a sheet says them.
    #[test]
    fn a_chord_parses_and_prints_both_ways() {
        for c in table_chords() {
            assert_eq!(parse(c).as_deref(), Some(c), "{c}");
            for mac in [false, true] {
                assert_eq!(
                    parse(&said(c, mac)).as_deref(),
                    Some(c),
                    "{c} on a Mac: {mac}"
                );
            }
        }
        let cases = [
            ("ctrl+r", "Ctrl+R"),
            ("Cmd+Shift+s", "Ctrl+Shift+S"),
            ("shift+ctrl+alt+K", "Ctrl+Alt+Shift+K"),
            ("Meta+F2", "Meta+F2"),
            ("Shift+←", "Shift+Left"),
            ("Enter", "Return"),
            ("Ctrl++", "Ctrl++"),
            ("+", "+"),
            ("Ctrl+'", "Ctrl+'"),
            ("space", "Space"),
            ("Ctrl+Escape", "Esc"),
        ];
        for (text, chord) in cases {
            assert_eq!(parse(text).as_deref(), Some(chord), "{text}");
        }
        for text in [
            "", "Ctrl+", "Hyper+R", "Shift+?", "Shift+1", "RR", "Ctrl+Fly",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
        assert_eq!(said("Ctrl+Shift+R", true), "Cmd+Shift+R");
        assert_eq!(said("Shift+Left", false), "Shift+←");
        assert_eq!(said("Ctrl++", true), "Cmd++");
        // A press names what parses: an F key, and nothing for a
        // modifier alone.
        let f2 = SharedString::from(Key::F2);
        assert_eq!(name(&f2, true, false, false, false), "Ctrl+F2");
        let shift = SharedString::from(Key::Shift);
        assert_eq!(name(&shift, false, true, false, false), "");
        assert!(!bindable("Esc"), "Escape cancels, and is nobody's to bind");
    }

    /// The actions are the table's: one chord a line, one default an
    /// action, none shared, and none in conflict as shipped.
    #[test]
    fn the_actions_are_the_tables_lines() {
        let lines: Vec<&Binding> = TABLE
            .iter()
            .flat_map(|g| g.lines)
            .filter(|b| b.action.is_some())
            .collect();
        for b in &lines {
            assert_eq!(b.chords.len(), 1, "{}", b.does);
            let a = actions()
                .iter()
                .find(|a| Some(a.id) == b.action)
                .expect("each line's action");
            assert_eq!(a.default, b.chords[0], "{}: one default", a.id);
            assert!(bindable(a.default) && !MAC_ONLY.contains(&a.default));
        }
        let defaults: BTreeSet<&str> = actions().iter().map(|a| a.default).collect();
        assert_eq!(defaults.len(), actions().len(), "two actions, one default");
        // J's two lines are one action.
        let j = actions().iter().find(|a| a.default == "J").unwrap();
        assert_eq!(j.does.len(), 2);
        assert!(does_of(j).contains("; in the Loupe"), "{}", does_of(j));
        assert!(Keymap::default().conflicts(false).is_empty());
        assert!(Keymap::default().conflicts(true).is_empty());
        // The sheet's rows: each action once, under the headings.
        let rows = binding_rows(&Keymap::default(), false);
        assert_eq!(rows.iter().filter(|r| !r.heading).count(), actions().len());
        assert!(rows[0].heading);
    }

    /// A rebinding moves the action's every line, on both sheets, and
    /// what the window reads; its old chord is nobody's.
    #[test]
    fn a_rebinding_moves_the_action_and_frees_its_old_chord() {
        let mut map = Keymap::default();
        assert!(map.set("proof", "Q"));
        assert!(!map.set("fly", "Q"), "no such action");
        assert!(!map.set("crop", "Shift+?"), "no such chord");
        assert_eq!(map.resolve("Q", false), "S", "the handler's name for it");
        assert_eq!(map.resolve("S", false), "", "S is free");
        assert_eq!(map.resolve("Shift+S", false), "Shift+S", "fixed");
        let sheet = rows_in(&map, "", false);
        let keys = |does: &str| {
            sheet
                .iter()
                .find(|r| r.does == does)
                .map(|r| r.keys.to_string())
        };
        assert_eq!(
            keys("Soft proof (in the grid, a stack: below)").as_deref(),
            Some("Q")
        );
        assert_eq!(
            keys("In the grid, open or close a stack; make the frame its top").as_deref(),
            Some("Q, Shift+S"),
            "S's other line goes with it"
        );
        assert!(map.set("undo", "Ctrl+U"));
        let row = binding_rows(&map, true)
            .into_iter()
            .find(|r| r.id == "undo")
            .unwrap();
        assert_eq!(row.keys, "Cmd+U", "the Mac's own name");
        // Back to the default is not moved.
        assert!(map.set("proof", "S"));
        assert!(map.set("undo", "Ctrl+Z"));
        assert_eq!(map, Keymap::default());
    }

    /// Two actions on one chord, or an action on a fixed key: each says
    /// what else has it, and the first in the table takes the press.
    #[test]
    fn a_shared_chord_is_reported() {
        let mut map = Keymap::default();
        assert!(map.set("crop", "Ctrl+Z"));
        let found = map.conflicts(false);
        let of = |id: &str| {
            found
                .iter()
                .find(|(a, _)| a.id == id)
                .map(|(_, o)| o.clone())
        };
        assert_eq!(of("crop"), Some(vec!["Undo"]));
        assert_eq!(of("undo"), Some(vec!["Crop, and Done"]));
        assert_eq!(found.len(), 2);
        assert_eq!(map.resolve("Ctrl+Z", false), "Ctrl+Z", "undo is first");
        let rows = binding_rows(&map, false);
        let also = |id: &str| rows.iter().find(|r| r.id == id).unwrap().also.to_string();
        assert_eq!(also("crop"), "Also: Undo");
        assert_eq!(also("undo"), "Also: Crop, and Done");
        assert_eq!(also("library"), "");
        // On a fixed key the action has it, and says so.
        assert!(map.set("crop", "1"));
        assert_eq!(map.resolve("1", false), "R");
        let found = map.conflicts(false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, vec!["Set a rating, or clear it"]);
        // Reset to defaults: no conflict, every press as shipped.
        map.reset();
        assert!(map.conflicts(false).is_empty());
        assert_eq!(map.resolve("R", false), "R");
        assert_eq!(map.resolve("1", false), "1");
    }

    /// Through the window: the Settings sheet's row listens for the
    /// next press, which the window's keys then go by; Escape keeps
    /// the old chord; Reset to defaults puts R back.
    /// A frame open in Develop and the Settings sheet over it, the
    /// window tall enough for the whole sheet: a row scrolled out of
    /// the view is out of the accessible tree too.
    fn settings_window() -> (
        App,
        std::rc::Rc<std::cell::RefCell<crate::State>>,
        std::rc::Rc<crate::worker::Worker>,
    ) {
        let (app, state, worker) = develop_window();
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 4000.0));
        app.set_settings_category("Keys".into());
        app.set_settings_open(true);
        (app, state, worker)
    }

    /// The sheet's categories: it opens at the one last shown, with the
    /// column holding the keys; Down and Up step it, held at the ends,
    /// and Ctrl+Tab goes round; Escape there closes the sheet and the
    /// keys are the window's again.
    #[test]
    fn the_settings_categories_step_by_the_keys() {
        let (app, _state, _worker) = develop_window();
        app.set_settings_category("Editing".into());
        app.invoke_settings_asked();
        assert_eq!(crate::testing::count_labeled(&app, "CROP"), 1);
        assert_eq!(crate::testing::count_labeled(&app, "SIDECARS"), 0);
        press(&app, Key::DownArrow);
        assert_eq!(app.get_settings_category(), "Keys");
        assert_eq!(crate::testing::count_labeled(&app, "CROP"), 0);
        assert_eq!(crate::testing::count_labeled(&app, "KEYS"), 1);
        press(&app, Key::DownArrow);
        press(&app, Key::DownArrow);
        assert_eq!(app.get_settings_category(), "About", "held at the foot");
        let ctrl = |key: Key| {
            app.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Control.into(),
            });
            press(&app, key);
            app.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Control.into(),
            });
        };
        ctrl(Key::Tab);
        assert_eq!(app.get_settings_category(), "Interface", "round to the top");
        press(&app, Key::UpArrow);
        assert_eq!(app.get_settings_category(), "Interface", "held at the top");
        assert_eq!(crate::testing::count_labeled(&app, "INTERFACE"), 1);
        // A click on a name shows its category.
        let tab = i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Library")
            .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Tab))
            .expect("the Library tab");
        tab.invoke_accessible_default_action();
        assert_eq!(app.get_settings_category(), "Library");
        assert_eq!(crate::testing::count_labeled(&app, "SIDECARS"), 1);
        press(&app, Key::Escape);
        assert!(!app.get_settings_open());
        slint::platform::update_timers_and_animations();
        press(&app, "g");
        assert!(app.get_grid_open(), "G is the window's");
        // Opened again, at the category left.
        app.set_grid_open(false);
        app.invoke_settings_asked();
        assert_eq!(app.get_settings_category(), "Library");
        assert_eq!(crate::testing::count_labeled(&app, "SIDECARS"), 1);
    }

    /// In the smallest window the fit allows, the sheet keeps Done in
    /// the window and the keys table scrolls inside its category.
    #[test]
    fn the_keys_category_scrolls_in_a_small_window() {
        use i_slint_backend_testing::ElementHandle;
        let (app, _state, _worker) = develop_window();
        app.window()
            .set_size(slint::LogicalSize::new(1024.0, 640.0));
        app.set_settings_category("Keys".into());
        app.invoke_settings_asked();
        // Every Done there is, the sheet's among them, in the window.
        assert!(crate::testing::count_labeled(&app, "Done") > 0);
        for done in ElementHandle::find_by_accessible_label(&app, "Done") {
            let foot = done.absolute_position().y + done.size().height;
            assert!(foot <= 640.0, "Done at {foot}, past the window");
        }
        // The table runs past the view: its foot is scrolled out.
        assert_eq!(crate::testing::count_labeled(&app, "Reset to defaults"), 0);
        assert!(crate::testing::count_labeled(&app, "Select every frame shown") > 0);
    }

    /// An action's chord button, not the words beside it, found
    /// afresh: a binding makes the rows again.
    fn row_of(app: &App, does: &'static str) -> i_slint_backend_testing::ElementHandle {
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        i_slint_backend_testing::ElementQuery::from_root(app)
            .match_descendants()
            .match_accessible_role(i_slint_backend_testing::AccessibleRole::Button)
            .match_predicate(move |e| e.accessible_label().as_deref() == Some(does))
            .find_first()
            .expect("the action's row")
    }

    fn row(app: &App) -> i_slint_backend_testing::ElementHandle {
        row_of(app, "Crop, and Done")
    }

    fn shown(app: &App) -> String {
        row(app).accessible_value().unwrap_or_default().to_string()
    }

    fn repeated(app: &App, key: &str) {
        app.window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
    }

    #[test]
    fn a_key_bound_in_the_settings_sheet_is_what_the_window_goes_by() {
        let (app, _state, _worker) = settings_window();
        assert_eq!(shown(&app), "R");
        row(&app).invoke_accessible_default_action();
        assert_eq!(shown(&app), "Press a key");
        // A modifier alone waits; the chord with it is taken.
        shifted(&app, "k");
        assert_eq!(shown(&app), "Shift+K");
        assert!(app.get_settings_open());
        row(&app).invoke_accessible_default_action();
        press(&app, "k");
        assert_eq!(shown(&app), "K");
        // Escape while listening keeps K and the sheet; the next one
        // closes it, and the keys are the window's again.
        row(&app).invoke_accessible_default_action();
        press(&app, Key::Escape);
        assert_eq!(shown(&app), "K");
        assert!(app.get_settings_open());
        press(&app, Key::Escape);
        assert!(!app.get_settings_open());
        slint::platform::update_timers_and_animations();

        press(&app, "r");
        assert!(!app.get_crop_mode(), "R is nobody's now");
        press(&app, "k");
        assert!(app.get_crop_mode(), "K is the crop");
        press(&app, "k");
        assert!(!app.get_crop_mode());
        assert!(
            rows("", false)
                .iter()
                .any(|r| r.keys == "K" && r.does == "Crop, and Done"),
            "the keys sheet shows the binding"
        );
        // A modifier alone does not end the listening.
        let shift = SharedString::from(Key::Shift);
        assert!(!app.invoke_key_rebound("crop".into(), shift, false, true, false, false));

        app.invoke_keys_reset();
        press(&app, "k");
        assert!(!app.get_crop_mode());
        press(&app, "r");
        assert!(app.get_crop_mode());
    }

    /// The sheet closed while a row listens (Done, a click beside it,
    /// or from Rust): the keys are the window's again.
    #[test]
    fn closing_the_sheet_while_a_row_listens_gives_the_keys_back() {
        let (app, _state, _worker) = settings_window();
        row(&app).invoke_accessible_default_action();
        assert_eq!(shown(&app), "Press a key");
        app.set_settings_open(false);
        slint::platform::update_timers_and_animations();
        press(&app, "g");
        assert!(app.get_grid_open(), "G is the window's");
    }

    /// Reset to defaults clicked while a row listens: the listening
    /// ends, and Escape closes the sheet.
    #[test]
    fn reset_while_a_row_listens_gives_the_keys_back() {
        let (app, _state, _worker) = settings_window();
        row(&app).invoke_accessible_default_action();
        press(&app, "k");
        assert_eq!(shown(&app), "K");
        row(&app).invoke_accessible_default_action();
        // The sheet's last button above Done: below the last row.
        let last = row_of(&app, "The keys sheet").absolute_position().y;
        let reset =
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "PanelButton")
                .filter(|b| b.absolute_position().y > last)
                .min_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y))
                .expect("the reset");
        let (at, size) = (reset.absolute_position(), reset.size());
        crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(shown(&app), "R");
        press(&app, Key::DownArrow);
        assert_eq!(app.get_settings_category(), "About", "the column's again");
        press(&app, Key::Escape);
        assert!(!app.get_settings_open());
        slint::platform::update_timers_and_animations();
        press(&app, "g");
        assert!(app.get_grid_open());
    }

    /// The key being bound, held: its repeats are neither a second
    /// chord nor the window's, until a fresh press.
    #[test]
    fn a_held_key_binds_once_and_does_nothing_more() {
        let (app, _state, _worker) = settings_window();
        // Open a folder, a key the window answers whatever is open.
        let asked = std::rc::Rc::new(std::cell::Cell::new(0));
        {
            let asked = asked.clone();
            app.on_open_folder(move || asked.set(asked.get() + 1));
        }
        let open_folder = |app: &App| {
            row_of(app, "Open a folder")
                .accessible_value()
                .unwrap_or_default()
                .to_string()
        };
        row_of(&app, "Open a folder").invoke_accessible_default_action();
        repeated(&app, "j");
        assert_eq!(open_folder(&app), "Press a key", "a repeat is no chord");
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: "k".into() });
        assert_eq!(open_folder(&app), "K");
        // K still held as Escape closes the sheet: its repeats reach
        // the window, which knows them for the chord just bound.
        press(&app, Key::Escape);
        assert!(!app.get_settings_open());
        slint::platform::update_timers_and_animations();
        for _ in 0..3 {
            repeated(&app, "k");
        }
        assert_eq!(asked.get(), 0, "the repeats are nobody's");
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text: "k".into() });
        press(&app, "k");
        assert_eq!(asked.get(), 1, "a fresh press opens one");
    }

    /// A chord bound, or Escape while listening: the focus is the
    /// column's again, so Up and Down step the categories and Ctrl+Tab
    /// goes round; a repeat of the chord just bound steps nothing.
    #[test]
    fn the_column_has_the_keys_once_a_chord_is_bound() {
        let (app, _state, _worker) = settings_window();
        let ctrl = |key: Key| {
            app.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Control.into(),
            });
            press(&app, key);
            app.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Control.into(),
            });
        };
        row(&app).invoke_accessible_default_action();
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::DownArrow.into(),
        });
        assert_eq!(shown(&app), "↓");
        for _ in 0..3 {
            repeated(&app, &SharedString::from(Key::DownArrow));
        }
        assert_eq!(
            app.get_settings_category(),
            "Keys",
            "the repeats step nothing"
        );
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::DownArrow.into(),
        });
        press(&app, Key::DownArrow);
        assert_eq!(app.get_settings_category(), "About");
        press(&app, Key::UpArrow);
        assert_eq!(app.get_settings_category(), "Keys");
        row(&app).invoke_accessible_default_action();
        press(&app, Key::Escape);
        assert_eq!(shown(&app), "↓");
        assert!(app.get_settings_open());
        ctrl(Key::Tab);
        assert_eq!(app.get_settings_category(), "About");
    }

    /// A click on the card's blank leaves a listening row listening,
    /// and Ctrl+Tab from a category's rows steps the categories.
    #[test]
    fn a_click_on_the_card_keeps_the_listening() {
        let (app, _state, _worker) = settings_window();
        row(&app).invoke_accessible_default_action();
        // Below the column's last name, on the card.
        let about = i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "About")
            .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Tab))
            .expect("the About tab");
        let (at, size) = (about.absolute_position(), about.size());
        crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height + 40.0);
        assert_eq!(shown(&app), "Press a key");
        press(&app, "k");
        assert_eq!(shown(&app), "K");
        let ctrl = |key: Key, shift: bool| {
            app.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Control.into(),
            });
            if shift {
                shifted(&app, &SharedString::from(key));
            } else {
                press(&app, key);
            }
            app.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Control.into(),
            });
        };
        // Tab from the column into the rows, then Ctrl+Shift+Tab.
        press(&app, Key::Tab);
        ctrl(Key::Tab, true);
        assert_eq!(app.get_settings_category(), "Editing");
        // The focus left the hidden row for the column: the keys go on.
        ctrl(Key::Tab, false);
        assert_eq!(app.get_settings_category(), "Keys");
        press(&app, Key::Escape);
        assert!(!app.get_settings_open());
    }

    /// Escape in the crop puts down what is in hand first, one thing a
    /// press: a typed value, the Draw, the level; then it leaves the
    /// crop, as Done does. In the grid it is the grid's.
    #[test]
    fn escape_leaves_the_crop_once_nothing_is_in_hand() {
        let (app, _state, _worker) = develop_window();
        press(&app, "r");
        assert!(app.get_crop_mode());
        let (at, size) = crate::testing::labeled(&app, "Angle value");
        crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        press(&app, Key::Escape);
        assert!(app.get_crop_mode(), "the typed value went first");
        app.set_crop_draw_armed(true);
        press(&app, Key::Escape);
        assert!(!app.get_crop_draw_armed());
        assert!(app.get_crop_mode(), "the Draw went next");
        app.set_level_mode(true);
        press(&app, Key::Escape);
        assert!(!app.get_level_mode());
        assert!(app.get_crop_mode(), "then the level");
        press(&app, Key::Escape);
        assert!(!app.get_crop_mode(), "then the crop");
        assert_eq!(app.get_panel_tab(), "Crop");
        // Out of the crop, Escape is nothing more here.
        press(&app, Key::Escape);
        assert!(!app.get_crop_mode());
        // The grid over a crop left on: its Escape, back to the loupe.
        press(&app, "r");
        app.set_grid_open(true);
        press(&app, Key::Escape);
        assert!(!app.get_grid_open());
        assert!(app.get_crop_mode(), "the grid's Escape");
    }

    /// A held Escape is one press: leaving the crop, its repeats do not
    /// go on to stop an export or let go of a set.
    #[test]
    fn a_held_escape_leaves_the_crop_and_does_nothing_more() {
        use std::cell::Cell;
        use std::rc::Rc;
        let (app, _state, _worker) = develop_window();
        let (stops, collapses) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        {
            let stops = stops.clone();
            app.on_export_stop(move || stops.set(stops.get() + 1));
            let collapses = collapses.clone();
            app.on_set_collapsed(move || collapses.set(collapses.get() + 1));
        }
        press(&app, "r");
        app.set_export_running(true);
        app.set_set_count(2);
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Escape.into(),
        });
        for _ in 0..2 {
            app.window().dispatch_event(WindowEvent::KeyPressRepeated {
                text: Key::Escape.into(),
            });
        }
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Escape.into(),
        });
        assert!(!app.get_crop_mode());
        assert_eq!(stops.get(), 0, "the export runs on");
        assert_eq!(collapses.get(), 0, "the set is held");
        // A press of its own is the export's.
        press(&app, Key::Escape);
        assert_eq!(stops.get(), 1);
    }

    /// The custom aspect's box taken away under its focus, by a frame
    /// with another aspect: the picture takes the keys at a crossing
    /// again.
    #[test]
    fn a_custom_aspect_box_taken_away_hands_the_keys_back() {
        use crate::testing::{click, labeled, type_text};
        let (app, state, _worker) = develop_window();
        press(&app, "r");
        app.set_aspect_name("Custom".into());
        let (at, size) = labeled(&app, "Custom aspect");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(app.global::<crate::Typing>().get_boxes(), 1);
        app.invoke_select(1);
        assert_ne!(app.get_aspect_name(), "Custom");
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(700.0, 400.0),
        });
        assert_eq!(app.global::<crate::Typing>().get_boxes(), 0);
        type_text(&app, "2");
        assert_eq!(state.borrow().sidecars[1].meta.rating, 2, "rated");
    }

    /// An aspect changed from the Rust side under another box (the
    /// filter's words moving the frame, say) leaves that box the keys.
    #[test]
    fn an_aspect_change_leaves_another_box_its_keys() {
        use crate::testing::type_text;
        let (app, _state, _worker) = develop_window();
        app.set_aspect_name("1:1".into());
        app.set_panel_tab("Crop".into());
        press(&app, "/");
        slint::platform::update_timers_and_animations();
        type_text(&app, "a");
        app.set_aspect_name("Free".into());
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        assert_eq!(app.global::<crate::Typing>().get_boxes(), 1);
        type_text(&app, "b");
        assert_eq!(app.get_filter_text(), "ab");
    }

    /// Each of the panel's text boxes keeps the keys while the pointer
    /// crosses the picture: a digit is typed, not a rating. Escape
    /// gives them back to the window.
    #[test]
    fn a_text_box_keeps_the_keys_over_the_picture() {
        use crate::testing::{click, labeled, type_text};
        let (app, state, _worker) = develop_window();
        let rating = |state: &std::rc::Rc<std::cell::RefCell<crate::State>>| {
            state.borrow().sidecars[0].meta.rating
        };
        let over_the_picture = |app: &App| {
            app.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(700.0, 400.0),
            });
        };
        let click_on = |app: &App, label: &str| {
            let (at, size) = labeled(app, label);
            click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        };
        let mut expected = 0;
        let mut keys_back = |app: &App, what: &str| {
            assert_eq!(rating(&state), expected, "{what}: typed, not rated");
            press(app, Key::Escape);
            expected = if expected == 3 { 4 } else { 3 };
            press(app, expected.to_string());
            assert_eq!(rating(&state), expected, "{what}: the keys are back");
        };

        // The title and the keywords, on the Develop tab.
        click_on(&app, "Frame title");
        over_the_picture(&app);
        type_text(&app, "5");
        assert_eq!(app.get_title_text(), "5");
        keys_back(&app, "the title");
        click_on(&app, "Add keywords");
        over_the_picture(&app);
        type_text(&app, "5");
        assert_eq!(app.get_keyword_text(), "5");
        keys_back(&app, "the keywords");

        // A custom aspect, in the crop: its Escape gives the keys
        // back and leaves the crop on.
        press(&app, "r");
        app.set_aspect_name("Custom".into());
        click_on(&app, "Custom aspect");
        over_the_picture(&app);
        type_text(&app, "5");
        assert!(app.get_aspect_custom().ends_with('5'));
        keys_back(&app, "the custom aspect");
        assert!(app.get_crop_mode(), "the box's Escape, not the crop's");
        press(&app, "r");

        // A mask's name.
        press(&app, "m");
        press(&app, Key::Escape);
        assert_eq!(app.get_placing(), "");
        click_on(&app, "Mask name");
        over_the_picture(&app);
        type_text(&app, "5");
        assert!(app.get_adjustment_name().ends_with('5'));
        keys_back(&app, "the mask's name");

        // The filter's words, in the CULLING section beside the
        // picture.
        app.set_panel_tab(crate::panel::cull::CULL_TAB.into());
        press(&app, "/");
        slint::platform::update_timers_and_animations();
        over_the_picture(&app);
        type_text(&app, "5");
        assert_eq!(app.get_filter_text(), "5");
        keys_back(&app, "the filter");
    }

    #[test]
    fn the_guide_is_this_releases() {
        let url = guide_url();
        assert!(
            url.starts_with("https://github.com/jessolmstead/greycard/blob/v"),
            "{url}"
        );
        assert!(url.ends_with("/docs/user-guide.md"), "{url}");
        assert!(url.contains(env!("CARGO_PKG_VERSION")));
    }
}
