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
//! focal length, day, and the keyword beside them — are the library
//! index's (notes §160). What is asked of the index is decided here
//! ([`Filter::index_filter`]) and answered in `library`, and comes
//! back as one bit a frame, [`Frame::index`]: whether the index's
//! tests pass it, or the index has no row for it yet. A frame the
//! index has not reached is shown until it has; hiding it for want
//! of a row would be the filter guessing. The facets' own counts are
//! one `GROUP BY` a facet over the frames the other tests leave,
//! which is this module's rule for a count, asked of SQL.
//!
//! The text field reads the §160 grammar as well as words: a token
//! shaped like a term (`camera:R6`, `iso>=3200`, `rating>=3`) is a
//! term, anything else the word it always was. A term the sidecar
//! can answer — rating, flag, label, keyword, name — is answered
//! from the sidecar, which is fresher than the row an `index_file`
//! is still writing; the EXIF ones go to the index.

use std::path::Path;

use greycard_edit::meta::{Flag, Label, Meta, STARS};
pub(crate) use greycard_library::filter::Facet;
use greycard_library::filter::{ParseError, Term};

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
            "Any".to_string()
        } else if n >= STARS {
            STARS.to_string()
        } else {
            format!("{n}+")
        }
    }
}

/// What the browser says when the filter has hidden the whole
/// folder. A blank grid and a zero is not an explanation, and the
/// four places that can arrive at one say the same thing.
///
/// It used to read "show All to see them", which named a control:
/// All was the three-way Show's first option. It is now the first
/// rating chip and means any rating, so the way back is Clear.
pub const NOTHING_SHOWN: &str = "no frames pass the filter; Clear shows the folder again";

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

/// A facet's place in [`Facet::ALL`], which is its slot in
/// [`Filter::facets`] and its code on the window.
pub fn facet_slot(facet: Facet) -> usize {
    Facet::ALL
        .iter()
        .position(|f| *f == facet)
        .expect("every facet is in ALL")
}

/// What a facet's row is captioned on the bar.
pub fn facet_caption(facet: Facet) -> &'static str {
    match facet {
        Facet::Camera => "Camera",
        Facet::Lens => "Lens",
        Facet::Iso => "ISO",
        Facet::Focal => "Focal",
        Facet::Date => "Day",
        Facet::Keyword => "Keyword",
    }
}

/// A facet's value as its chip says it: a focal length in
/// millimetres, the rest as the index has it.
pub fn facet_chip_text(facet: Facet, label: &str) -> String {
    match facet {
        Facet::Focal => format!("{label} mm"),
        _ => label.to_string(),
    }
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
    pub facets: [Vec<String>; 6],
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
        any_of(&self.flags, flag)
    }

    pub fn shows_label(&self, label: Label) -> bool {
        any_of(&self.labels, label)
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

    /// Press a facet's chip: on if it was off, off if it was on.
    pub fn toggle_facet(&mut self, facet: Facet, value: &str) {
        let slot = &mut self.facets[facet_slot(facet)];
        *slot = pressed(slot, value.to_string());
    }

    /// Whether the keyword chips pass this frame. The keyword is the
    /// sidecar's, so it is asked of the sidecar, as the text field's
    /// keyword terms are; its counts are still the index's.
    pub fn shows_keyword(&self, meta: &Meta) -> bool {
        let values = self.chosen(Facet::Keyword);
        values.is_empty()
            || Term::Facet {
                facet: Facet::Keyword,
                values: values.to_vec(),
            }
            .on_meta(Path::new(""), meta)
            .unwrap_or(true)
    }

    /// What the index is asked: the EXIF terms typed and every facet
    /// but the keyword with a chip on, less `except`'s own group —
    /// which is how a facet's count sets its own row aside.
    pub fn index_filter(&self, typed: &Typed, except: Option<Facet>) -> greycard_library::Filter {
        let mut terms = typed.index.clone();
        for facet in Facet::ALL {
            let values = self.chosen(facet);
            if facet == Facet::Keyword || Some(facet) == except || values.is_empty() {
                continue;
            }
            terms.push(Term::Facet {
                facet,
                values: values.to_vec(),
            });
        }
        greycard_library::Filter { terms }
    }

    /// Whether anything is asked of the index: without that, every
    /// frame passes it and the index need not be asked.
    pub fn asks_index(&self, typed: &Typed) -> bool {
        !typed.index.is_empty()
            || Facet::ALL
                .iter()
                .any(|f| *f != Facet::Keyword && !self.chosen(*f).is_empty())
    }

    /// Whether the tests the sidecars answer pass this frame: every
    /// group but the index's, and with `keyword` false the keyword
    /// chips set aside, for that facet's own count.
    pub fn shows_meta(&self, typed: &Typed, frame: Frame, keyword: bool) -> bool {
        self.stars.shows(frame.meta.rating)
            && self.shows_flag(frame.meta.flag)
            && self.shows_label(frame.meta.label)
            && typed.shows(frame.path, frame.meta)
            && (!keyword || self.shows_keyword(frame.meta))
    }

    /// Whether the whole filter shows this frame.
    pub fn shows(&self, frame: Frame) -> bool {
        frame.index && self.shows_meta(&self.typed(), frame, true)
    }

    /// The file indices the browser lists, in file order.
    pub fn apply(&self, frames: &[Frame]) -> Vec<usize> {
        let typed = self.typed();
        frames
            .iter()
            .enumerate()
            .filter(|(_, f)| f.index && self.shows_meta(&typed, **f, true))
            .map(|(i, _)| i)
            .collect()
    }

    /// Tick a flag chip on, or off if it was on.
    pub fn toggle_flag(&mut self, flag: Flag) {
        self.flags = pressed(&self.flags, flag);
    }

    pub fn toggle_label(&mut self, label: Label) {
        self.labels = pressed(&self.labels, label);
    }

    /// The filter in words, for the log and the status line: what is
    /// asked of a frame, in the order the chips sit in, or "all
    /// frames" when nothing is asked.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        match self.stars {
            Stars::AtLeast(0) => {}
            Stars::AtLeast(n) => parts.push(format!("{n} stars or more")),
            Stars::Exactly(0) => parts.push("unrated".to_string()),
            Stars::Exactly(n) => parts.push(format!("exactly {n} stars")),
        }
        if !self.flags.is_empty() {
            parts.push(join(self.flags.iter().map(|f| f.name().to_lowercase())));
        }
        if !self.labels.is_empty() {
            parts.push(join(self.labels.iter().map(|l| match l {
                Label::None => "no label".to_string(),
                _ => l.name().to_lowercase(),
            })));
        }
        let text = self.text.trim();
        if !text.is_empty() {
            parts.push(format!("\"{text}\""));
        }
        for facet in Facet::ALL {
            let values = self.chosen(facet);
            if values.is_empty() {
                continue;
            }
            let said = join(values.iter().cloned());
            parts.push(match facet {
                Facet::Camera => format!("camera {said}"),
                Facet::Lens => format!("lens {said}"),
                Facet::Iso => format!("ISO {said}"),
                Facet::Focal => format!("{said} mm"),
                Facet::Date => format!("taken {said}"),
                Facet::Keyword => format!("keyword {said}"),
            });
        }
        if parts.is_empty() {
            "all frames".to_string()
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

/// `--filter` in the filter language, split: a camera, lens, iso,
/// focal, date or keyword term with `:` or `=` is a facet's chips to
/// turn on once the index can name them ([`Wanted`]); every other
/// token is left for the text field, in its order.
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

/// The set a group would come to if `chip` were pressed: on if it was
/// off, off if it was on, the order of the rest kept.
///
/// This is what a click does. It is not what a chip's count counts;
/// see [`Counts`] for why those are two different questions.
fn pressed<T: PartialEq + Clone>(set: &[T], chip: T) -> Vec<T> {
    let mut out = set.to_vec();
    match out.iter().position(|v| *v == chip) {
        Some(at) => {
            out.remove(at);
        }
        None => out.push(chip),
    }
    out
}

/// "a", "a or b", "a, b or c".
fn join(names: impl IntoIterator<Item = String>) -> String {
    let names: Vec<String> = names.into_iter().collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

/// What each chip would show, and what the whole filter shows now.
///
/// A chip's count is how many frames carry that chip's value among
/// the frames the *other* groups leave: its own group's chips are
/// ignored, and every other group is applied as it stands. So the
/// flag row under a three-star filter reads how the three-star
/// frames are flagged, and the label row reads how they are
/// labelled — the state of the cull, narrowed by everything else
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
            // The text, the keyword chips and the index's tests are
            // groups of their own whose rows are counted elsewhere,
            // so here they are one more thing the other rows apply.
            let text = typed.shows(frame.path, meta) && filter.shows_keyword(meta) && frame.index;
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
            ..self.clone()
        }
    }

    /// This filter with the label group set to just this one chip.
    pub fn only_label(&self, label: Label) -> Filter {
        Filter {
            labels: vec![label],
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
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A folder to filter: a name, a rating, a flag, a label and
    /// some keywords each.
    fn folder() -> Vec<(PathBuf, Meta)> {
        let frame = |name: &str, rating, flag, label, words: &[&str]| {
            (
                PathBuf::from("/shoot").join(name),
                Meta {
                    rating,
                    flag,
                    label,
                    keywords: words.iter().map(|w| w.to_string()).collect(),
                    ..Meta::default()
                },
            )
        };
        vec![
            frame("IMG_0001.CR3", 0, Flag::None, Label::None, &[]),
            frame("IMG_0002.CR3", 3, Flag::Pick, Label::Red, &["harbor"]),
            frame("IMG_0003.CR3", 5, Flag::Pick, Label::Green, &["Sunset"]),
            frame("IMG_0004.CR3", 1, Flag::Reject, Label::None, &["sunset"]),
            frame(
                "DSC_0005.NEF",
                3,
                Flag::None,
                Label::Red,
                &["harbor", "dusk"],
            ),
        ]
    }

    /// Thirty-three frames flagged as the sample shoot is: fifteen
    /// nobody has said anything about, thirteen picks, five rejects.
    /// Nothing else is set, so the flag row is the only one with
    /// anything to say.
    fn shoot() -> Vec<(PathBuf, Meta)> {
        let flags = std::iter::repeat_n(Flag::None, 15)
            .chain(std::iter::repeat_n(Flag::Pick, 13))
            .chain(std::iter::repeat_n(Flag::Reject, 5));
        flags
            .enumerate()
            .map(|(i, flag)| {
                (
                    PathBuf::from("/shoot").join(format!("IMG_{i:04}.CR3")),
                    Meta {
                        flag,
                        ..Meta::default()
                    },
                )
            })
            .collect()
    }

    fn view(folder: &[(PathBuf, Meta)]) -> Vec<Frame<'_>> {
        folder
            .iter()
            .map(|(path, meta)| Frame {
                path,
                meta,
                index: true,
            })
            .collect()
    }

    #[test]
    fn each_chip_asks_its_own_question_and_a_frame_passes_them_all() {
        let folder = folder();
        let frames = view(&folder);
        // Nothing asked is the whole folder.
        assert!(Filter::default().is_empty());
        assert_eq!(Filter::default().apply(&frames), vec![0, 1, 2, 3, 4]);

        // At least three stars, and exactly three.
        let three = Filter {
            stars: Stars::AtLeast(3),
            ..Filter::default()
        };
        assert_eq!(three.apply(&frames), vec![1, 2, 4]);
        let exactly = Filter {
            stars: Stars::Exactly(3),
            ..Filter::default()
        };
        assert_eq!(exactly.apply(&frames), vec![1, 4]);
        let unrated = Filter {
            stars: Stars::Exactly(0),
            ..Filter::default()
        };
        assert_eq!(unrated.apply(&frames), vec![0]);

        // Flags, any-of: one chip, then two.
        let picks = Filter {
            flags: vec![Flag::Pick],
            ..Filter::default()
        };
        assert_eq!(picks.apply(&frames), vec![1, 2]);
        let no_rejects = Filter {
            flags: vec![Flag::None, Flag::Pick],
            ..Filter::default()
        };
        assert_eq!(no_rejects.apply(&frames), vec![0, 1, 2, 4]);

        // Labels, any-of, and "no label" is a chip like the rest.
        let warm = Filter {
            labels: vec![Label::Red, Label::Green],
            ..Filter::default()
        };
        assert_eq!(warm.apply(&frames), vec![1, 2, 4]);
        let bare = Filter {
            labels: vec![Label::None],
            ..Filter::default()
        };
        assert_eq!(bare.apply(&frames), vec![0, 3]);

        // All four at once, which is an and.
        let both = Filter {
            stars: Stars::AtLeast(3),
            flags: vec![Flag::Pick],
            labels: vec![Label::Red],
            text: "harbor".into(),
            ..Filter::default()
        };
        assert_eq!(both.apply(&frames), vec![1]);
    }

    #[test]
    fn the_text_looks_at_the_name_and_the_keywords_and_wants_every_word() {
        let folder = folder();
        let frames = view(&folder);
        let find = |text: &str| {
            Filter {
                text: text.into(),
                ..Filter::default()
            }
            .apply(&frames)
        };
        // The name, whole or in part, and case is nobody's business.
        assert_eq!(find("DSC"), vec![4]);
        assert_eq!(find("dsc_0005"), vec![4]);
        assert_eq!(find("img"), vec![0, 1, 2, 3]);
        // A keyword, whatever case it was put on in.
        assert_eq!(find("sunset"), vec![2, 3]);
        assert_eq!(find("SUNSET"), vec![2, 3]);
        // Two words narrow: both have to be somewhere, and they may
        // be in different places.
        assert_eq!(find("harbor dusk"), vec![4]);
        assert_eq!(find("harbor 0002"), vec![1]);
        assert_eq!(find("harbor sunset"), Vec::<usize>::new());
        // Space is not a query.
        assert_eq!(find("   "), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_chip_says_how_many_frames_carry_it_among_the_rest() {
        let folder = folder();
        let frames = view(&folder);

        // The rule, held against every chip of every group: a chip's
        // count is what the filter would show with that chip's own
        // group set to just that chip — the other groups as they
        // stand, its own set aside.
        let holds = |filter: &Filter| {
            let counts = Counts::of(filter, &frames);
            let what = filter.describe();
            assert_eq!(counts.total, frames.len(), "{what}");
            assert_eq!(counts.shown, filter.apply(&frames).len(), "{what}");
            for (n, &flag) in Flag::ALL.iter().enumerate() {
                let want = filter.only_flag(flag).apply(&frames).len();
                assert_eq!(counts.flags[n], want, "flag chip {n} of {what}");
            }
            for (n, &label) in Label::ALL.iter().enumerate() {
                let want = filter.only_label(label).apply(&frames).len();
                assert_eq!(counts.labels[n], want, "label chip {n} of {what}");
            }
            for n in 0..=STARS {
                let want = filter.only_stars(n).apply(&frames).len();
                assert_eq!(counts.stars[n as usize], want, "star chip {n} of {what}");
            }
            // And so a row adds up to what the other groups leave,
            // since a frame carries one flag and one label or none.
            // A row that adds up is a row that reads at a glance.
            let loose = Filter {
                flags: Vec::new(),
                ..filter.clone()
            };
            let sum: usize = counts.flags.iter().sum();
            assert_eq!(sum, loose.apply(&frames).len(), "the flags add up, {what}");
            let loose = Filter {
                labels: Vec::new(),
                ..filter.clone()
            };
            let sum: usize = counts.labels.iter().sum();
            assert_eq!(sum, loose.apply(&frames).len(), "the labels add up, {what}");
        };
        holds(&Filter::default());
        for flags in [
            vec![Flag::Pick],
            vec![Flag::Pick, Flag::Reject],
            vec![Flag::None, Flag::Pick],
        ] {
            holds(&Filter {
                flags,
                ..Filter::default()
            });
        }
        for labels in [
            vec![Label::Red],
            vec![Label::Red, Label::Green],
            vec![Label::None],
        ] {
            holds(&Filter {
                labels,
                ..Filter::default()
            });
        }
        holds(&Filter {
            stars: Stars::AtLeast(5),
            ..Filter::default()
        });
        holds(&Filter {
            stars: Stars::Exactly(3),
            ..Filter::default()
        });
        holds(&Filter {
            stars: Stars::AtLeast(3),
            flags: vec![Flag::Pick],
            labels: vec![Label::Red, Label::Green],
            text: "sunset".into(),
            ..Filter::default()
        });

        // And the numbers themselves, so the rule above is anchored
        // to something a reader can count on their fingers.
        let counts = Counts::of(&Filter::default(), &frames);
        assert_eq!(counts.total, 5);
        assert_eq!(counts.shown, 5);
        assert_eq!(counts.stars, [5, 4, 3, 3, 1, 1]);
        assert_eq!(counts.flags, [2, 2, 1]);
        assert_eq!(counts.labels, [2, 2, 0, 1, 0, 0]);

        // Picks only: the flag row is unmoved, because a flag chip's
        // count sets its own group aside, while the star and label
        // rows narrow to the two picks.
        let picks = Filter {
            flags: vec![Flag::Pick],
            ..Filter::default()
        };
        let counts = Counts::of(&picks, &frames);
        assert_eq!(counts.shown, 2);
        assert_eq!(counts.flags, [2, 2, 1]);
        assert_eq!(counts.stars, [2, 2, 2, 2, 1, 1]);
        assert_eq!(counts.labels, [0, 1, 0, 1, 0, 0]);

        // A chip that leads nowhere says nothing rather than a
        // number that turns out to be somebody else's.
        let five = Filter {
            stars: Stars::AtLeast(5),
            ..Filter::default()
        };
        let counts = Counts::of(&five, &frames);
        assert_eq!(counts.shown, 1);
        assert_eq!(counts.flags, [0, 1, 0]);
        assert_eq!(counts.labels, [0, 0, 0, 1, 0, 0]);

        // Under "exactly" the star chips are the ratings themselves.
        let exact = Filter {
            stars: Stars::Exactly(3),
            ..Filter::default()
        };
        let counts = Counts::of(&exact, &frames);
        assert_eq!(counts.shown, 2);
        assert_eq!(counts.stars, [1, 1, 0, 2, 0, 1]);
    }

    /// The case the rule exists for, on a folder shaped like the
    /// sample shoot: with the picks showing, the flag row still says
    /// how the shoot is flagged. The other reading — how many frames
    /// each press would leave — would say 28, 33 and 18 here, which
    /// is arithmetic about the filter and tells a culler nothing
    /// about the pictures. How many a press leaves is one number and
    /// the figure beside the text field is where it goes.
    #[test]
    fn the_flag_row_reads_the_shoot_and_the_press_is_read_beside_the_field() {
        let shoot = shoot();
        let frames = view(&shoot);
        let picks = Filter {
            flags: vec![Flag::Pick],
            ..Filter::default()
        };
        let counts = Counts::of(&picks, &frames);
        assert_eq!((counts.shown, counts.total), (13, 33));
        assert_eq!(counts.flags, [15, 13, 5], "the state of the cull");

        // Press Reject and it joins the picks: eighteen frames, and
        // the `N of M` beside the field is what says eighteen.
        let mut after = picks.clone();
        after.toggle_flag(Flag::Reject);
        assert_eq!(after.flags, vec![Flag::Pick, Flag::Reject]);
        assert_eq!(after.apply(&frames).len(), 18);
        assert_eq!(Counts::of(&after, &frames).shown, 18);
        // And the row still reads the shoot, unmoved by either chip.
        assert_eq!(Counts::of(&after, &frames).flags, [15, 13, 5]);
    }

    #[test]
    fn the_old_three_way_show_still_parses_and_still_means_what_it_did() {
        let folder = folder();
        let frames = view(&folder);
        let flags: Vec<Flag> = folder.iter().map(|(_, m)| m.flag).collect();
        for name in Filter::NAMES {
            let filter = Filter::from_name(name).expect("one of the three");
            let want: Vec<usize> = flags
                .iter()
                .enumerate()
                .filter(|(_, f)| match name {
                    "Picks" => **f == Flag::Pick,
                    "No rejects" => **f != Flag::Reject,
                    _ => true,
                })
                .map(|(i, _)| i)
                .collect();
            assert_eq!(filter.apply(&frames), want, "{name}");
        }
        assert_eq!(Filter::from_name("Rejects"), None);
        assert!(Filter::from_name("All").expect("all").is_empty());
    }

    /// `--filter` in the filter language: a facet term with `:` or
    /// `=` waits for the index to name its chips, and the rest is
    /// the text field's.
    #[test]
    fn the_command_line_filter_splits_into_chips_and_text() {
        let want = |facet, value: &str, exact| Wanted {
            facet,
            value: value.to_string(),
            exact,
        };
        let (text, wanted) = from_cli("camera:R6 iso>=3200 rating>=3 focal=50mm harbor");
        assert_eq!(text, "iso>=3200 rating>=3 harbor");
        assert_eq!(
            wanted,
            vec![
                want(Facet::Camera, "R6", false),
                want(Facet::Focal, "50mm", true)
            ]
        );
        let (text, wanted) = from_cli("lens:\"RF 50mm\" date:2026-09 kw:dusk camera=\"EOS R5\"");
        assert_eq!(text, "");
        assert_eq!(
            wanted,
            vec![
                want(Facet::Lens, "RF 50mm", false),
                want(Facet::Date, "2026-09", false),
                want(Facet::Keyword, "dusk", false),
                want(Facet::Camera, "EOS R5", true)
            ]
        );
        // Not a facet, or not a chip's operator: text.
        let (text, wanted) = from_cli("aperture:2.8 iso!=100");
        assert_eq!(text, "aperture:2.8 iso!=100");
        assert!(wanted.is_empty());
    }

    /// The field reads terms among its words, and a field with no
    /// term in it means exactly what the search box meant.
    #[test]
    fn the_text_field_reads_terms_and_leaves_words_as_they_were() {
        let typed = |text: &str| {
            Filter {
                text: text.into(),
                ..Filter::default()
            }
            .typed()
        };
        let t = typed("harbor Dusk \"low tide\" 12:30 foo:bar");
        assert_eq!(
            t.words,
            ["harbor", "dusk", "\"low", "tide\"", "12:30", "foo:bar"]
        );
        assert!(t.meta.is_empty() && t.index.is_empty() && t.errors.is_empty());
        let t = typed("camera:R6 rating>=3 iso>=3200 keyword:dusk lens:\"RF 50\"");
        assert!(t.words.is_empty());
        assert_eq!(t.meta.len(), 2, "rating and keyword are the sidecar's");
        assert_eq!(t.index.len(), 3, "camera, iso and lens are the index's");
        // One that does not parse is looked for as the word it was.
        let t = typed("rating>=9");
        assert_eq!(t.words, ["rating>=9"]);
        assert_eq!(t.errors.len(), 1);

        // A sidecar term narrows here; an index term is the index's.
        let folder = folder();
        let frames = view(&folder);
        let find = |text: &str| {
            Filter {
                text: text.into(),
                ..Filter::default()
            }
            .apply(&frames)
        };
        assert_eq!(find("rating>=3"), vec![1, 2, 4]);
        assert_eq!(find("rating>=3 flag:pick"), vec![1, 2]);
        assert_eq!(find("keyword=sunset"), vec![2, 3]);
        assert_eq!(find("label:red dusk"), vec![4]);
        assert_eq!(
            find("camera:R6"),
            vec![0, 1, 2, 3, 4],
            "no index, no answer"
        );
    }

    /// A facet's chips join their group as the flags do, and the
    /// index's answer arrives as each frame's `index` bit.
    #[test]
    fn a_facet_chip_joins_its_group_and_the_index_decides_the_rest() {
        let folder = folder();
        let mut filter = Filter::default();
        filter.toggle_facet(Facet::Camera, "Canon EOS R6");
        filter.toggle_facet(Facet::Camera, "Canon EOS R5");
        assert_eq!(
            filter.chosen(Facet::Camera),
            ["Canon EOS R6", "Canon EOS R5"]
        );
        assert!(!filter.is_empty());
        assert!(filter.asks_index(&filter.typed()));
        assert_eq!(filter.describe(), "camera Canon EOS R6 or Canon EOS R5");
        // The index's tests less the camera's own, for its count.
        let typed = filter.typed();
        assert_eq!(filter.index_filter(&typed, None).terms.len(), 1);
        assert!(filter.index_filter(&typed, Some(Facet::Camera)).is_empty());
        // Frames 1 and 3 fail the index; the others pass or have no
        // row yet, and show.
        let frames: Vec<Frame> = folder
            .iter()
            .enumerate()
            .map(|(i, (path, meta))| Frame {
                path,
                meta,
                index: i != 1 && i != 3,
            })
            .collect();
        assert_eq!(filter.apply(&frames), vec![0, 2, 4]);
        let counts = Counts::of(&filter, &frames);
        assert_eq!(counts.shown, 3);
        assert_eq!(counts.flags, [2, 1, 0], "the rows read what the index left");

        // The keyword chips are the sidecar's, any-of, case folded.
        let mut words = Filter::default();
        words.toggle_facet(Facet::Keyword, "harbor");
        assert!(!words.asks_index(&words.typed()), "the keyword is in hand");
        assert_eq!(words.apply(&view(&folder)), vec![1, 4]);
        words.toggle_facet(Facet::Keyword, "sunset");
        assert_eq!(words.apply(&view(&folder)), vec![1, 2, 3, 4]);
        words.toggle_facet(Facet::Keyword, "harbor");
        words.toggle_facet(Facet::Keyword, "sunset");
        assert!(words.is_empty(), "pressed twice is not pressed");
    }

    #[test]
    fn a_chip_pressed_twice_is_a_chip_not_pressed() {
        let mut filter = Filter::default();
        filter.toggle_flag(Flag::Pick);
        filter.toggle_flag(Flag::None);
        assert_eq!(filter.flags, vec![Flag::Pick, Flag::None]);
        filter.toggle_flag(Flag::Pick);
        assert_eq!(filter.flags, vec![Flag::None]);
        filter.toggle_flag(Flag::None);
        assert!(filter.is_empty(), "back to the whole folder");

        filter.toggle_label(Label::Red);
        filter.toggle_label(Label::Red);
        assert!(filter.is_empty());
    }

    #[test]
    fn the_rating_chips_are_named_for_the_reading_they_are_in() {
        let at_least: Vec<String> = (0..=STARS).map(|n| Stars::chip_name(n, false)).collect();
        assert_eq!(at_least, ["Any", "1+", "2+", "3+", "4+", "5"]);
        let exactly: Vec<String> = (0..=STARS).map(|n| Stars::chip_name(n, true)).collect();
        assert_eq!(exactly, ["0", "1", "2", "3", "4", "5"]);
        // The count survives a change of reading; the meaning does not.
        assert_eq!(Stars::AtLeast(3).read_as(true), Stars::Exactly(3));
        assert!(Stars::AtLeast(3).shows(5));
        assert!(!Stars::Exactly(3).shows(5));
        assert!(Stars::default().shows(0), "the default hides nothing");
    }

    #[test]
    fn the_filter_says_what_it_is_in_words() {
        assert_eq!(Filter::default().describe(), "all frames");
        assert_eq!(
            Filter {
                stars: Stars::AtLeast(4),
                flags: vec![Flag::Pick],
                labels: vec![Label::Red, Label::Green],
                text: " harbor ".into(),
                ..Filter::default()
            }
            .describe(),
            "4 stars or more, pick, red or green, \"harbor\""
        );
        assert_eq!(
            Filter {
                stars: Stars::Exactly(0),
                labels: vec![Label::None],
                ..Filter::default()
            }
            .describe(),
            "unrated, no label"
        );
        assert_eq!(
            Filter::from_name("No rejects")
                .expect("it parses")
                .describe(),
            "unflagged or pick"
        );
    }
}
