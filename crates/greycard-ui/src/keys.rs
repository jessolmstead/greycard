//! The window's keys: one table, read by the dispatcher and shown on
//! the keys sheet.
//!
//! A press reaches the window's key handler (`keys := FocusScope` in
//! `ui/app.slint`), which asks [`chord`] for its name: "G",
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

use std::rc::Rc;

use slint::platform::Key;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{App, KeyRow};

/// A line of the sheet: the chords that do it, by [`chord`]'s names,
/// how the sheet writes them when the names alone would read badly,
/// and what they do.
pub(crate) struct Binding {
    pub(crate) chords: &'static [&'static str],
    /// The keys as the sheet shows them, with "Ctrl" for the platform
    /// to turn into "Cmd"; `None` writes each chord, comma between.
    pub(crate) shown: Option<&'static str>,
    pub(crate) does: &'static str,
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
        ],
    },
    Group {
        title: "Views",
        lines: &[
            line(&["G"], "Library (the grid) or Develop (one frame)"),
            line(&["L"], "The lightbox, and back"),
            line(&["C"], "Culling mode"),
            line(&["V"], "In culling, compare two or four frames"),
            shown(
                &["Return"],
                "Enter",
                "Open the frame chosen in the grid or the lightbox; leave culling",
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
            line(&["Space"], "Fit or 1:1"),
            line(&["Z"], "Fit or 3:1"),
            line(&["J"], "Shadow and highlight clipping warnings"),
            line(&["S"], "Soft proof (in the grid, a stack: below)"),
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
            line(
                &["B"],
                "Add the selected frames to a collection, or take them out",
            ),
            line(&["Ctrl+G"], "Stack the selected frames"),
            line(&["Ctrl+Shift+G"], "Unstack them"),
            shown(
                &["S", "Shift+S"],
                "S, Shift+S",
                "In the grid, open or close a stack; make the frame its top",
            ),
            line(&["Ctrl+'"], "New version of the frame"),
            line(
                &["Delete", "Backspace"],
                "Delete the selected frames, after a confirmation",
            ),
        ],
    },
    Group {
        title: "Editing",
        lines: &[
            line(&["Ctrl+Z"], "Undo"),
            shown(
                &["Ctrl+Shift+Z", "Ctrl+Y"],
                "Ctrl+Shift+Z or Ctrl+Y",
                "Redo",
            ),
            line(&["Ctrl+C"], "Copy the open frame's settings"),
            line(&["Ctrl+V"], "Paste them onto the selected frames"),
            line(
                &["Ctrl+Shift+S"],
                "Sync the selected frames to the open one",
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
            line(&["Ctrl+O"], "Open a folder"),
            line(&["Ctrl+Shift+I"], "Import from a card"),
            line(&["Ctrl+Shift+E"], "Export"),
            line(&["Ctrl+,"], "Settings"),
            line(&["?"], "This sheet"),
            line(
                &["Esc"],
                "Close a sheet, put a tool down, let go of a set, or go back to one frame",
            ),
        ],
    },
];

/// The chords bound on one platform only: the Mac's delete key is
/// Backspace, and elsewhere Backspace is nobody's.
const MAC_ONLY: &[&str] = &["Backspace"];

/// Whether `name` is a chord the window binds on this platform.
fn bound(name: &str, mac: bool) -> bool {
    (mac || !MAC_ONLY.contains(&name))
        && TABLE
            .iter()
            .flat_map(|g| g.lines)
            .any(|b| b.chords.contains(&name))
}

/// The keys Slint names by a private character, by the name a chord
/// gives them.
fn named_keys() -> [(Key, &'static str); 14] {
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
        (Key::F6, "F6"),
        (Key::F7, "F7"),
        (Key::F8, "F8"),
    ]
}

/// The name of a press: the key, after whichever of Ctrl, Alt, Meta
/// and Shift are held, in that order. A letter is upper case and says
/// its Shift, so Caps Lock is no Shift; any other character carries
/// its Shift already (a "?" is Shift and "/", a "+" Shift and "=" on
/// most boards) and does not say it again. Escape is "Esc" whatever is
/// held: it closes a sheet or puts a tool down with a hand still on a
/// modifier. Pure: the name whether or not anything binds it.
pub(crate) fn name(text: &str, ctrl: bool, shift: bool, alt: bool, meta: bool) -> String {
    let named = named_keys()
        .into_iter()
        .find(|(k, _)| SharedString::from(*k).as_str() == text)
        .map(|(_, n)| n);
    if named == Some("Esc") {
        return "Esc".to_string();
    }
    let mut chars = text.chars();
    let (key, says_shift) = match (named, chars.next(), chars.next()) {
        (Some(n), _, _) => (n.to_string(), true),
        (None, Some(' '), None) => ("Space".to_string(), true),
        (None, Some(c), None) if c.is_ascii_alphabetic() => {
            (c.to_ascii_uppercase().to_string(), true)
        }
        (None, Some(c), None) if !c.is_control() => (c.to_string(), false),
        _ => return String::new(),
    };
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
    out.push_str(&key);
    out
}

/// What the window's key handler reads: the press's name when the
/// table binds it on this platform, empty when nothing does.
pub(crate) fn chord(text: &str, ctrl: bool, shift: bool, alt: bool, meta: bool) -> String {
    chord_on(text, ctrl, shift, alt, meta, cfg!(target_os = "macos"))
}

fn chord_on(text: &str, ctrl: bool, shift: bool, alt: bool, meta: bool, mac: bool) -> String {
    let n = name(text, ctrl, shift, alt, meta);
    if bound(&n, mac) { n } else { String::new() }
}

/// A chord as the sheet writes it on this platform.
fn said(chord: &str, mac: bool) -> String {
    let key = match chord {
        "Left" => "←",
        "Right" => "→",
        "Up" => "↑",
        "Down" => "↓",
        "Return" => "Enter",
        "Backspace" => "⌫",
        other => other,
    };
    platform(key, mac)
}

/// Ctrl as the platform names it.
fn platform(text: &str, mac: bool) -> String {
    if mac {
        text.replace("Ctrl+", "Cmd+")
    } else {
        text.to_string()
    }
}

/// A line's keys as the sheet writes them.
fn keys_of(b: &Binding, mac: bool) -> String {
    match b.shown {
        Some(s) => {
            let s = platform(s, mac);
            // The Mac's own delete beside Delete.
            if mac && b.chords.contains(&"Backspace") {
                format!("{s}, ⌫")
            } else {
                s
            }
        }
        None => b
            .chords
            .iter()
            .filter(|c| mac || !MAC_ONLY.contains(c))
            .map(|c| said(c, mac))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// The sheet's rows for a search: each group's heading over its
/// lines, a line kept when its keys or its words hold `query` (case
/// aside), a group kept when a line of it is or its title holds it.
/// The pointer's two go under the first group. An empty query keeps
/// everything.
pub(crate) fn rows(query: &str, mac: bool) -> Vec<KeyRow> {
    let q = query.trim().to_lowercase();
    let holds = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
    let mut out = Vec::new();
    for (i, g) in TABLE.iter().enumerate() {
        let whole = !q.is_empty() && holds(g.title);
        let pointer = if i == 0 { POINTER } else { &[] };
        let lines: Vec<KeyRow> = g
            .lines
            .iter()
            .map(|b| (keys_of(b, mac), platform(b.does, mac)))
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

/// The handler's question, the sheet's search and the Help menu's
/// guide. Wanted by every window, a test's bare one too: without
/// [`chord`] no key does anything.
pub(crate) fn install(app: &App) {
    app.on_key_chord(|text, ctrl, shift, alt, meta| chord(&text, ctrl, shift, alt, meta).into());
    show(app, "");
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
        handler_in(include_str!("../ui/app.slint")).0
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
        chords_in(include_str!("../ui/app.slint"))
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
                let keys = keys_of(b, mac);
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
