//! The folder's filter: which frames the browser lists, out of what
//! their sidecars say about them.
//!
//! This is the three-way Show grown up. That one asked one question
//! of one field — All, Picks, No rejects — and a culling pass asks
//! more than that: the four-star frames, the reds and the greens,
//! everything unflagged, the ones whose name or keywords carry a
//! word. So the filter is four tests over a frame's [`Meta`] and its
//! file name, and a frame is shown when it passes all four. The
//! three old names still parse, for the command line and for a
//! folder opened the way it was last time.
//!
//! Everything here is pure: it decides, and it never reads a file or
//! touches the window. The counts the chips carry are decided here
//! too, in one pass over the sidecars the browser has already
//! loaded, because a count that disagrees with what a click does is
//! worse than no count at all.
//!
//! The EXIF is not in a sidecar, so the facets — camera, lens, ISO,
//! focal length, day, style — are the library index's (notes §160);
//! the keyword beside them is the sidecar's and the file's kind its
//! path's, both answered in hand ([`in_hand`]). What is asked of the
//! index is decided here ([`Filter::index_filter`]) and answered in
//! `library`, and comes back as one bit a frame, [`Frame::index`]:
//! whether the index's tests pass it, or the index has no row for it
//! yet. A frame the index has not reached is shown until it has;
//! hiding it for want of a row would be the filter guessing. The
//! facets' own counts are one `GROUP BY` a facet over the frames the
//! other tests leave, which is this module's rule for a count, asked
//! of SQL.
//!
//! The text field reads the §160 grammar as well as words: a token
//! shaped like a term (`camera:R6`, `iso>=3200`, `rating>=3`) is a
//! term, anything else the word it always was. A term the sidecar
//! can answer — rating, flag, label, keyword, name — is answered
//! from the sidecar, which is fresher than the row an `index_file`
//! is still writing, and the kind from the path; the EXIF ones go to
//! the index.

use std::path::Path;

use greycard_edit::meta::{Flag, Label, Meta, STARS};
pub use greycard_library::filter::Facet;
use greycard_library::filter::{Cmp, NumberField, ParseError, Term};

use crate::i18n::text;
use crate::tr;

/// How the rating chips read: at least so many stars, or exactly so
/// many.
///
/// `AtLeast(0)` is every frame, which is why there is no third arm
/// for "any": the chip that says Any and the chip that says nothing
/// are the same chip, and one enum with one meaning is easier to be
/// right about than two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stars {
    AtLeast(u8),
    Exactly(u8),
}

impl Default for Stars {
    fn default() -> Self {
        Stars::AtLeast(0)
    }
}

impl Stars {
    /// The stars the chip stands for, whichever way it reads.
    pub fn count(self) -> u8 {
        match self {
            Stars::AtLeast(n) | Stars::Exactly(n) => n.min(STARS),
        }
    }

    pub fn is_exact(self) -> bool {
        matches!(self, Stars::Exactly(_))
    }

    /// The same count, read the other way.
    pub fn read_as(self, exact: bool) -> Stars {
        let n = self.count();
        if exact {
            Stars::Exactly(n)
        } else {
            Stars::AtLeast(n)
        }
    }

    pub fn shows(self, rating: u8) -> bool {
        match self {
            Stars::AtLeast(n) => rating >= n,
            Stars::Exactly(n) => rating == n,
        }
    }

    /// What the `n`th rating chip is called, under the reading the
    /// row is in. Under "at least" the first chip lets every frame
    /// through and says so; under "exactly" it is the unrated ones,
    /// and there is nothing else to call it but zero. The last chip
    /// is five either way, since no frame carries six.
    pub fn chip_name(n: u8, exact: bool) -> String {
        if exact {
            n.to_string()
        } else if n == 0 {
            // Filter: the rating choice that passes every frame
            tr!("Any")
        } else if n >= STARS {
            STARS.to_string()
        } else {
            format!("{n}+")
        }
    }
}

/// The filter as the settings keep it between sessions, and as a smart
/// collection's file does: the library's, since the command line
/// writes it too.
pub use greycard_library::filter::Saved;

/// The bar kept and given back: [`Saved`] is the shape on disk, and
/// the filter it stands for is this module's.
pub trait SavedBar {
    fn of(filter: &Filter) -> Saved;
    /// The filter kept, a facet this build does not know left out. ISO
    /// and focal length chips, kept by a build that had them, come back
    /// as the range's terms in the text ([`ranged_terms`]). Quiet: it is
    /// asked at every rebuild, of every smart collection.
    fn filter(&self) -> Filter;
    /// [`SavedBar::filter`] for a filter put back on the bar (the last
    /// session's, a smart collection opened), the log told once of any
    /// held-out ISO or focal value it drops.
    fn opened(&self) -> Filter;
}

/// The terms that stand for a ranged facet's chips as an older build
/// kept them: the values chosen as one range, from the lowest to the
/// highest (`iso:800` for one value, `iso>=400 iso<=1600` for more,
/// which also takes the values between them). A value that is not a
/// number is dropped. Values held out are dropped too ([`SavedBar::opened`]
/// logs them): a held-out chip let a frame with no value through, and
/// `iso!=v` would hide it, so the filter widens in plain view rather
/// than hiding frames no one asked to hide.
fn ranged_terms(facet: Facet, on: &[String]) -> Vec<String> {
    let name = facet.field();
    fn number(v: &str) -> Option<(f64, &str)> {
        v.trim().parse::<f64>().ok().map(|n| (n, v.trim()))
    }
    let mut on: Vec<(f64, &str)> = on.iter().map(String::as_str).filter_map(number).collect();
    on.sort_by(|a, b| a.0.total_cmp(&b.0));
    match (on.first(), on.last()) {
        (Some(lo), Some(hi)) if lo.0 == hi.0 => vec![format!("{name}:{}", lo.1)],
        (Some(lo), Some(hi)) => vec![format!("{name}>={}", lo.1), format!("{name}<={}", hi.1)],
        _ => Vec::new(),
    }
}

impl SavedBar for Saved {
    fn of(filter: &Filter) -> Saved {
        Saved {
            stars: filter.stars.count(),
            exact: filter.stars.is_exact(),
            flags: filter.flags.clone(),
            labels: filter.labels.clone(),
            text: filter.text.clone(),
            facets: Facet::ALL
                .iter()
                .filter(|f| !filter.chosen(**f).is_empty())
                .map(|f| (f.field().to_string(), filter.chosen(*f).to_vec()))
                .collect(),
            not_flags: filter.not_flags.clone(),
            not_labels: filter.not_labels.clone(),
            not_facets: Facet::ALL
                .iter()
                .filter(|f| !filter.held_out(**f).is_empty())
                .map(|f| (f.field().to_string(), filter.held_out(*f).to_vec()))
                .collect(),
        }
    }

    fn filter(&self) -> Filter {
        let mut filter = Filter {
            stars: Stars::AtLeast(self.stars.min(STARS)).read_as(self.exact),
            flags: self.flags.clone(),
            labels: self.labels.clone(),
            text: self.text.clone(),
            not_flags: self.not_flags.clone(),
            not_labels: self.not_labels.clone(),
            ..Filter::default()
        };
        for (field, values) in &self.facets {
            if let Some(facet) = Facet::from_field(field).filter(|f| ranged(*f).is_none()) {
                filter.facets[facet_slot(facet)] = values.clone();
            }
        }
        for (field, values) in &self.not_facets {
            if let Some(facet) = Facet::from_field(field).filter(|f| ranged(*f).is_none()) {
                filter.not_facets[facet_slot(facet)] = values.clone();
            }
        }
        // ISO and the focal length have no chips: an older build's come
        // back as the range's terms.
        let terms: Vec<String> = RANGED
            .iter()
            .flat_map(|(facet, _)| ranged_terms(*facet, &kept(&self.facets, *facet)))
            .collect();
        filter.text = append(std::mem::take(&mut filter.text), &terms);
        filter
    }

    fn opened(&self) -> Filter {
        for (facet, _) in RANGED {
            let out = kept(&self.not_facets, facet);
            if !out.is_empty() {
                tracing::info!(
                    "filter: {} values held out by an older build ({}) dropped; \
                     a range cannot leave a value out",
                    facet.field(),
                    out.join(", ")
                );
            }
        }
        self.filter()
    }
}

/// A saved filter's values for `facet`, under any of its field's names.
fn kept(map: &std::collections::BTreeMap<String, Vec<String>>, facet: Facet) -> Vec<String> {
    (map.iter())
        .filter(|(field, _)| Facet::from_field(field) == Some(facet))
        .flat_map(|(_, v)| v.iter().cloned())
        .collect()
}

/// What the browser says when the filter has hidden the whole
/// folder. A blank grid and a zero is not an explanation, and the
/// four places that can arrive at one say the same thing.
///
/// It used to read "show All to see them", which named a control:
/// All was the three-way Show's first option. It is now the first
/// rating chip and means any rating, so the way back is Clear.
pub fn nothing_shown() -> String {
    tr!("no frames pass the filter; Clear shows the folder again")
}

/// One frame as the filter reads it: where the file is, which is
/// where its name is, what its sidecar says about it, and what the
/// library index made of it.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    pub path: &'a Path,
    pub meta: &'a Meta,
    /// Whether the index's tests — the EXIF facets and the EXIF
    /// terms typed — pass this frame. True when nothing is asked of
    /// the index, and when the index has no row for the frame yet:
    /// it shows until the pass says otherwise.
    pub index: bool,
}

/// The text field, read: the words looked for as they always were,
/// the terms the sidecar answers, the terms only the index can, and
/// what did not parse.
#[derive(Debug, Clone, Default)]
pub struct Typed {
    pub words: Vec<String>,
    pub meta: Vec<Term>,
    pub index: Vec<Term>,
    pub errors: Vec<ParseError>,
}

impl Typed {
    /// Whether the words and the sidecar's terms pass this frame.
    /// Every word has to be somewhere, in the name or in one
    /// keyword, which is the search box's rule unchanged.
    fn shows(&self, path: &Path, meta: &Meta) -> bool {
        if !self.words.is_empty() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let words: Vec<String> = meta.keywords.iter().map(|k| k.to_lowercase()).collect();
            let all = self
                .words
                .iter()
                .all(|w| name.contains(w.as_str()) || words.iter().any(|k| k.contains(w.as_str())));
            if !all {
                return false;
            }
        }
        self.meta
            .iter()
            .all(|t| t.on_meta(path, meta).unwrap_or(true))
    }
}

/// Whether a facet's chips are answered by the frames in hand rather
/// than by the index, and its row counted from them: the keyword from
/// the sidecar, which is fresher than a row, and the kind from the
/// path, which every frame has before the index has reached it.
pub fn in_hand(facet: Facet) -> bool {
    matches!(facet, Facet::Keyword | Facet::Kind)
}

/// A facet's place in [`Facet::ALL`], which is its slot in
/// [`Filter::facets`] and its code on the window.
pub fn facet_slot(facet: Facet) -> usize {
    Facet::ALL
        .iter()
        .position(|f| *f == facet)
        .expect("every facet is in ALL")
}

/// What a facet's row is captioned on the bar: in English, marked for
/// the catalog, and translated where it is shown.
pub fn facet_caption(facet: Facet) -> &'static str {
    use crate::marked;
    match facet {
        // Filter bar: a facet's chip
        Facet::Camera => marked::tr!("Camera"),
        // Filter bar: a facet's chip
        Facet::Lens => marked::tr!("Lens"),
        // Filter bar: a facet's chip
        Facet::Kind => marked::tr!("Kind"),
        // Filter bar: a facet's chip
        Facet::Style => marked::tr!("Style"),
        Facet::Iso => "ISO",
        // Filter bar: a facet's chip, the focal length
        Facet::Focal => marked::tr!("Focal"),
        // Filter bar: a facet's chip, the capture day
        Facet::Date => marked::tr!("Day"),
        // Filter bar: a facet's chip
        Facet::Keyword => marked::tr!("Keyword"),
    }
}

/// The flags' and the labels' names (`Flag::name`, `Label::name`, in
/// greycard-edit), marked here for the catalog: [`Filter::describe`]
/// translates them where it says them.
const _MARK_NAMES: [&str; 9] = [
    crate::marked::tr!("Unflagged"),
    crate::marked::tr!("Pick"),
    crate::marked::tr!("Reject"),
    crate::marked::tr!("None"),
    crate::marked::tr!("Red"),
    crate::marked::tr!("Yellow"),
    crate::marked::tr!("Green"),
    crate::marked::tr!("Blue"),
    crate::marked::tr!("Purple"),
];

/// The flag and label names as a sentence says them, in lower case,
/// their own entries in the catalog (a language need not lower its
/// words as English does).
const _MARK_LOWER_NAMES: [&str; 9] = [
    crate::marked::tr!("unflagged"),
    crate::marked::tr!("pick"),
    crate::marked::tr!("reject"),
    crate::marked::tr!("none"),
    crate::marked::tr!("red"),
    crate::marked::tr!("yellow"),
    crate::marked::tr!("green"),
    crate::marked::tr!("blue"),
    crate::marked::tr!("purple"),
];

/// A facet's value as its chip says it: a focal length in
/// millimeters, a kind as a word (Raw) or a format's capitals (JPEG),
/// the rest as the index has it.
pub fn facet_chip_text(facet: Facet, label: &str) -> String {
    match facet {
        Facet::Focal => format!("{label} mm"),
        // Filter bar: the kind chip for raws
        Facet::Kind if label == greycard_library::filter::RAW => tr!("Raw"),
        Facet::Kind => label.to_uppercase(),
        _ => label.to_string(),
    }
}

/// The facets the CULLING section filters by a range rather than by a
/// chip a value, each with the number field its terms test. The range
/// reads and writes those terms in the text, so a range set is a
/// filter typed, and one typed is a range set.
pub const RANGED: [(Facet, NumberField); 2] = [
    (Facet::Iso, NumberField::Iso),
    (Facet::Focal, NumberField::Focal),
];

/// The number field a ranged facet's terms test.
pub fn ranged(facet: Facet) -> Option<NumberField> {
    RANGED.iter().find(|(f, _)| *f == facet).map(|(_, n)| *n)
}

/// The tokens of a filter's text by their byte spans, split as
/// [`greycard_library::filter::tokens`] splits them: on whitespace
/// outside double quotes.
fn spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut quoted = false;
    for (i, c) in text.char_indices() {
        if c == '"' {
            quoted = !quoted;
            start.get_or_insert(i);
        } else if c.is_whitespace() && !quoted {
            if let Some(s) = start.take() {
                out.push((s, i));
            }
        } else {
            start.get_or_insert(i);
        }
    }
    if let Some(s) = start {
        out.push((s, text.len()));
    }
    out
}

/// A token of the text as a bound on `field`: its test and value. A
/// `!=` is no bound and is left alone, as is anything else.
fn bound_on(token: &str, field: NumberField) -> Option<(Cmp, f64)> {
    if !greycard_library::filter::is_term(token) {
        return None;
    }
    let parsed = greycard_library::Filter::from_terms(&[token]).ok()?;
    match parsed.terms.as_slice() {
        [
            Term::Number {
                field: f,
                op,
                value,
            },
        ] if *f == field && *op != Cmp::Ne => Some((*op, *value)),
        _ => None,
    }
}

/// The text with the tokens `goes` picks spliced out, every other byte
/// as typed: a token goes with the gap before it, or, with no token
/// kept before it, with the gap after it.
fn cut(text: &str, goes: impl Fn(&str) -> bool) -> String {
    let spans = spans(text);
    let Some((first, last)) = spans.first().zip(spans.last()) else {
        return text.to_string();
    };
    let mut out = text[..first.0].to_string();
    let mut kept_any = false;
    let mut prev_end = first.0;
    for &(s, e) in &spans {
        if !goes(&text[s..e]) {
            if kept_any {
                out.push_str(&text[prev_end..s]);
            }
            out.push_str(&text[s..e]);
            kept_any = true;
        }
        prev_end = e;
    }
    out.push_str(&text[last.1..]);
    out
}

/// Terms written at the end of the text: after a closing quote when the
/// text ends inside an open one, so a term is never taken into a
/// phrase, and after a space unless the text ends in one.
fn append(mut text: String, terms: &[String]) -> String {
    if terms.is_empty() {
        return text;
    }
    if text.matches('"').count() % 2 == 1 {
        text.push('"');
    }
    if !text.is_empty() && !text.ends_with(char::is_whitespace) {
        text.push(' ');
    }
    text.push_str(&terms.join(" "));
    text
}

/// Whether a value passes a bound, as the index's SQL has it: a whole
/// number exactly, a focal length through its tenth of a millimeter.
fn passes(field: NumberField, op: Cmp, bound: f64, v: f64) -> bool {
    let (lo, hi) = if field.is_whole() {
        (bound, bound)
    } else {
        (bound - 0.05, bound + 0.05)
    };
    match op {
        Cmp::Eq => v >= lo && v <= hi,
        Cmp::Ne => true,
        Cmp::Ge => v >= lo,
        Cmp::Gt => v > hi,
        Cmp::Le => v <= hi,
        Cmp::Lt => v < lo,
    }
}

fn is_low(op: Cmp) -> bool {
    matches!(op, Cmp::Ge | Cmp::Gt | Cmp::Eq)
}

fn is_high(op: Cmp) -> bool {
    matches!(op, Cmp::Le | Cmp::Lt | Cmp::Eq)
}

/// Where a range's ends sit among the folder's `values` in order, for
/// the text's bounds on `field`: the low end at the lowest value every
/// low bound (`>=`, `>`, `:`) lets through, the high at the highest
/// every high bound (`<=`, `<`, `:`) does, so the handles span the
/// values the filter passes; and whether anything bounds it. A bound on
/// a value the folder lacks moves its end to the next value in, the
/// text left as typed. Bounds that pass no value put both ends on the
/// value nearest the one asked: an exact one, or else the tightest low,
/// or else the tightest high.
pub fn range_ends(text: &str, field: NumberField, values: &[f64]) -> (usize, usize, bool) {
    let top = values.len().saturating_sub(1);
    let bounds: Vec<(Cmp, f64)> = greycard_library::filter::tokens(text)
        .iter()
        .filter_map(|t| bound_on(t, field))
        .collect();
    if bounds.is_empty() || values.is_empty() {
        return (0, top, !bounds.is_empty());
    }
    let all = |want: fn(Cmp) -> bool, v: f64| {
        bounds
            .iter()
            .filter(|(op, _)| want(*op))
            .all(|(op, b)| passes(field, *op, *b, v))
    };
    let low = values.iter().position(|v| all(is_low, *v));
    let high = values.iter().rposition(|v| all(is_high, *v));
    if let (Some(l), Some(h)) = (low, high)
        && l <= h
    {
        return (l, h, true);
    }
    let of = |want: fn(Cmp) -> bool| bounds.iter().filter(move |(op, _)| want(*op)).map(|b| b.1);
    let asked = bounds
        .iter()
        .find(|(op, _)| *op == Cmp::Eq)
        .map(|b| b.1)
        .or_else(|| of(is_low).reduce(f64::max))
        .or_else(|| of(is_high).reduce(f64::min))
        .unwrap_or_default();
    let near = (0..values.len())
        .min_by(|a, b| {
            (values[*a] - asked)
                .abs()
                .total_cmp(&(values[*b] - asked).abs())
        })
        .unwrap_or(0);
    (near, near, true)
}

/// The text after a range's end was moved: the moved end's bounds on
/// the facet's field spliced out and `field>=low` or `field<=high`
/// written at the end, none when that end is at the folder's own
/// (`low` or `high` empty). The other end's bounds stay as typed;
/// an exact `field:v` bounds both ends, so it goes, with every bound on
/// the field, and both ends are written as shown. A `!=` and every
/// other byte of the text stay as typed.
pub fn with_range(text: &str, facet: Facet, moved_low: bool, low: &str, high: &str) -> String {
    let Some(field) = ranged(facet) else {
        return text.to_string();
    };
    let both = spans(text)
        .iter()
        .any(|(s, e)| bound_on(&text[*s..*e], field).is_some_and(|(op, _)| op == Cmp::Eq));
    let goes = |op: Cmp| both || if moved_low { is_low(op) } else { is_high(op) };
    let kept = cut(text, |t| bound_on(t, field).is_some_and(|(op, _)| goes(op)));
    let name = facet.field();
    let mut terms = Vec::new();
    if (moved_low || both) && !low.is_empty() {
        terms.push(format!("{name}>={low}"));
    }
    if (!moved_low || both) && !high.is_empty() {
        terms.push(format!("{name}<={high}"));
    }
    append(kept, &terms)
}

/// The text with the facet's range bounds spliced out, every other byte
/// as typed: the folder's whole range, which asks nothing.
pub fn without_range(text: &str, facet: Facet) -> String {
    let Some(field) = ranged(facet) else {
        return text.to_string();
    };
    cut(text, |t| bound_on(t, field).is_some())
}

/// What the browser shows of the folder.
///
/// Every field is a test, and a frame is shown when it passes all of
/// them. An empty set of flags or labels is every flag or every
/// label rather than none: a chip row with nothing picked is a
/// question nobody asked, and the alternative — an empty folder
/// until something is ticked — is a filter that starts by hiding
/// everything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    pub stars: Stars,
    /// Any-of. Empty is every flag.
    pub flags: Vec<Flag>,
    /// Any-of. Empty is every label, [`Label::None`] among them.
    pub labels: Vec<Label>,
    /// Matched against the file name and the keywords, a word at a
    /// time, with the §160 grammar's terms among the words. Empty is
    /// every frame.
    pub text: String,
    /// The facets' chips, by the facet's slot in [`Facet::ALL`]:
    /// any-of within a facet, as the flags and labels are, and an
    /// empty facet asks nothing. The values are the index's own
    /// ([`greycard_library::FacetCount::value`]).
    pub facets: [Vec<String>; Facet::ALL.len()],
    /// The chips held out, a Ctrl+click each: none-of, within each
    /// group, beside the any-of above, so "everything but the
    /// rejects" is one chip. A chip is on, held out, or neither.
    pub not_flags: Vec<Flag>,
    pub not_labels: Vec<Label>,
    pub not_facets: [Vec<String>; Facet::ALL.len()],
}

impl Filter {
    /// Whether this filter shows the whole folder, which is what
    /// the panel asks before it bothers rebuilding anything.
    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    /// The three names the old three-way Show went by, still spoken
    /// on the command line and in the log. Nothing else parses.
    pub const NAMES: [&'static str; 3] = ["All", "Picks", "No rejects"];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match Self::NAMES.iter().position(|n| *n == name)? {
            1 => Filter {
                flags: vec![Flag::Pick],
                ..Filter::default()
            },
            2 => Filter {
                flags: vec![Flag::None, Flag::Pick],
                ..Filter::default()
            },
            _ => Filter::default(),
        })
    }

    /// Whether the flag chips are ticked; a frame's flag passes when
    /// none of them is.
    pub fn shows_flag(&self, flag: Flag) -> bool {
        any_of(&self.flags, flag) && !self.not_flags.contains(&flag)
    }

    pub fn shows_label(&self, label: Label) -> bool {
        any_of(&self.labels, label) && !self.not_labels.contains(&label)
    }

    /// The text field read: words, the sidecar's terms, the index's
    /// terms, and the terms that did not parse. A token shaped like a
    /// term that does not parse (`rating>=9`) is kept as the word it
    /// would have been before the field took terms, and its error is
    /// said on the status line; a filter that silently asked nothing
    /// would look like the folder and not like a mistake.
    ///
    /// Words are split exactly as the search box always split them —
    /// on whitespace, quotes kept as characters — so a field with no
    /// term in it means what it meant.
    pub fn typed(&self) -> Typed {
        use greycard_library::filter::{is_term, tokens};
        let mut typed = Typed::default();
        let text = self.text.trim();
        if text.is_empty() {
            return typed;
        }
        for token in tokens(text) {
            if is_term(&token) {
                match greycard_library::Filter::from_terms(&[&token]) {
                    Ok(parsed) => {
                        for term in parsed.terms {
                            if term.is_meta() {
                                typed.meta.push(term);
                            } else {
                                typed.index.push(term);
                            }
                        }
                        continue;
                    }
                    Err(e) => typed.errors.push(e),
                }
            }
            typed
                .words
                .extend(token.to_lowercase().split_whitespace().map(str::to_string));
        }
        typed
    }

    /// The chips of one facet.
    pub fn chosen(&self, facet: Facet) -> &[String] {
        &self.facets[facet_slot(facet)]
    }

    /// The chips of one facet held out.
    pub fn held_out(&self, facet: Facet) -> &[String] {
        &self.not_facets[facet_slot(facet)]
    }

    /// Press a facet's chip: on if it was off, off if it was on or
    /// held out.
    pub fn toggle_facet(&mut self, facet: Facet, value: &str) {
        let at = facet_slot(facet);
        press(
            &mut self.facets[at],
            &mut self.not_facets[at],
            value.to_string(),
            false,
        );
    }

    /// Ctrl+click a facet's chip: held out if it was not, off if it
    /// was.
    pub fn exclude_facet(&mut self, facet: Facet, value: &str) {
        let at = facet_slot(facet);
        press(
            &mut self.facets[at],
            &mut self.not_facets[at],
            value.to_string(),
            true,
        );
    }

    /// Whether the chips answered in hand ([`in_hand`]) pass this
    /// frame, `except`'s own set aside for that facet's count: the
    /// keyword asked of the sidecar, as the text field's keyword
    /// terms are, and the kind of the path, as `kind:` is.
    pub fn shows_in_hand(&self, frame: Frame, except: Option<Facet>) -> bool {
        Facet::ALL
            .into_iter()
            .filter(|f| in_hand(*f) && Some(*f) != except)
            .all(|facet| {
                let (on, out) = (self.chosen(facet), self.held_out(facet));
                (on.is_empty()
                    || Term::Facet {
                        facet,
                        values: on.to_vec(),
                    }
                    .on_meta(frame.path, frame.meta)
                    .unwrap_or(true))
                    && Term::NotFacet {
                        facet,
                        values: out.to_vec(),
                    }
                    .on_meta(frame.path, frame.meta)
                    .unwrap_or(true)
            })
    }

    /// What the index is asked: the EXIF terms typed and every facet
    /// not answered in hand with a chip on, less `except`'s own group
    /// — which is how a facet's count sets its own row aside.
    pub fn index_filter(&self, typed: &Typed, except: Option<Facet>) -> greycard_library::Filter {
        let mut terms = typed.index.clone();
        for facet in Facet::ALL {
            if in_hand(facet) || Some(facet) == except {
                continue;
            }
            let (on, out) = (self.chosen(facet), self.held_out(facet));
            if !on.is_empty() {
                terms.push(Term::Facet {
                    facet,
                    values: on.to_vec(),
                });
            }
            if !out.is_empty() {
                terms.push(Term::NotFacet {
                    facet,
                    values: out.to_vec(),
                });
            }
        }
        greycard_library::Filter { terms }
    }

    /// Whether anything is asked of the index: without that, every
    /// frame passes it and the index need not be asked.
    pub fn asks_index(&self, typed: &Typed) -> bool {
        !typed.index.is_empty()
            || Facet::ALL.iter().any(|f| {
                !in_hand(*f) && !(self.chosen(*f).is_empty() && self.held_out(*f).is_empty())
            })
    }

    /// Whether the tests the frames in hand answer pass this frame:
    /// every group but the index's, with `except`'s chips set aside,
    /// for that facet's own count.
    pub fn shows_meta(&self, typed: &Typed, frame: Frame, except: Option<Facet>) -> bool {
        self.stars.shows(frame.meta.rating)
            && self.shows_flag(frame.meta.flag)
            && self.shows_label(frame.meta.label)
            && typed.shows(frame.path, frame.meta)
            && self.shows_in_hand(frame, except)
    }

    /// Whether the whole filter shows this frame.
    pub fn shows(&self, frame: Frame) -> bool {
        frame.index && self.shows_meta(&self.typed(), frame, None)
    }

    /// Turn on the chips `--filter` asked for that no frame has to
    /// name — a kind, which [`from_cli`] has already read as the
    /// term reads it — and give back the rest, to be chosen once the
    /// frames and the index can name theirs.
    pub fn choose_at_once(&mut self, wanted: Vec<Wanted>) -> Vec<Wanted> {
        let mut rest = Vec::new();
        for want in wanted {
            if want.facet == Facet::Kind {
                if !self.chosen(Facet::Kind).contains(&want.value) {
                    self.toggle_facet(Facet::Kind, &want.value);
                }
            } else {
                rest.push(want);
            }
        }
        rest
    }

    /// The file indices the browser lists, in file order.
    pub fn apply(&self, frames: &[Frame]) -> Vec<usize> {
        let typed = self.typed();
        frames
            .iter()
            .enumerate()
            .filter(|(_, f)| f.index && self.shows_meta(&typed, **f, None))
            .map(|(i, _)| i)
            .collect()
    }

    /// Tick a flag chip on, or off if it was on or held out.
    pub fn toggle_flag(&mut self, flag: Flag) {
        press(&mut self.flags, &mut self.not_flags, flag, false);
    }

    /// Hold a flag chip out, or let it back if it was.
    pub fn exclude_flag(&mut self, flag: Flag) {
        press(&mut self.flags, &mut self.not_flags, flag, true);
    }

    pub fn toggle_label(&mut self, label: Label) {
        press(&mut self.labels, &mut self.not_labels, label, false);
    }

    pub fn exclude_label(&mut self, label: Label) {
        press(&mut self.labels, &mut self.not_labels, label, true);
    }

    /// The filter in words, for the status line and the log, in the
    /// window's language: what is asked of a frame, in the order the
    /// chips sit in, or "all frames" when nothing is asked.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        match self.stars {
            Stars::AtLeast(0) => {}
            Stars::AtLeast(n) => parts.push(tr!("{n} star or more" | "{n} stars or more" % n)),
            Stars::Exactly(0) => parts.push(tr!("unrated")),
            Stars::Exactly(n) => parts.push(tr!("exactly {n} star" | "exactly {n} stars" % n)),
        }
        let label_name = |l: &Label| match l {
            Label::None => tr!("no label"),
            _ => text(&l.name().to_lowercase()),
        };
        let flag_name = |f: &Flag| text(&f.name().to_lowercase());
        if !self.flags.is_empty() {
            parts.push(join(self.flags.iter().map(flag_name)));
        }
        if !self.not_flags.is_empty() {
            parts.push(tr!("not {}", join(self.not_flags.iter().map(flag_name))));
        }
        if !self.labels.is_empty() {
            parts.push(join(self.labels.iter().map(label_name)));
        }
        // "No label" held out is every label, said so.
        if self.not_labels.contains(&Label::None) {
            parts.push(tr!("labeled"));
        }
        let colors: Vec<&Label> = self
            .not_labels
            .iter()
            .filter(|l| **l != Label::None)
            .collect();
        if !colors.is_empty() {
            parts.push(tr!("not {}", join(colors.into_iter().map(label_name))));
        }
        let text = self.text.trim();
        if !text.is_empty() {
            parts.push(format!("\"{text}\""));
        }
        for (facet, values, not) in Facet::ALL
            .into_iter()
            .flat_map(|f| [(f, self.chosen(f), false), (f, self.held_out(f), true)])
        {
            if values.is_empty() {
                continue;
            }
            let said = join(values.iter().cloned());
            let asked = match facet {
                Facet::Camera => tr!("camera {}", said),
                Facet::Lens => tr!("lens {}", said),
                Facet::Kind => tr!(
                    "kind {}",
                    join(values.iter().map(|v| facet_chip_text(Facet::Kind, v)))
                ),
                Facet::Style => tr!("style {}", said),
                Facet::Iso => format!("ISO {said}"),
                Facet::Focal => format!("{said} mm"),
                Facet::Date => tr!("taken {}", said),
                Facet::Keyword => tr!("keyword {}", said),
            };
            parts.push(if not { tr!("not {}", asked) } else { asked });
        }
        if parts.is_empty() {
            tr!("all frames")
        } else {
            parts.join(", ")
        }
    }
}

/// A facet's chips `--filter` asked for, to be turned on once the
/// index can name them: with `:` every value containing `value`,
/// with `=` the value that is it, case aside, as the language reads
/// the two on a text field; a number is equal either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub facet: Facet,
    pub value: String,
    pub exact: bool,
}

/// `--filter` in the filter language, split: a camera, lens, kind,
/// style, date, or keyword term with `:` or `=` is a facet's chips to
/// turn on once they can be named ([`Wanted`]); every other token,
/// ISO and focal length terms among them, is left for the text field,
/// in its order. A
/// kind is read as the term reads it (`jpg` is `jpeg`) and wanted
/// exactly, which names its chip at once
/// ([`Filter::choose_at_once`]), and one that is no kind is left for
/// the field, to be refused there with the kinds there are.
pub fn from_cli(text: &str) -> (String, Vec<Wanted>) {
    let mut rest = Vec::new();
    let mut wanted = Vec::new();
    for token in greycard_library::filter::tokens(text) {
        let name_len = token
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(token.len());
        let (name, after) = token.split_at(name_len);
        let (value, exact) = match (after.strip_prefix(':'), after.strip_prefix('=')) {
            (Some(v), _) => (Some(v), false),
            (None, Some(v)) => (Some(v), true),
            (None, None) => (None, false),
        };
        match (Facet::from_field(name), value.map(|v| v.trim_matches('"'))) {
            // ISO and the focal length have no chips: their terms go to
            // the text, where the CULLING section's range reads them,
            // under the field's own name (`mm:50` as `focal:50`). A
            // token that is no term (the word `mm`) stays as typed.
            (Some(facet), _) if ranged(facet).is_some() => {
                let named = format!("{}{after}", facet.field());
                rest.push(if greycard_library::filter::is_term(&named) {
                    named
                } else {
                    token.clone()
                });
            }
            (Some(Facet::Kind), Some(v)) => match greycard_library::filter::kind_named(v) {
                Some(kind) => wanted.push(Wanted {
                    facet: Facet::Kind,
                    value: kind.to_string(),
                    exact: true,
                }),
                None => rest.push(token),
            },
            (Some(facet), Some(v)) if !v.is_empty() && !v.starts_with('=') => {
                wanted.push(Wanted {
                    facet,
                    value: v.to_string(),
                    exact,
                });
            }
            _ => rest.push(token),
        }
    }
    (rest.join(" "), wanted)
}

/// Whether an any-of set lets a value through: an empty set is every
/// value, not none, so a group with no chip ticked asks nothing.
fn any_of<T: PartialEq>(set: &[T], value: T) -> bool {
    set.is_empty() || set.contains(&value)
}

/// A group's chips after `chip` is pressed, `on` the set ticked and
/// `out` the set held out, the order of the rest kept. A chip held
/// out comes back to neither, whichever the press; otherwise a plain
/// press (`exclude` false) ticks it on or off, and a Ctrl+press holds
/// it out, off the ticked set.
///
/// This is what a click does. It is not what a chip's count counts;
/// see [`Counts`] for why those are two different questions.
fn press<T: PartialEq>(on: &mut Vec<T>, out: &mut Vec<T>, chip: T, exclude: bool) {
    if let Some(at) = out.iter().position(|v| *v == chip) {
        out.remove(at);
    } else if let Some(at) = on.iter().position(|v| *v == chip) {
        on.remove(at);
        if exclude {
            out.push(chip);
        }
    } else if exclude {
        out.push(chip);
    } else {
        on.push(chip);
    }
}

/// "a", "a or b", "a, b or c".
fn join(names: impl IntoIterator<Item = String>) -> String {
    let names: Vec<String> = names.into_iter().collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => tr!("{} or {last}", rest.join(", "), last = last),
    }
}

/// What each chip would show, and what the whole filter shows now.
///
/// A chip's count is how many frames carry that chip's value among
/// the frames the *other* groups leave: its own group's chips are
/// ignored, and every other group is applied as it stands. So the
/// flag row under a three-star filter reads how the three-star
/// frames are flagged, and the label row reads how they are
/// labeled — the state of the cull, narrowed by everything else
/// being asked.
///
/// It is worth being plain about what this is not, because the first
/// cut of it was the other thing. A count is not "press this and N
/// frames will be listed". For the rating row the two happen to
/// agree, since a rating chip replaces. For the flag and label rows
/// they do not, because those chips *join* their group: with Picks
/// on, this rule has Reject read the number of rejects, while
/// pressing Reject adds them to the picks and lists both. The
/// press-and-N reading was tried and is worse — with Picks on it
/// made the row read Unflagged 28, Pick 33, Reject 18 on a shoot of
/// 33, which is arithmetic about the filter rather than anything
/// about the pictures, and a culler learns nothing from it. What is
/// wanted from a row of chips is the shape of the shoot; how many
/// frames a press will leave is one number, and the `N of M` beside
/// the field is where it belongs.
///
/// Two consequences worth leaning on. Within a group the counts sum
/// to the frames the other groups leave, since a frame has one flag
/// and one label or none — a row that adds up is a row that can be
/// read at a glance. And a chip that says 0 leads nowhere when
/// pressed *alone*, which is what dims it.
///
/// Stated exactly: `counts.flags[n]` is
/// `filter.only_flag(Flag::ALL[n]).apply(frames).len()`, and the
/// same for the label and rating rows. The tests hold it to that.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Counts {
    /// By the chip, 0 to [`STARS`], read the way the row is read.
    pub stars: [usize; STARS as usize + 1],
    /// By the chip, in [`Flag::ALL`]'s order: unflagged, pick, reject.
    pub flags: [usize; 3],
    /// By the chip, in [`Label::ALL`]'s order: no label, then the
    /// five colors.
    pub labels: [usize; 6],
    /// What the filter as it stands shows, and the folder's size.
    pub shown: usize,
    pub total: usize,
}

impl Counts {
    /// One pass over the loaded sidecars. Nothing is indexed and no
    /// file is read: the folder is open, its sidecars are in hand,
    /// and a thousand frames is a thousand cheap comparisons.
    ///
    /// Each frame answers the four groups once. It then counts
    /// towards a group's chips exactly when the other three said
    /// yes, which is that group being ignored for its own row; and
    /// within the row it lands on the chip it carries — for the
    /// rating, on every chip its stars satisfy, since those chips
    /// overlap by construction.
    pub fn of(filter: &Filter, frames: &[Frame]) -> Counts {
        let mut counts = Counts {
            total: frames.len(),
            ..Counts::default()
        };
        let exact = filter.stars.is_exact();
        let typed = filter.typed();
        for frame in frames {
            let meta = frame.meta;
            let stars = filter.stars.shows(meta.rating);
            let flag = filter.shows_flag(meta.flag);
            let label = filter.shows_label(meta.label);
            // The text, the chips answered in hand and the index's
            // tests are groups of their own whose rows are counted
            // elsewhere, so here they are one more thing the other
            // rows apply.
            let text =
                typed.shows(frame.path, meta) && filter.shows_in_hand(*frame, None) && frame.index;
            if stars && flag && label && text {
                counts.shown += 1;
            }
            if flag && label && text {
                for (n, seen) in counts.stars.iter_mut().enumerate() {
                    if Stars::AtLeast(n as u8).read_as(exact).shows(meta.rating) {
                        *seen += 1;
                    }
                }
            }
            if stars
                && label
                && text
                && let Some(n) = Flag::ALL.iter().position(|f| *f == meta.flag)
            {
                counts.flags[n] += 1;
            }
            if stars
                && flag
                && text
                && let Some(n) = Label::ALL.iter().position(|l| *l == meta.label)
            {
                counts.labels[n] += 1;
            }
        }
        counts
    }
}

/// The three filters a row of chips is counted against, which is the
/// rule [`Counts`] keeps written down in one place and the tests
/// hold it to. Nothing in the window needs them — the count is made
/// in one pass rather than by filtering the folder once a chip — so
/// they exist to say what that pass is supposed to come to.
#[cfg(test)]
impl Filter {
    /// This filter with the flag group set to just this one chip.
    ///
    /// The question a flag chip's count answers, and deliberately not
    /// the question pressing it asks: see [`Counts`].
    pub fn only_flag(&self, flag: Flag) -> Filter {
        Filter {
            flags: vec![flag],
            not_flags: Vec::new(),
            ..self.clone()
        }
    }

    /// This filter with the label group set to just this one chip.
    pub fn only_label(&self, label: Label) -> Filter {
        Filter {
            labels: vec![label],
            not_labels: Vec::new(),
            ..self.clone()
        }
    }

    /// This filter with the rating set to this chip, read the way the
    /// row is read. A rating chip replaces rather than joining, so
    /// here the count's question and the press's are the same one.
    pub fn only_stars(&self, n: u8) -> Filter {
        Filter {
            stars: Stars::AtLeast(n).read_as(self.stars.is_exact()),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod range_tests;
