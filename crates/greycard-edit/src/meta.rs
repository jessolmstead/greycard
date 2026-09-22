//! What a frame is worth and what it is called: the rating, the
//! pick or reject flag, the color label, the keywords, the title and
//! the caption.
//!
//! This is not part of the edit and never goes through its history.
//! A rating is a judgment about the picture, not a step in
//! developing it: an undo that took a star back would be a surprise,
//! and a preset that carried one to the next frame would be a bug.
//! So the [`Meta`] sits beside [`crate::Edit`] in the sidecar, the
//! two written together and read together, neither one needed for
//! the other: a frame can be rated before it is ever developed and
//! opens at the default edit, and a frame developed for a year can
//! carry no meta at all.
//!
//! Nothing here has a schema version of its own. The edit's
//! [`crate::VERSION`] covers what the edit means; this section is
//! new, additive, and has no field whose meaning has ever changed,
//! so a sidecar written by a build with no meta in it loads with the
//! default, and one written here loads in a build that never heard
//! of it — serde drops what it does not know.
//!
//! Everything here is read loosely, and that is what pays for having
//! no version. A value this build cannot make sense of — a sixth
//! color label a later build wrote, a flag spelled by hand, a rating
//! of two hundred, a keyword list that is one word rather than a
//! list — comes back as the default for that field and nothing else
//! moves. It must never take the read down with it: the meta shares
//! a file with the edit and the history, and a whole sidecar refused
//! over one misspelled word would open the frame as a fresh one and
//! let the next slider write over a year of work.

use serde::{Deserialize, Deserializer, Serialize};

/// The most stars a frame can carry.
pub const STARS: u8 = 5;

/// A value as a file gives it: the thing it should be, or anything
/// at all. The second arm swallows what the first could not read,
/// which is how a field falls back to its default instead of failing
/// the whole read.
#[derive(Deserialize)]
#[serde(untagged)]
enum Loose<T> {
    Known(T),
    Unknown(serde::de::IgnoredAny),
}

/// Read `T`, or its default if what is there is not a `T`.
pub(crate) fn loose<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(match Loose::<T>::deserialize(d)? {
        Loose::Known(value) => value,
        Loose::Unknown(_) => T::default(),
    })
}

/// A whole [`Meta`], or an empty one when the section itself is
/// something this build cannot read. For the sidecar's field: the
/// last net under the per-field ones, so that nothing written under
/// `meta` can cost the edit beside it.
pub fn loose_meta<'de, D: Deserializer<'de>>(d: D) -> Result<Meta, D::Error> {
    loose::<D, Meta>(d)
}

/// A frame's quarter turns, read the same loose way and held to 0..3:
/// a turn of seven, or of "left", is no turn at all rather than a
/// sidecar refused. See [`crate::Sidecar::turn`].
pub fn loose_turn<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    Ok(loose::<D, u8>(d)? % 4)
}

/// The pick-or-reject flag, the culling pass's first cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flag {
    #[default]
    None,
    Pick,
    Reject,
}

impl Flag {
    /// The badge a browser row draws for this flag: nothing, 1 a
    /// pick, 2 a reject. The `.slint` file matches on the number,
    /// which is why it is spelled out here rather than taken from
    /// the enum's own order.
    pub fn code(self) -> i32 {
        match self {
            Flag::None => 0,
            Flag::Pick => 1,
            Flag::Reject => 2,
        }
    }

    pub const ALL: [Flag; 3] = [Flag::None, Flag::Pick, Flag::Reject];

    pub fn name(self) -> &'static str {
        match self {
            Flag::None => "Unflagged",
            Flag::Pick => "Pick",
            Flag::Reject => "Reject",
        }
    }
}

/// The color label. The five are Lightroom's, so a folder culled
/// there and a folder culled here mean the same thing by a red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Label {
    #[default]
    None,
    Red,
    Yellow,
    Green,
    Blue,
    Purple,
}

impl Label {
    /// The badge a browser row draws for this label, in the order
    /// the number keys set them. As [`Flag::code`], the `.slint`
    /// file matches on the number.
    pub fn code(self) -> i32 {
        match self {
            Label::None => 0,
            Label::Red => 1,
            Label::Yellow => 2,
            Label::Green => 3,
            Label::Blue => 4,
            Label::Purple => 5,
        }
    }

    pub const ALL: [Label; 6] = [
        Label::None,
        Label::Red,
        Label::Yellow,
        Label::Green,
        Label::Blue,
        Label::Purple,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Label::None => "None",
            Label::Red => "Red",
            Label::Yellow => "Yellow",
            Label::Green => "Green",
            Label::Blue => "Blue",
            Label::Purple => "Purple",
        }
    }
}

/// A frame's meta, beside its edit.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    /// Stars, 0 to [`STARS`]. Zero is unrated.
    #[serde(deserialize_with = "rating")]
    pub rating: u8,
    #[serde(deserialize_with = "loose")]
    pub flag: Flag,
    #[serde(deserialize_with = "loose")]
    pub label: Label,
    /// Keywords, in the order they were put on, each one once.
    #[serde(deserialize_with = "keywords")]
    pub keywords: Vec<String>,
    #[serde(deserialize_with = "text")]
    pub title: String,
    #[serde(deserialize_with = "text")]
    pub caption: String,
}

/// A rating as a file gives it: held to [`STARS`], and none of them
/// when it is not a count at all. Read wide, so that a rating of a
/// thousand is five stars rather than nothing.
fn rating<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    Ok(loose::<D, u64>(d)?.min(STARS as u64) as u8)
}

/// Words as read from a file: trimmed, since a sidecar can be edited
/// by hand and a padded caption is not a different caption. Done on
/// the way in, so that the `.gcd` and an XMP saying the same thing
/// with different padding agree and neither churns a save.
fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(loose::<D, String>(d)?.trim().to_string())
}

/// Keywords as read from a file: normalized, since a sidecar can be
/// edited by hand and a duplicate or a stray space is not worth
/// carrying into the session.
fn keywords<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(normalize(loose::<D, Vec<String>>(d)?))
}

/// Trimmed, the empty ones dropped, the first of each kept. Two
/// keywords that differ only in case are one keyword: a folder with
/// both "Sunset" and "sunset" in it is a mistake nobody makes on
/// purpose and nobody enjoys finding later.
fn normalize(words: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    for word in words {
        let word = word.trim();
        if word.is_empty() || out.iter().any(|k| k.eq_ignore_ascii_case(word)) {
            continue;
        }
        out.push(word.to_string());
    }
    out
}

impl Meta {
    /// Nothing said about this frame at all.
    pub fn is_empty(&self) -> bool {
        *self == Meta::default()
    }

    /// Set the stars, held to the range.
    pub fn set_rating(&mut self, stars: u8) {
        self.rating = stars.min(STARS);
    }

    /// Put a keyword on, at the end; false when it is already there
    /// or is nothing but space.
    pub fn add_keyword(&mut self, word: &str) -> bool {
        let word = word.trim();
        if word.is_empty() || self.keywords.iter().any(|k| k.eq_ignore_ascii_case(word)) {
            return false;
        }
        self.keywords.push(word.to_string());
        true
    }

    /// Take a keyword off, whatever its case; false when it was not
    /// there.
    pub fn remove_keyword(&mut self, word: &str) -> bool {
        let word = word.trim();
        let before = self.keywords.len();
        self.keywords.retain(|k| !k.eq_ignore_ascii_case(word));
        self.keywords.len() != before
    }

    /// Set the title, trimmed as a file's is.
    pub fn set_title(&mut self, text: &str) {
        self.title = text.trim().to_string();
    }

    /// Set the caption, trimmed as a file's is.
    pub fn set_caption(&mut self, text: &str) {
        self.caption = text.trim().to_string();
    }

    /// Replace the keywords, normalized as a file's are.
    pub fn set_keywords(&mut self, words: Vec<String>) {
        self.keywords = normalize(words);
    }
}

/// What one key press does to the meta of the frames it is aimed at.
///
/// A press carries a whole selection, even where there is only one
/// frame under it today: the browser has no multi-select yet, and
/// when it grows one nothing here has to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Stars, 0 for none.
    Rating(u8),
    Flag(Flag),
    /// A label key, which is a toggle: see [`Change::settled`].
    Label(Label),
}

impl Change {
    /// The change a key stands for: 1 to 5 the stars and 0 none of
    /// them, P a pick, X a reject, U neither, 6 to 9 red, yellow,
    /// green and blue.
    ///
    /// Purple has no key. Lightroom gives it none either — 0 is the
    /// rating's zero there, and taking it for the sixth label would
    /// cost the one key that clears a rating. It is in the schema
    /// and it reads back; nothing in the browser sets it yet.
    pub fn from_key(key: &str) -> Option<Change> {
        Some(match key {
            "0" => Change::Rating(0),
            "1" => Change::Rating(1),
            "2" => Change::Rating(2),
            "3" => Change::Rating(3),
            "4" => Change::Rating(4),
            "5" => Change::Rating(5),
            "6" => Change::Label(Label::Red),
            "7" => Change::Label(Label::Yellow),
            "8" => Change::Label(Label::Green),
            "9" => Change::Label(Label::Blue),
            "p" | "P" => Change::Flag(Flag::Pick),
            "x" | "X" => Change::Flag(Flag::Reject),
            "u" | "U" => Change::Flag(Flag::None),
            _ => return None,
        })
    }

    /// The same change, with a label key settled against the frames
    /// it is about to be applied to: a label key clears the label
    /// when every one of them already carries it, and sets it
    /// otherwise.
    ///
    /// That is the rule that makes one press on a mixed selection
    /// mean something. Deciding frame by frame would leave the set
    /// half red and half bare, which is neither of the two things
    /// the press could have meant. Ratings and flags are not
    /// toggles: the key names the value it writes.
    pub fn settled<'a>(self, frames: impl IntoIterator<Item = &'a Meta>) -> Change {
        let Change::Label(label) = self else {
            return self;
        };
        if label == Label::None {
            return self;
        }
        let mut any = false;
        let all = frames.into_iter().all(|m| {
            any = true;
            m.label == label
        });
        if any && all {
            Change::Label(Label::None)
        } else {
            self
        }
    }

    /// Put this change into one frame's meta; true when it changed
    /// anything.
    pub fn apply(self, meta: &mut Meta) -> bool {
        let before = meta.rating;
        match self {
            Change::Rating(stars) => {
                meta.set_rating(stars);
                meta.rating != before
            }
            Change::Flag(flag) => std::mem::replace(&mut meta.flag, flag) != flag,
            Change::Label(label) => std::mem::replace(&mut meta.label, label) != label,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The browser's badges are drawn from these numbers in
    /// `app.slint`, so they are part of the meta's contract and not
    /// the enum's declaration order.
    #[test]
    fn the_badges_number_the_flags_and_the_labels() {
        assert_eq!(Flag::None.code(), 0);
        assert_eq!(Flag::Pick.code(), 1);
        assert_eq!(Flag::Reject.code(), 2);
        assert_eq!(Label::None.code(), 0);
        assert_eq!(Label::Red.code(), 1);
        assert_eq!(Label::Yellow.code(), 2);
        assert_eq!(Label::Green.code(), 3);
        assert_eq!(Label::Blue.code(), 4);
        assert_eq!(Label::Purple.code(), 5);
        // Nothing is nothing for both, and the rest are distinct.
        let codes: Vec<i32> = Label::ALL.iter().map(|l| l.code()).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "{codes:?}");
    }

    #[test]
    fn the_default_meta_says_nothing() {
        let meta = Meta::default();
        assert!(meta.is_empty());
        assert_eq!(meta.rating, 0);
        assert_eq!(meta.flag, Flag::None);
        assert_eq!(meta.label, Label::None);
        assert!(meta.keywords.is_empty());
        assert!(meta.title.is_empty() && meta.caption.is_empty());
    }

    #[test]
    fn meta_round_trips_through_json() {
        let mut meta = Meta {
            rating: 4,
            flag: Flag::Pick,
            label: Label::Yellow,
            title: "Low tide".into(),
            caption: "Third of the morning's set.".into(),
            ..Meta::default()
        };
        meta.set_keywords(vec!["Sea".into(), "Rocks".into()]);
        let json = serde_json::to_string(&meta).unwrap();
        assert_eq!(serde_json::from_str::<Meta>(&json).unwrap(), meta);
        // The words on the wire are the words a person would write.
        assert!(json.contains(r#""flag":"pick""#), "{json}");
        assert!(json.contains(r#""label":"yellow""#), "{json}");
    }

    #[test]
    fn a_meta_with_fields_missing_reads_as_the_default() {
        let meta: Meta = serde_json::from_str(r#"{"rating":3}"#).unwrap();
        assert_eq!(meta.rating, 3);
        assert_eq!(meta.flag, Flag::None);
        assert_eq!(meta.label, Label::None);
        assert_eq!(Meta::default(), serde_json::from_str("{}").unwrap());
    }

    #[test]
    fn keywords_are_ordered_and_each_one_is_there_once() {
        let mut meta = Meta::default();
        assert!(meta.add_keyword("sea"));
        assert!(meta.add_keyword("rocks"));
        assert!(!meta.add_keyword("Sea"));
        assert!(!meta.add_keyword("  "));
        assert_eq!(meta.keywords, ["sea", "rocks"]);
        assert!(meta.remove_keyword("SEA"));
        assert!(!meta.remove_keyword("sea"));
        assert_eq!(meta.keywords, ["rocks"]);
        // A file edited by hand is tidied on the way in.
        let meta: Meta =
            serde_json::from_str(r#"{"keywords":[" sea ","sea","","Sea","rocks"]}"#).unwrap();
        assert_eq!(meta.keywords, ["sea", "rocks"]);
    }

    #[test]
    fn a_rating_is_held_to_five_stars() {
        let mut meta = Meta::default();
        meta.set_rating(9);
        assert_eq!(meta.rating, STARS);
        // And from a file, whatever is written there.
        let stars = |json: &str| serde_json::from_str::<Meta>(json).unwrap().rating;
        assert_eq!(stars(r#"{"rating":200}"#), STARS);
        assert_eq!(stars(r#"{"rating":100000}"#), STARS);
        assert_eq!(stars(r#"{"rating":3}"#), 3);
        assert_eq!(stars(r#"{"rating":-1}"#), 0);
        assert_eq!(stars(r#"{"rating":"four"}"#), 0);
    }

    #[test]
    fn a_value_this_build_cannot_read_costs_that_field_and_nothing_else() {
        // A later build's sixth label, a flag spelled by hand, a
        // keyword list that is one word: the field defaults, the rest
        // of the section stands, and the read does not fail.
        let meta: Meta = serde_json::from_str(
            r#"{"label":"grey","flag":"picked","keywords":"sea","rating":4,"title":"Low tide"}"#,
        )
        .unwrap();
        assert_eq!(meta.label, Label::None);
        assert_eq!(meta.flag, Flag::None);
        assert!(meta.keywords.is_empty());
        assert_eq!(meta.rating, 4);
        assert_eq!(meta.title, "Low tide");
        // A title that is not words, and a whole section that is not
        // a section: an empty meta, not an error.
        let meta: Meta = serde_json::from_str(r#"{"title":7,"caption":[]}"#).unwrap();
        assert!(meta.is_empty());
        assert_eq!(
            loose_meta(&mut serde_json::Deserializer::from_str("42")).unwrap(),
            Meta::default()
        );
        let nested: Meta =
            loose_meta(&mut serde_json::Deserializer::from_str(r#"{"rating":2}"#)).unwrap();
        assert_eq!(nested.rating, 2);
    }

    #[test]
    fn the_keys_are_the_ones_a_culler_knows() {
        assert_eq!(Change::from_key("3"), Some(Change::Rating(3)));
        assert_eq!(Change::from_key("0"), Some(Change::Rating(0)));
        assert_eq!(Change::from_key("P"), Some(Change::Flag(Flag::Pick)));
        assert_eq!(Change::from_key("x"), Some(Change::Flag(Flag::Reject)));
        assert_eq!(Change::from_key("u"), Some(Change::Flag(Flag::None)));
        assert_eq!(Change::from_key("6"), Some(Change::Label(Label::Red)));
        assert_eq!(Change::from_key("9"), Some(Change::Label(Label::Blue)));
        assert_eq!(Change::from_key("g"), None);
        assert_eq!(Change::from_key(""), None);
    }

    #[test]
    fn a_change_writes_the_value_its_key_names() {
        let mut meta = Meta::default();
        assert!(Change::Rating(4).apply(&mut meta));
        assert_eq!(meta.rating, 4);
        assert!(!Change::Rating(4).apply(&mut meta));
        assert!(Change::Rating(0).apply(&mut meta));
        assert!(Change::Flag(Flag::Reject).apply(&mut meta));
        assert_eq!(meta.flag, Flag::Reject);
        assert!(Change::Flag(Flag::None).apply(&mut meta));
        // The stars and the flag are not each other's business.
        meta.rating = 2;
        Change::Flag(Flag::Pick).apply(&mut meta);
        assert_eq!(meta.rating, 2);
    }

    #[test]
    fn a_label_key_clears_the_label_the_whole_selection_carries() {
        let red = Meta {
            label: Label::Red,
            ..Meta::default()
        };
        let bare = Meta::default();
        // All red already: the press takes it off.
        let all_red = [red.clone(), red.clone()];
        assert_eq!(
            Change::Label(Label::Red).settled(all_red.iter()),
            Change::Label(Label::None)
        );
        // Mixed, or all some other label: the press puts it on.
        let mixed = [red.clone(), bare.clone()];
        assert_eq!(
            Change::Label(Label::Red).settled(mixed.iter()),
            Change::Label(Label::Red)
        );
        assert_eq!(
            Change::Label(Label::Blue).settled(all_red.iter()),
            Change::Label(Label::Blue)
        );
        // An empty selection settles to itself, and a rating key is
        // never a toggle.
        assert_eq!(
            Change::Label(Label::Red).settled(std::iter::empty()),
            Change::Label(Label::Red)
        );
        assert_eq!(Change::Rating(0).settled(all_red.iter()), Change::Rating(0));
    }
}
