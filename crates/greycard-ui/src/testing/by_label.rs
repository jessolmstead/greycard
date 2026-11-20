//! Window tests find their controls by element id (`Section::name`),
//! never by the words a control shows or says: a word renamed or
//! translated would fail a test about something else. A lookup by
//! words is a call that passes a string the catalog translates (one in
//! `greycard.pot`) to a test's helper: `labeled(&app, "Exposure")`,
//! `count_labeled(&app, "Reset")`, `click_labeled(&app, "Cull mode")`.
//! What is left of them is counted here, file by file, and a count may
//! only go down: a new test that finds a control by its words fails
//! this one. A file whose subject is the words themselves (what a
//! screen reader says, the catalogs) is let through whole.
//!
//! A string handed on with `.into()` or the like is data, an option's
//! key or a value set (`set_panel_tab("Crop".into())`), and is not
//! counted; nor is one inside a macro (`assert_eq!`, `tr!`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Files whose subject is the words, from `src`.
const ABOUT_THE_WORDS: &[&str] = &["i18n.rs", "spoken_names.rs", "testing/by_label.rs"];

/// The lookups by words left in each file, from `src`: lower a count
/// as a file's lookups move to ids, and never raise one.
const LEFT: &[(&str, usize)] = &[
    ("archive/rejects.rs", 1),
    ("entry/tests.rs", 2),
    ("filter/tests.rs", 2),
    ("keys/tests.rs", 31),
    ("lightbox.rs", 7),
    ("panel/browser/tests.rs", 2),
    ("panel/color/tests.rs", 5),
    ("panel/crop/tests.rs", 14),
    ("panel/cull/tests.rs", 13),
    ("panel/curve.rs", 11),
    ("panel/delete.rs", 1),
    ("panel/deliver/tests.rs", 20),
    ("panel/edit/fill_tests.rs", 1),
    ("panel/edit_in/tests.rs", 12),
    ("panel/history/tests.rs", 8),
    ("panel/import.rs", 1),
    ("panel/lightroom.rs", 1),
    ("panel/look_preview.rs", 5),
    ("panel/look_remove.rs", 2),
    ("panel/look_rename/tests.rs", 1),
    ("panel/mask/tests.rs", 34),
    ("panel/menu.rs", 4),
    ("panel/models.rs", 1),
    ("panel/order/tests.rs", 3),
    ("panel/painted.rs", 1),
    ("panel/prefs/tests.rs", 31),
    ("panel/preset_list.rs", 5),
    ("panel/queue_view.rs", 12),
    ("panel/recent.rs", 7),
    ("panel/shares.rs", 1),
    ("panel/sizes/tests.rs", 9),
    ("panel/sync/tests.rs", 25),
    ("panel/viewport/tests.rs", 14),
    ("roots/tests/lists.rs", 1),
    ("roots/tests/pane.rs", 22),
    ("roots/tests/sort.rs", 1),
    ("settings.rs", 6),
    ("tree/tests.rs", 2),
];

/// The catalog's English: every `msgid` in the template.
fn catalog_words() -> BTreeSet<String> {
    let pot = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../translations/greycard.pot");
    let text = std::fs::read_to_string(&pot).expect("the catalog's template");
    let mut words = BTreeSet::new();
    let mut current: Option<String> = None;
    let unquote = |s: &str| {
        s.trim()
            .trim_start_matches('"')
            .trim_end_matches('"')
            .replace("\\\"", "\"")
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("msgid ") {
            current = Some(unquote(rest));
        } else if line.starts_with('"')
            && let Some(c) = current.as_mut()
        {
            c.push_str(&unquote(line));
        } else if let Some(c) = current.take()
            && !c.is_empty()
        {
            words.insert(c);
        }
    }
    words
}

/// Whether `s` is words the window shows in the user's language: a
/// message of the catalog, or a slider's field, "{} value".
fn translated(words: &BTreeSet<String>, s: &str) -> bool {
    words.contains(s) || s.strip_suffix(" value").is_some_and(|w| words.contains(w))
}

/// The part of `path`'s source that is tests: all of a tests file, or
/// what follows a file's first `#[cfg(test)]`.
fn test_part(path: &Path, text: &str) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let in_tests_dir = path.components().any(|c| c.as_os_str() == "tests");
    if name == "tests.rs" || name.ends_with("_tests.rs") || in_tests_dir {
        return Some(text.to_string());
    }
    text.find("#[cfg(test)]").map(|at| text[at..].to_string())
}

/// The lookups by words in `source`: each string literal of the
/// catalog's that is a call's first argument, or its second after a
/// plain one (`app`, `&self.app`), where the call is a function's,
/// not a method's or a macro's, and the string is not handed on.
fn lookups(words: &BTreeSet<String>, source: &str) -> Vec<String> {
    let mut found = Vec::new();
    // The two lines of code before this one: a call rustfmt wraps
    // has its name and first argument there.
    let mut above: [&str; 2] = ["", ""];
    for line in source.lines() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'"' {
                i += 1;
                continue;
            }
            // The literal, to its closing quote.
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            let end = end.min(bytes.len());
            let literal = &line[start..end];
            i = end + 1;
            let after = line.get(i..).unwrap_or("");
            if after.starts_with(".into") || after.starts_with(".to_") || literal.contains("::") {
                continue;
            }
            if !translated(words, literal)
                || !is_call_argument(&format!(
                    "{}\n{}\n{}",
                    above[0],
                    above[1],
                    &line[..start - 1]
                ))
            {
                continue;
            }
            found.push(literal.to_string());
        }
        above = [above[1], line];
    }
    found
}

/// Whether what comes before a literal makes it a function call's
/// first argument, or its second after a plain one.
fn is_call_argument(before: &str) -> bool {
    let before = before.trim_end();
    let before = match before.strip_suffix(',') {
        // Past the first argument: a path or a reference to one.
        Some(rest) => rest
            .trim_end()
            .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '.')
            .trim_end_matches('&')
            .trim_end(),
        None => before,
    };
    let Some(callee) = before.strip_suffix('(') else {
        return false;
    };
    let name_start = callee
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |k| k + 1);
    let name = &callee[name_start..];
    let by = callee[..name_start].chars().last();
    !name.is_empty()
        && !matches!(by, Some('.') | Some('!'))
        && !matches!(name, "Some" | "Ok" | "Err" | "from" | "new")
}

/// Every Rust source under `dir`.
fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("the sources read") {
        let path = entry.expect("a source").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The lookups by words in each file, from `src`, but those about
/// the words.
fn counted() -> BTreeMap<String, Vec<String>> {
    let words = catalog_words();
    assert!(words.len() > 1000, "only {} messages", words.len());
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut counts = BTreeMap::new();
    for path in files {
        let name = path
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if ABOUT_THE_WORDS.contains(&name.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a source reads");
        let Some(tests) = test_part(&path, &text) else {
            continue;
        };
        let found = lookups(&words, &tests);
        if !found.is_empty() {
            counts.insert(name, found);
        }
    }
    counts
}

#[test]
fn no_new_test_finds_a_control_by_its_words() {
    let counts = counted();
    let left: BTreeMap<&str, usize> = LEFT.iter().copied().collect();
    let mut grown = Vec::new();
    let mut shrunk = Vec::new();
    for (file, found) in &counts {
        let allowed = left.get(file.as_str()).copied().unwrap_or(0);
        if found.len() > allowed {
            grown.push(format!(
                "{file}: {} lookups by words, {allowed} allowed: {found:?}",
                found.len()
            ));
        } else if found.len() < allowed {
            shrunk.push(format!("{file}: {} (was {allowed})", found.len()));
        }
    }
    for (file, allowed) in &left {
        if !counts.contains_key(*file) {
            shrunk.push(format!("{file}: 0 (was {allowed})"));
        }
    }
    let table: String = counts
        .iter()
        .map(|(f, found)| format!("    ({f:?}, {}),\n", found.len()))
        .collect();
    assert!(
        grown.is_empty(),
        "find these controls by their element ids (`testing::by_id`, \
         `testing::track_middle`, `testing::header`), giving one an id \
         in its .slint where it has none:\n{}",
        grown.join("\n")
    );
    assert!(
        shrunk.is_empty(),
        "fewer lookups by words: lower `LEFT` to match:\n{}\nthe whole table:\n{table}",
        shrunk.join("\n")
    );
}

/// The scan's own cases: what it counts and what it lets by.
#[test]
fn the_scan_counts_a_call_with_the_words() {
    let words: BTreeSet<String> = ["Exposure", "Crop", "Reset"]
        .into_iter()
        .map(String::from)
        .collect();
    let count = |s: &str| lookups(&words, s).len();
    assert_eq!(count(r#"let (at, size) = labeled(&app, "Exposure");"#), 1);
    assert_eq!(
        count(r#"crate::testing::count_labeled(&self.app, "Reset")"#),
        1
    );
    assert_eq!(count(r#"track_middle(&app, "Exposure value")"#), 1);
    assert_eq!(count(r#"m.drag("Exposure")"#), 0, "a method's");
    assert_eq!(count(r#"app.set_panel_tab("Crop".into());"#), 0, "data");
    assert_eq!(count(r#"assert_eq!(app.get_panel_tab(), "Crop");"#), 0);
    assert_eq!(count(r#"by_id(&app, "LightSection::exposure")"#), 0);
    assert_eq!(count(r#"labeled(&app, "Not a message")"#), 0);
    assert_eq!(count(r#"// labeled(&app, "Exposure")"#), 0);
    assert_eq!(
        count("let (at, size) = crate::testing::labeled(\n    &app,\n    \"Exposure\",\n);"),
        1,
        "a call rustfmt wraps"
    );
}
