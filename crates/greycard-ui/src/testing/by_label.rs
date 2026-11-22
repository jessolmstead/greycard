//! Window tests find their controls by element id (`Section::name`),
//! never by the words a control shows or says: a word renamed or
//! translated would fail a test about something else, and the suite
//! runs in any bundled language (`GREYCARD_TEST_LANG`).
//!
//! A row of a repeater shares its id with every other row, so each
//! row carries a key of data, its `accessible-id` (its place in the
//! model, a code, a path), and is found by the two together
//! (`testing::keyed`, `by_key`, `click_key`; a `Segmented` row's
//! option by the option itself, `testing::segment`). Slint draws a
//! menu's entries itself, with no id of ours: an entry is found by
//! its title as the catalog gives it in the window's language
//! (`testing::count_menu_entries`). A test whose subject is what a screen
//! reader hears finds by the catalog's words too (`testing::spoken`).
//! A label that is data, a frame's or a root's name, may be looked up
//! as it is (`testing::count_labeled(&app, "f03.CR3")`).
//!
//! This scan holds the line, over every test and the test-only
//! helpers (`testing.rs`), outside the files whose subject is the
//! words: no lookup by a screen reader's words. A helper that finds by
//! words is followed into every file that calls it. What it lets by,
//! by design: the helpers that take the catalog's `msgid` and look for
//! its words in the window's language (`spoken`, `spoken_bounds`,
//! which a test may use to find a control to click, and
//! `count_menu_entries`). What it cannot tell: data that happens to be
//! a catalog word (a mask named "Global") is counted; rename the data.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Files whose subject is the words, from `src`.
const ABOUT_THE_WORDS: &[&str] = &["i18n.rs", "spoken_names.rs", "testing/by_label.rs"];

/// The finders by a screen reader's words. A call counts unless the
/// words it is given are data: a string the catalog does not hold,
/// or words taken from the catalog (`tr!`, `i18n::text`).
const FINDERS: &[&str] = &[
    "count_labeled",
    "find_by_accessible_label",
    "match_accessible_label",
    // A sheet's last line, by the words it starts with.
    "keeps_its_last_line",
    "wheel_brings_in",
];

/// The catalog's English: every `msgid` and `msgid_plural` in the
/// template, and those with placeholders as patterns.
struct Catalog {
    words: BTreeSet<String>,
    /// A message with placeholders, its literal parts in order:
    /// `"Fold {}"` is `["Fold ", ""]`.
    templates: Vec<Vec<String>>,
}

impl Catalog {
    fn read() -> Self {
        let pot = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../translations/greycard.pot");
        let text = std::fs::read_to_string(&pot).expect("the catalog's template");
        Self::parse(&text)
    }

    fn parse(text: &str) -> Self {
        let mut words = BTreeSet::new();
        let mut current: Option<String> = None;
        let unquote = |s: &str| {
            s.trim()
                .trim_start_matches('"')
                .trim_end_matches('"')
                .replace("\\\"", "\"")
        };
        for line in text.lines() {
            let start = line
                .strip_prefix("msgid ")
                .or_else(|| line.strip_prefix("msgid_plural "));
            if let Some(rest) = start {
                if let Some(c) = current.take()
                    && !c.is_empty()
                {
                    words.insert(c);
                }
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
        if let Some(c) = current.take()
            && !c.is_empty()
        {
            words.insert(c);
        }
        let templates = words
            .iter()
            .filter_map(|w| {
                let parts = literal_parts(w)?;
                let letters = parts
                    .iter()
                    .flat_map(|p| p.chars())
                    .filter(|c| c.is_alphabetic())
                    .count();
                (letters >= 3).then_some(parts)
            })
            .collect();
        Self { words, templates }
    }

    /// Whether `s` is words the window shows in the user's language: a
    /// message of the catalog, one filled in, a slider's field
    /// ("{} value"), or such messages joined as a screen reader's
    /// label joins them ("Detail: Dehaze", "3 stars, pick, yellow").
    fn translated(&self, s: &str) -> bool {
        if self.whole(s) {
            return true;
        }
        let pieces: Vec<&str> = s.split([':', ',']).map(str::trim).collect();
        pieces.len() > 1 && pieces.iter().all(|p| self.whole(p))
    }

    fn whole(&self, s: &str) -> bool {
        !s.is_empty()
            && (self.words.contains(s)
                || s.strip_suffix(" value")
                    .is_some_and(|w| self.words.contains(w))
                || self.templates.iter().any(|t| fills(t, s)))
    }
}

/// The literal parts of a message with placeholders (`{}`, `{n}`,
/// `{name}`); `None` for one with none.
fn literal_parts(message: &str) -> Option<Vec<String>> {
    let mut parts = vec![String::new()];
    let mut rest = message;
    let mut any = false;
    while let Some(open) = rest.find('{') {
        let close = rest[open..].find('}')? + open;
        let inside = &rest[open + 1..close];
        if !inside
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return None;
        }
        parts.last_mut().unwrap().push_str(&rest[..open]);
        parts.push(String::new());
        any = true;
        rest = &rest[close + 1..];
    }
    parts.last_mut().unwrap().push_str(rest);
    any.then_some(parts)
}

/// Whether `s` is the template whose literal parts are `parts` with
/// something in each placeholder.
fn fills(parts: &[String], s: &str) -> bool {
    let (first, last) = (&parts[0], &parts[parts.len() - 1]);
    let Some(mut rest) = s.strip_prefix(first.as_str()) else {
        return false;
    };
    for middle in &parts[1..parts.len() - 1] {
        // At least a character for the placeholder before it.
        let Some(at) = rest.get(1..).and_then(|r| r.find(middle.as_str())) else {
            return false;
        };
        rest = &rest[at + 1 + middle.len()..];
    }
    rest.len() > last.len() && rest.ends_with(last.as_str())
}

/// The part of `path`'s source that is tests: all of a tests file or
/// of the test-only helpers (`testing.rs` and `testing/`, built only
/// for tests by `main.rs`, with no `#[cfg(test)]` of their own), or
/// what follows a file's first `#[cfg(test)]`.
fn test_part(path: &Path, text: &str) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let in_dir = |dir: &str| path.components().any(|c| c.as_os_str() == dir);
    let helpers = name == "testing.rs" || in_dir("testing");
    if name == "tests.rs" || name.ends_with("_tests.rs") || in_dir("tests") || helpers {
        return Some(text.to_string());
    }
    text.find("#[cfg(test)]").map(|at| text[at..].to_string())
}

/// `source` with its comment lines blanked, so their words are not
/// code.
fn without_comments(source: &str) -> String {
    source
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("//") {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The end of the string literal that opens at `start` (its quote),
/// past its closing quote.
fn literal_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() && bytes[i] != b'"' {
        i += if bytes[i] == b'\\' { 2 } else { 1 };
    }
    (i + 1).min(bytes.len())
}

/// The index past the bracket that closes the one opened just before
/// `from`, string literals skipped.
fn closing(bytes: &[u8], from: usize) -> usize {
    let mut depth = 1;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = literal_end(bytes, i);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// Each argument of a call's argument list, trimmed.
fn arguments(list: &str) -> Vec<String> {
    let bytes = list.as_bytes();
    let (mut out, mut depth, mut from, mut i) = (Vec::new(), 0, 0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = literal_end(bytes, i);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(list[from..i].trim().to_string());
                from = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let last = list[from..].trim();
    if !last.is_empty() {
        out.push(last.to_string());
    }
    out
}

/// A call of `name` in `source`: where its name starts, whether it is
/// a definition (`fn name(`) or a method's (`.name(`), and its
/// arguments.
struct Call {
    at: usize,
    defined: bool,
    method: bool,
    args: Vec<String>,
}

/// Every call of `name` in `source`: a function's, a path's or a
/// method's, never another name that ends in it.
fn calls(source: &str, name: &str) -> Vec<Call> {
    let bytes = source.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = source[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        if at > 0 && ident(bytes[at - 1]) {
            continue;
        }
        let after = source[from..].trim_start();
        if !after.starts_with('(') {
            continue;
        }
        let open = source.len() - after.len() + 1;
        let close = closing(bytes, open);
        let before = source[..at].trim_end();
        out.push(Call {
            at,
            defined: before.ends_with("fn"),
            method: before.ends_with('.'),
            args: arguments(&source[open..close.saturating_sub(1)]),
        });
    }
    out
}

/// The string literal `arg` is, `&` or not, if it is one.
fn literal(arg: &str) -> Option<&str> {
    let a = arg.trim_start_matches('&').trim();
    let inner = a.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains('"') || inner.contains("\\\"")).then_some(inner)
}

/// A function's or a `let` closure's body in a source: its name,
/// whether another file may call it (`pub`), its parameters, and
/// where its body runs.
struct Body {
    name: String,
    public: bool,
    params: Vec<String>,
    span: std::ops::Range<usize>,
}

/// The names in a parameter list, types and `mut` aside.
fn params(list: &str) -> Vec<String> {
    arguments(list)
        .iter()
        .filter_map(|p| {
            let name = p.split(':').next()?.trim();
            let name = name.strip_prefix("mut ").unwrap_or(name).trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

/// Every function and `let` closure of `source`.
fn bodies(source: &str) -> Vec<Body> {
    let bytes = source.as_bytes();
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = source[from..].find("fn ") {
        let at = from + found;
        from = at + 3;
        if at > 0 && ident(bytes[at - 1] as char) {
            continue;
        }
        let name: String = source[at + 3..].chars().take_while(|c| ident(*c)).collect();
        let Some(open_params) = source[at..].find('(').map(|o| at + o + 1) else {
            continue;
        };
        let close_params = closing(bytes, open_params);
        let Some(open) = source[at..].find('{').map(|o| at + o + 1) else {
            continue;
        };
        let line_start = source[..at].rfind('\n').map_or(0, |n| n + 1);
        out.push(Body {
            name,
            public: source[line_start..at].contains("pub"),
            params: params(&source[open_params..close_params.saturating_sub(1)]),
            span: open..closing(bytes, open),
        });
    }
    for (at, _) in source.match_indices("let ") {
        let rest = &source[at + 4..];
        let name: String = rest.chars().take_while(|c| ident(*c)).collect();
        let tail = rest[name.len()..].trim_start();
        let Some(tail) = tail.strip_prefix('=') else {
            continue;
        };
        let tail = tail.trim_start();
        let tail = tail.strip_prefix("move").unwrap_or(tail).trim_start();
        if name.is_empty() || !tail.starts_with('|') {
            continue;
        }
        let start = source.len() - tail.len();
        let bar = source[start + 1..]
            .find('|')
            .map_or(start + 1, |b| start + 1 + b);
        // The closure's body: to the `;` that ends the statement.
        let (mut depth, mut i, mut end) = (0i32, start, source.len());
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    i = literal_end(bytes, i);
                    continue;
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b';' if depth <= 0 => {
                    end = i;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        out.push(Body {
            name,
            public: false,
            params: params(&source[start + 1..bar]),
            span: start..end,
        });
    }
    out
}

/// Helpers that take the catalog's `msgid` and look for its words in
/// the window's language: right in any language, so not fenced.
/// `spoken` and `spoken_bounds` find what a screen reader hears, and a
/// test may use `spoken_bounds` to find a control to click, by
/// design; `count_menu_entries` finds a menu's entry, which has no id
/// of ours.
const BY_THE_CATALOG: &[&str] = &[
    "spoken",
    "spoken_bounds",
    "count_menu_entries",
    "menu_entries",
];

/// What reads a control's words.
const READERS: &[&str] = &["accessible_label()", "label_of("];

/// The string constants of `source`, by name.
fn consts_of(source: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (at, _) in source.match_indices("const ") {
        let rest = &source[at + 6..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let Some(value) = rest[name.len()..]
            .trim_start()
            .strip_prefix(':')
            .and_then(|r| r.split_once('='))
            .map(|(_, v)| v.trim_start())
            .and_then(|v| v.split_once(';'))
            .and_then(|(v, _)| literal(v))
        else {
            continue;
        };
        out.insert(name, value.to_string());
    }
    out
}

/// What a file's scan knows beyond the file: the finders by words the
/// test-only helpers (`testing.rs`) offer every test, their `pub`
/// ones, and the string constants in reach, by name.
#[derive(Default, Clone)]
struct Shared {
    finders: BTreeSet<String>,
    consts: BTreeMap<String, String>,
}

impl Shared {
    /// What the helpers' sources offer, a finder calling another
    /// followed until no more are found.
    fn of(helpers: &[&str]) -> Self {
        let mut shared = Shared::default();
        for source in helpers {
            shared.consts.extend(consts_of(&without_comments(source)));
        }
        loop {
            let before = shared.finders.len();
            for source in helpers {
                let source = without_comments(source);
                for (name, public) in wrappers(&source, &shared) {
                    if public {
                        shared.finders.insert(name);
                    }
                }
            }
            if shared.finders.len() == before {
                return shared;
            }
        }
    }

    /// The scope of a file: these and the file's own constants.
    fn with(&self, source: &str) -> Self {
        let mut scope = self.clone();
        scope.consts.extend(consts_of(source));
        scope
    }

    /// The words a constant named by `arg` (as `NAME` or a path to it)
    /// holds, if `arg` names one.
    fn value(&self, arg: &str) -> Option<&str> {
        let a = arg.trim_start_matches('&').trim();
        let name = a.rsplit("::").next()?;
        self.consts.get(name).map(String::as_str)
    }
}

/// Whether the argument a finder is given may be the window's words:
/// a literal the catalog holds, a constant that holds one, or anything
/// else not a literal, but for words taken from the catalog itself.
fn may_be_words(catalog: &Catalog, shared: &Shared, arg: &str) -> bool {
    if let Some(lit) = literal(arg).or_else(|| shared.value(arg)) {
        return catalog.translated(lit);
    }
    let a = arg.trim_start_matches('&').trim();
    let from_catalog = ["tr!", "crate::tr!", "crate::i18n::text", "i18n::text"];
    !from_catalog.iter().any(|f| a.starts_with(f))
}

/// The functions and closures of `source` that find by a screen
/// reader's words, and whether each is `pub`: whose body calls a
/// finder, reads a label, or calls another such, of this file or
/// shared. One that takes its words from the catalog (`tr!`,
/// `i18n::text`, or a helper of [`BY_THE_CATALOG`]) is given a
/// `msgid`, and finds in the window's language.
fn wrappers(source: &str, shared: &Shared) -> Vec<(String, bool)> {
    let bodies = bodies(source);
    let mut found: BTreeMap<String, bool> = BTreeMap::new();
    loop {
        let before = found.len();
        for body in &bodies {
            let name = body.name.as_str();
            if found.contains_key(name) || FINDERS.contains(&name) || BY_THE_CATALOG.contains(&name)
            {
                continue;
            }
            let text = &source[body.span.clone()];
            let translates = text.contains("i18n::text")
                || text.contains("tr!(")
                || BY_THE_CATALOG.iter().any(|h| !calls(text, h).is_empty());
            let finds = READERS.iter().any(|r| text.contains(r))
                || FINDERS
                    .iter()
                    .any(|f| calls(text, f).iter().any(|c| !c.defined))
                || found
                    .keys()
                    .chain(shared.finders.iter())
                    .any(|f| calls(text, f).iter().any(|c| !c.defined && !c.method));
            let finds = finds && !translates;
            if finds {
                found.insert(body.name.clone(), body.public);
            }
        }
        if found.len() == before {
            return found.into_iter().collect();
        }
    }
}

/// `text` with the arguments of calls that take words from the
/// catalog (`tr!`, `i18n::text`) blanked: those words are the
/// window's own, in its language.
fn without_catalog_words(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = text.to_string().into_bytes();
    for taker in ["tr!(", "text(", "text_in(", "text_with("] {
        for (at, _) in text.match_indices(taker) {
            let open = at + taker.len();
            let close = closing(bytes, open);
            for b in &mut out[open..close.saturating_sub(1)] {
                *b = b' ';
            }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Whether `word` stands whole in `text`, not part of a longer name.
fn names(text: &str, word: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

/// Whether `statement`, which reads a label (`subject`, a reader or a
/// name bound to one), compares it with the catalog's words outside
/// what it takes from the catalog: a literal, or a constant holding
/// one, after `==`, `!=`, `Some(`, `starts_with(`, `ends_with(`, or
/// `contains(`, or an `assert_eq!`'s or `assert_ne!`'s other side. An
/// assertion's message is not a comparison.
fn compares_words(catalog: &Catalog, scope: &Shared, statement: &str, subjects: &[&str]) -> bool {
    let text = without_catalog_words(statement);
    let words = |v: &str| {
        let v = v.trim();
        literal(v)
            .or_else(|| scope.value(v).filter(|_| !v.contains('"')))
            .is_some_and(|w| catalog.translated(w))
    };
    for macro_name in ["assert_eq!(", "assert_ne!("] {
        for (at, _) in text.match_indices(macro_name) {
            let open = at + macro_name.len();
            let close = closing(text.as_bytes(), open);
            let args = arguments(&text[open..close.saturating_sub(1)]);
            if let [a, b, ..] = args.as_slice() {
                let reads = |s: &str| {
                    subjects.iter().any(|r| {
                        if READERS.contains(r) {
                            s.contains(r)
                        } else {
                            names(s, r)
                        }
                    })
                };
                if (reads(a) && words(b)) || (reads(b) && words(a)) {
                    return true;
                }
            }
        }
    }
    let compared = |before: &str| {
        [
            "==",
            "!=",
            "Some(",
            "starts_with(",
            "ends_with(",
            "contains(",
        ]
        .iter()
        .any(|c| before.trim_end().ends_with(c))
    };
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(q) = text[i..].find('"') {
        let start = i + q;
        let end = literal_end(bytes, start);
        if compared(&text[..start])
            && catalog.translated(&text[start + 1..end.saturating_sub(1).max(start + 1)])
        {
            return true;
        }
        i = end;
    }
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    scope.consts.iter().any(|(name, value)| {
        catalog.translated(value)
            && text.match_indices(name.as_str()).any(|(at, _)| {
                let whole = !text[..at].chars().next_back().is_some_and(ident)
                    && !text[at + name.len()..].chars().next().is_some_and(ident);
                whole && compared(&text[..at])
            })
    })
}

/// The lookups by words in `source`, each said with its line: a
/// finder given words that may be the window's, but for a finder
/// passing on its own parameter (its callers are counted); a call of a
/// finder of the file's own or a shared one given the catalog's
/// words; and a label read (`accessible_label()`, `label_of`, or a
/// name bound to either) compared with the catalog's words in its
/// statement.
fn lookups(catalog: &Catalog, shared: &Shared, source: &str) -> Vec<String> {
    let source = without_comments(source);
    let scope = shared.with(&source);
    let shared = &scope;
    let line_of = |at: usize| source[..at].matches('\n').count() + 1;
    let all = bodies(&source);
    // A finder passing on its own parameter, or inside a helper that
    // finds by the catalog's words: its callers are what counts.
    let passed_on = |at: usize, arg: &str| {
        let a = arg.trim_start_matches('&').trim();
        all.iter().any(|b| {
            b.span.contains(&at)
                && (b.params.iter().any(|p| p == a) || BY_THE_CATALOG.contains(&b.name.as_str()))
        })
    };
    let mut found = BTreeMap::new();
    for finder in FINDERS {
        for call in calls(&source, finder) {
            if call.defined {
                continue;
            }
            let words = if *finder == "match_accessible_label" {
                call.args.first()
            } else {
                call.args.get(1)
            };
            if words.is_some_and(|w| may_be_words(catalog, shared, w) && !passed_on(call.at, w)) {
                found.insert(
                    line_of(call.at),
                    format!("{finder}({})", call.args.join(", ")),
                );
            }
        }
    }
    let own = wrappers(&source, shared).into_iter().map(|(name, _)| name);
    for wrapper in own
        .chain(shared.finders.iter().cloned())
        .collect::<BTreeSet<_>>()
    {
        for call in calls(&source, &wrapper) {
            if call.defined || call.method {
                continue;
            }
            let words = call.args.iter().any(|a| {
                literal(a)
                    .or_else(|| shared.value(a))
                    .is_some_and(|l| catalog.translated(l))
            });
            if words {
                found.insert(
                    line_of(call.at),
                    format!("{wrapper}({})", call.args.join(", ")),
                );
            }
        }
    }
    let lines: Vec<&str> = source.lines().collect();
    // Names bound to a label read, and the line past which each is no
    // longer followed.
    let mut bound: Vec<(String, usize)> = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        bound.retain(|(_, until)| n <= *until);
        let mut subjects: Vec<&str> = READERS
            .iter()
            .copied()
            .filter(|r| line.contains(r))
            .collect();
        let reads = !subjects.is_empty();
        subjects.extend(
            bound
                .iter()
                .map(|(name, _)| name.as_str())
                .filter(|name| names(line, name)),
        );
        if subjects.is_empty() {
            continue;
        }
        // The statement from here: to its `;`, as rustfmt wraps it.
        let mut statement = Vec::new();
        for next in lines.iter().skip(n).take(16) {
            statement.push(*next);
            if next.trim_end().ends_with(';') {
                break;
            }
        }
        let statement = statement.join("\n");
        if !found.contains_key(&(n + 1)) && compares_words(catalog, shared, &statement, &subjects) {
            found.insert(n + 1, line.trim().to_string());
        }
        if reads {
            let bind = line.trim_start().strip_prefix("let ").map(|rest| {
                let rest = rest.strip_prefix("mut ").unwrap_or(rest);
                rest.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect::<String>()
            });
            if let Some(name) = bind.filter(|b| !b.is_empty()) {
                bound.push((name, n + 12));
            }
        }
    }
    found
        .into_iter()
        .map(|(line, what)| format!("line {line}: {what}"))
        .collect()
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
/// the words; the test-only helpers' finders followed into every
/// file.
fn counted() -> BTreeMap<String, Vec<String>> {
    let catalog = Catalog::read();
    assert!(
        catalog.words.len() > 1000,
        "only {} messages",
        catalog.words.len()
    );
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut tests = Vec::new();
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
        if let Some(part) = test_part(&path, &text) {
            tests.push((name, part));
        }
    }
    let helpers: Vec<&str> = tests
        .iter()
        .filter(|(name, _)| name == "testing.rs" || name.starts_with("testing/"))
        .map(|(_, t)| t.as_str())
        .collect();
    let shared = Shared::of(&helpers);
    let mut counts = BTreeMap::new();
    for (name, part) in &tests {
        let found = lookups(&catalog, &shared, part);
        if !found.is_empty() {
            counts.insert(name.clone(), found);
        }
    }
    counts
}

#[test]
fn no_test_finds_a_control_by_its_words() {
    let found: Vec<String> = counted()
        .into_iter()
        .flat_map(|(file, lookups)| lookups.into_iter().map(move |l| format!("{file} {l}")))
        .collect();
    assert!(
        found.is_empty(),
        "find these controls by their element ids (`testing::by_id`), a \
         row of a repeater by its id and key (`testing::keyed`, giving it \
         an `accessible-id` of data in its .slint), a menu's entry by the \
         catalog's title (`testing::count_menu_entries`), or, where the \
         words are the test's subject, by the catalog's words \
         (`testing::spoken`). Data that happens to be a catalog word (a \
         mask named \"Global\") is counted too: rename the data.\n{}",
        found.join("\n")
    );
}

/// The scan's own cases: what it counts and what it lets by.
#[test]
fn the_scan_counts_a_lookup_by_words() {
    let catalog = Catalog::parse(
        "msgid \"Exposure\"\nmsgstr \"\"\n\nmsgid \"Fold {}\"\nmsgstr \"\"\n\n\
         msgid \"{n} star\"\nmsgid_plural \"{n} stars\"\nmsgstr[0] \"\"\n\n\
         msgid \"pick\"\nmsgstr \"\"\n",
    );
    let count = |s: &str| lookups(&catalog, &Shared::default(), s).len();
    assert_eq!(count(r#"count_labeled(&app, "Exposure");"#), 1);
    assert_eq!(count(r#"count_labeled(&app, "Fold day");"#), 1, "filled");
    assert_eq!(
        count(r#"count_labeled(&app, "3 stars, pick");"#),
        1,
        "joined"
    );
    assert_eq!(count(r#"count_labeled(&app, "f03.CR3");"#), 0, "a name");
    assert_eq!(
        count("ElementHandle::find_by_accessible_label(app, label)"),
        1,
        "words not known"
    );
    assert_eq!(
        count(r#"find_by_accessible_label(app, &crate::tr!("Exposure"))"#),
        0,
        "the catalog's words"
    );
    assert_eq!(count(r#".match_accessible_label("Exposure")"#), 1);
    assert_eq!(
        count(
            "fn row(app: &App, label: &str) {\n    count_labeled(app, label);\n}\nrow(&app, \"Exposure\");"
        ),
        1,
        "a finder of the file's own passes its words on; its call with the words counts"
    );
    assert_eq!(
        count("let at = |label: &str| count_labeled(&app, label);\nat(\"Exposure\");"),
        1,
        "a closure's too"
    );
    assert_eq!(
        count(r#".filter(|e| e.accessible_label().as_deref() == Some("Exposure"))"#),
        1,
        "a label compared with the words"
    );
    assert_eq!(
        count("e.accessible_label()\n    .is_some_and(|l| l.starts_with(\"Fold day\"))"),
        1,
        "a comparison rustfmt wraps"
    );
    assert_eq!(
        count(
            "let found = all(&app)\n    .into_iter()\n    .filter(|e| {\n        \
             e.accessible_label()\n            .as_deref()\n            .is_some_and(|l| {\n                \
             l == \"Exposure\"\n            })\n    })\n    .count();"
        ),
        1,
        "a comparison wrapped over many lines"
    );
    assert_eq!(
        count("let l = e.accessible_label();\nassert_eq!(l.as_deref(), Some(\"Exposure\"));"),
        1,
        "a label read in one statement and compared in the next"
    );
    assert_eq!(
        count("let l = e.accessible_label();\nassert_eq!(tab, \"Exposure\");"),
        0,
        "the next statement's words, compared with something else"
    );
    assert_eq!(
        count(
            "const WORD: &str = \"Exposure\";\n.filter(|e| e.accessible_label().as_deref() == Some(WORD))"
        ),
        1,
        "a constant that holds the words"
    );
    assert_eq!(
        count("const WORD: &str = \"Exposure\";\ncount_labeled(&app, WORD);"),
        1,
        "a constant given to a finder"
    );
    assert_eq!(
        count("const NAME: &str = \"f03.CR3\";\ncount_labeled(&app, NAME);"),
        0,
        "a constant that holds data"
    );
    assert_eq!(
        count(r#"assert_eq!(testing::label_of(&app, "LightSection::exposure"), "Exposure");"#),
        1,
        "the words `label_of` reads compared with English"
    );
    assert_eq!(
        count(
            r#"assert_eq!(label_of(&app, "KeysSheet::chip"), crate::tr!("key binding" => "Exposure"));"#
        ),
        0,
        "compared with the catalog's, a context's message too"
    );
    assert_eq!(
        count("e.accessible_label().as_deref(),\n    Some(crate::tr!(\"Exposure\").as_str())"),
        0,
        "compared with the catalog's"
    );
    assert_eq!(
        count(r#"let (at, size) = crate::testing::spoken_bounds(&app, "Exposure");"#),
        0,
        "found by the catalog's words in the window's language, by design"
    );
    assert_eq!(
        count(r#"click_segment(&app, "SettingsSheet::theme-row", "Exposure");"#),
        0,
        "an option's code"
    );
    assert_eq!(
        count(r#"by_key(&app, "LookSection::look-row", "Exposure");"#),
        0,
        "a key"
    );
    assert_eq!(count(r#"// count_labeled(&app, "Exposure")"#), 0);
    assert_eq!(
        count("let n = crate::testing::count_labeled(\n    &app,\n    \"Exposure\",\n);"),
        1,
        "a call rustfmt wraps"
    );
}

/// A finder another file defines is followed: `testing.rs`, test-only
/// with no `#[cfg(test)]` of its own, is scanned whole, and its `pub`
/// finders are counted where they are called.
#[test]
fn the_scan_follows_a_finder_into_another_file() {
    let catalog = Catalog::parse("msgid \"Exposure\"\nmsgstr \"\"\n");
    let helpers = "pub(crate) fn labeled(app: &App, label: &str) -> usize {\n    \
                   count_labeled(app, label)\n}\nfn row(app: &App, label: &str) -> usize {\n    \
                   count_labeled(app, label)\n}\n";
    assert!(
        test_part(Path::new("src/testing.rs"), helpers).is_some(),
        "testing.rs is scanned whole"
    );
    let test =
        "let n = crate::testing::labeled(&app, \"Exposure\");\nlet m = row(&app, \"Exposure\");";
    let shared = Shared::of(&[helpers]);
    assert_eq!(
        lookups(&catalog, &shared, helpers).len(),
        0,
        "they pass words on"
    );
    assert_eq!(
        lookups(&catalog, &shared, test).len(),
        1,
        "the helpers' pub finder's call counts; their private `row` is not this file's"
    );
}
