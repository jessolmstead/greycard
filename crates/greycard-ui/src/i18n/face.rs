//! Han, kana, and hangul in the typeface of the window's language's own
//! region. Japanese, Chinese, and Korean share thousands of ideographs
//! whose standard forms differ by region (直, 骨, and 次 are the usual
//! examples), and a CJK font family is cut once a region. Slint shapes
//! text with no locale, so the system's fallback picks one CJK family
//! for every Han string; with fontconfig that is whichever ranks first
//! (here the Korean cut), and Japanese or Chinese text shows another
//! region's forms.
//!
//! We bundle no font. When the window's language is Japanese, Chinese,
//! or Korean we look at the generic families Slint asks for, in its
//! order, and find the first that draws the language. When that is a
//! known cut of another region, or an unknown family missing some of
//! the region's sample characters (a Chinese font, for Japanese), we
//! place the region's own face (the first of a preference list a
//! platform that is installed and maps all the samples) just before it;
//! when none draws the language, after them all. When it is the
//! region's own cut, or a family no table knows that maps all the
//! samples (one the user chose, say Sarasa Gothic), we leave the lists
//! alone. The families before the region's face, the
//! system's Latin UI face first, keep every character they cover, so
//! Latin text and digits look as in English. Fontique walks the
//! generic families before its per-script fallback, so on Linux, where
//! fontconfig puts a CJK family in the generic list itself, a fallback
//! entry alone would never be reached.
//!
//! The change goes through fontique's shared collection, the way Slint
//! offers for adding coverage at run time, once, at the start. Every
//! other language leaves the collection untouched.

use slint::fontique_012::fontique::{Collection, FallbackKey, FamilyId, GenericFamily, Script};

/// The four writing regions a CJK family is cut for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Region {
    Japan,
    SimplifiedChinese,
    TraditionalChinese,
    Korea,
}

impl Region {
    const ALL: [Region; 4] = [
        Region::Japan,
        Region::SimplifiedChinese,
        Region::TraditionalChinese,
        Region::Korea,
    ];

    /// The region a language's text is written for, from a catalog's
    /// tag or a locale (`ja`, `zh_CN`, `zh-Hant`, `zh_HK`, `ko_KR`):
    /// Chinese by its script when one is given, else by its region;
    /// none for any other language, English (empty) included.
    pub(crate) fn of(tag: &str) -> Option<Self> {
        let tag = tag.replace('-', "_").to_ascii_lowercase();
        let mut parts = tag.split(['_', '.', '@']);
        match parts.next()? {
            "ja" => Some(Self::Japan),
            "ko" => Some(Self::Korea),
            "zh" => {
                let parts: Vec<&str> = parts.collect();
                let traditional = !parts.contains(&"hans")
                    && parts
                        .iter()
                        .any(|p| matches!(*p, "tw" | "hk" | "mo" | "hant"));
                Some(if traditional {
                    Self::TraditionalChinese
                } else {
                    Self::SimplifiedChinese
                })
            }
            _ => None,
        }
    }

    /// The language in words, for the log.
    fn words(self) -> &'static str {
        match self {
            Self::Japan => "Japanese",
            Self::SimplifiedChinese => "Simplified Chinese",
            Self::TraditionalChinese => "Traditional Chinese",
            Self::Korea => "Korean",
        }
    }

    /// Characters the region's own fonts have and the commonest wrong
    /// region's lack, as far as single characters can tell: a family
    /// mapping all of them is taken for a font of the language. Kana
    /// alone tells nothing (KS X 1001 and GB2312 carry it), so Japanese
    /// adds the kokuji 込, which no Korean or Chinese legacy set has;
    /// 这 is a simplified form; 們 is in Big5 and not in JIS X 0208 or
    /// GB2312; hangul is in no Japanese or Chinese legacy set. Korean
    /// asks no ideograph, as a Korean UI face may carry no hanja. A font
    /// wider than the legacy sets passes for more than its region: a
    /// JIS X 0213 or Adobe-Japan1-6 Japanese font has 們, and a pan-CJK
    /// or GB18030 font with kana and hangul has everything. Only the
    /// tables tell those apart.
    fn samples(self) -> &'static [char] {
        match self {
            Self::Japan => &['あ', '込', IDEOGRAPH],
            Self::SimplifiedChinese => &['这', IDEOGRAPH],
            Self::TraditionalChinese => &['們', IDEOGRAPH],
            Self::Korea => &['한'],
        }
    }

    /// The fallback keys of the region's scripts: what the system would
    /// draw the language with when no generic family covers it.
    fn scripts(self) -> &'static [&'static [u8; 4]] {
        match self {
            Self::Japan => &[b"Hira", b"Kana", b"Hani"],
            Self::SimplifiedChinese | Self::TraditionalChinese => &[b"Hani"],
            Self::Korea => &[b"Hang", b"Hani"],
        }
    }
}

/// A platform's preferred families for each region, best first.
pub(crate) struct Table {
    japan: &'static [&'static str],
    simplified_chinese: &'static [&'static str],
    traditional_chinese: &'static [&'static str],
    korea: &'static [&'static str],
}

impl Table {
    fn families(&self, region: Region) -> &'static [&'static str] {
        match region {
            Region::Japan => self.japan,
            Region::SimplifiedChinese => self.simplified_chinese,
            Region::TraditionalChinese => self.traditional_chinese,
            Region::Korea => self.korea,
        }
    }
}

/// Linux and the other fontconfig systems: Noto Sans CJK as the
/// distributions package it, then Source Han Sans (the same design
/// under Adobe's name: its region subsets' names, old and new, and its
/// collection's, whose unsuffixed family is the Japanese), then the
/// common region-only families.
pub(crate) const LINUX: Table = Table {
    japan: &[
        "Noto Sans CJK JP",
        "Noto Sans JP",
        "Source Han Sans JP",
        "Source Han Sans",
        "IPAexGothic",
        "IPAGothic",
        "TakaoGothic",
        "VL Gothic",
    ],
    simplified_chinese: &[
        "Noto Sans CJK SC",
        "Noto Sans SC",
        "Source Han Sans SC",
        "Source Han Sans CN",
        "WenQuanYi Micro Hei",
        "WenQuanYi Zen Hei",
        "Droid Sans Fallback",
    ],
    traditional_chinese: &[
        "Noto Sans CJK TC",
        "Noto Sans TC",
        "Source Han Sans TC",
        "Source Han Sans TW",
        "Noto Sans CJK HK",
        "Source Han Sans HC",
    ],
    korea: &[
        "Noto Sans CJK KR",
        "Noto Sans KR",
        "Source Han Sans KR",
        "Source Han Sans K",
        "NanumGothic",
        "UnDotum",
        "Baekmuk Gulim",
    ],
};

/// Windows: the UI cuts of the families the system ships for each
/// region, then their document cuts.
pub(crate) const WINDOWS: Table = Table {
    japan: &[
        "Yu Gothic UI",
        "Meiryo UI",
        "Yu Gothic",
        "Meiryo",
        "MS UI Gothic",
    ],
    simplified_chinese: &[
        "Microsoft YaHei UI",
        "Microsoft YaHei",
        "DengXian",
        "SimHei",
    ],
    traditional_chinese: &["Microsoft JhengHei UI", "Microsoft JhengHei"],
    korea: &["Malgun Gothic", "Gulim", "Dotum"],
};

/// macOS: the system's own region families, then the older ones.
pub(crate) const MACOS: Table = Table {
    japan: &[
        "Hiragino Sans",
        "Hiragino Kaku Gothic ProN",
        "Hiragino Kaku Gothic Pro",
    ],
    simplified_chinese: &["PingFang SC", "Hiragino Sans GB", "Heiti SC", "STHeiti"],
    traditional_chinese: &["PingFang TC", "Heiti TC"],
    korea: &["Apple SD Gothic Neo", "AppleGothic"],
};

/// This platform's table.
pub(crate) const PLATFORM: &Table = if cfg!(windows) {
    &WINDOWS
} else if cfg!(target_vendor = "apple") {
    &MACOS
} else {
    &LINUX
};

/// Any ideograph at all: a family mapping it, or any of a region's
/// samples, draws some of the language.
const IDEOGRAPH: char = '一';

/// The table's families for the region, as `resolve` finds them
/// installed (by any of their names, and so by id), in the table's
/// order, each once.
pub(crate) fn choices<T: PartialEq>(
    table: &Table,
    region: Region,
    mut resolve: impl FnMut(&str) -> Option<T>,
) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for name in table.families(region) {
        if let Some(id) = resolve(name)
            && !out.contains(&id)
        {
            out.push(id);
        }
    }
    out
}

/// Which regions' tables hold the family `id`, matched by `resolve`
/// (by id, so a family whose first name is localized, 文泉驿微米黑 for
/// WenQuanYi Micro Hei, is still known): its own region's, and another's.
pub(crate) fn kinship<T: PartialEq>(
    table: &Table,
    region: Region,
    id: &T,
    mut resolve: impl FnMut(&str) -> Option<T>,
) -> (bool, bool) {
    let mut held = |r: Region| {
        table
            .families(r)
            .iter()
            .any(|name| resolve(name).as_ref() == Some(id))
    };
    let own = held(region);
    let another = Region::ALL.iter().any(|r| *r != region && held(*r));
    (own, another)
}

/// The first generic family that draws the language, as [`step`] sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct First {
    /// In the region's own table.
    pub(crate) own: bool,
    /// In another region's table.
    pub(crate) another: bool,
    /// Maps every one of the region's samples.
    pub(crate) full: bool,
}

/// What to do about the first generic family that draws the language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// None draws it: the region's face goes after them all.
    Append,
    /// Another region's cut, known by the tables or by missing some of
    /// the region's samples: the region's face goes before it.
    Before,
    /// The region's own cut, or one no table knows that maps all the
    /// samples (the user's choice): the lists stay.
    Leave,
}

/// [`Step`] for the first family drawing the language.
pub(crate) fn step(first: Option<First>) -> Step {
    match first {
        None => Step::Append,
        Some(First { own: true, .. }) => Step::Leave,
        Some(First { another: true, .. }) => Step::Before,
        Some(First { full: true, .. }) => Step::Leave,
        Some(_) => Step::Before,
    }
}

/// What [`arrange`] found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The region's face, named, placed in Slint's generic families.
    Placed(String),
    /// The lists left as they were: the first family covering the
    /// language, named, is the region's own or the user's choice.
    Kept(String),
    /// No face of the region is installed; another family covers the
    /// language, whose forms may be another region's.
    Elsewhere,
    /// No family covers the language: its text shows as boxes.
    Uncovered,
}

/// The generic families Slint asks for, in its order. They mirror
/// Slint's `FALLBACK_FAMILIES` (i-slint-common 1.19,
/// `sharedfontique.rs:221`), which Slint does not export; a test holds
/// the two together, so a Slint bump that changes it fails there.
pub(crate) const GENERICS: [GenericFamily; 2] = [GenericFamily::SystemUi, GenericFamily::SansSerif];

/// Rearrange `collection`'s generic families for the language of `tag`
/// by `table`; none, and nothing touched, for a language with no CJK
/// region.
///
/// The changed list is set as the collection's own. A collection made
/// with the system's fonts (Slint's is) still yields the system's list
/// after it, so a query walks ours, then the system's original again.
/// That is harmless: the second walk holds the same families, so it
/// can match nothing the first did not, and is reached only for a
/// character no family covers. Fontique has no way to replace the
/// system's part.
pub(crate) fn arrange(collection: &mut Collection, tag: &str, table: &Table) -> Option<Outcome> {
    let region = Region::of(tag)?;
    let face = region_face(collection, region, table);
    let lists: Vec<(GenericFamily, Vec<FamilyId>)> = GENERICS
        .iter()
        .map(|g| (*g, collection.generic_families(*g).collect()))
        .collect();
    // The first family drawing the language, in the order Slint asks.
    let mut first = None;
    for f in lists.iter().flat_map(|(_, list)| list.iter().copied()) {
        if draws(collection, f, region) {
            first = Some(f);
            break;
        }
    }
    let Some(face) = face else {
        return Some(if first.is_some() || by_fallback(collection, region) {
            Outcome::Elsewhere
        } else {
            Outcome::Uncovered
        });
    };
    let seen = first.map(|f| {
        let (own, another) = kinship(table, region, &f, |name| collection.family_id(name));
        let full = full(collection, f, region);
        First { own, another, full }
    });
    if step(seen) == Step::Leave {
        let name = first.and_then(|f| collection.family_name(f));
        return Some(Outcome::Kept(name.unwrap_or("").to_owned()));
    }
    // Into the first list that has families: one ahead of the other in
    // Slint's order, so a face at its end comes before all the second's.
    let (generic, list) = lists
        .into_iter()
        .find(|(_, list)| !list.is_empty())
        .unwrap_or((GENERICS[0], Vec::new()));
    let mut new: Vec<FamilyId> = list.into_iter().filter(|f| *f != face).collect();
    let at = first
        .and_then(|f| new.iter().position(|x| *x == f))
        .unwrap_or(new.len());
    new.insert(at, face);
    collection.set_generic_families(generic, new.into_iter());
    Some(Outcome::Placed(
        collection.family_name(face).unwrap_or("").to_owned(),
    ))
}

/// The first of the table's families for the region that is installed
/// and maps every one of its samples.
fn region_face(collection: &mut Collection, region: Region, table: &Table) -> Option<FamilyId> {
    let ids = choices(table, region, |name| collection.family_id(name));
    ids.into_iter().find(|id| full(collection, *id, region))
}

/// Whether the family draws some of the language: maps one of its
/// samples, or any ideograph.
fn draws(collection: &mut Collection, family: FamilyId, region: Region) -> bool {
    coverage(collection, family, region.samples()) > 0
        || coverage(collection, family, &[IDEOGRAPH]) > 0
}

/// Whether the family maps every one of the region's samples.
fn full(collection: &mut Collection, family: FamilyId, region: Region) -> bool {
    let samples = region.samples();
    coverage(collection, family, samples) == samples.len()
}

/// Whether the system's fallback for the region's scripts draws the
/// language.
fn by_fallback(collection: &mut Collection, region: Region) -> bool {
    let mut families: Vec<FamilyId> = Vec::new();
    for script in region.scripts() {
        families.extend(
            collection.fallback_families(FallbackKey::new(Script::from_bytes(**script), None)),
        );
    }
    families.into_iter().any(|f| draws(collection, f, region))
}

/// How many of `chars` the family's default face maps to a glyph.
fn coverage(collection: &mut Collection, family: FamilyId, chars: &[char]) -> usize {
    let Some(info) = collection.family(family) else {
        return 0;
    };
    let Some(font) = info.default_font() else {
        return 0;
    };
    let Some(blob) = font.load(None) else {
        return 0;
    };
    let Some(map) = font.charmap_index().charmap(blob.as_ref()) else {
        return 0;
    };
    chars
        .iter()
        .filter(|c| map.map(**c).is_some_and(|glyph| glyph != 0))
        .count()
}

/// The log's line for a language no installed font covers; on Linux,
/// which font to install and where it is packaged.
fn uncovered(tag: &str, region: Region, linux: bool) -> String {
    let plain = format!("no installed font covers {tag} ({})", region.words());
    if linux {
        format!(
            "{plain}: its text shows as boxes. Install Noto Sans CJK, your distribution's \
             package for it (noto-fonts-cjk on Arch, fonts-noto-cjk on Debian and Ubuntu, \
             google-noto-sans-cjk-fonts on Fedora)"
        )
    } else {
        plain
    }
}

/// Put the region's face of the language of `tag` in force for the
/// window's text, once, at the start, when a component exists (Slint's
/// context is made with it). Any other language touches nothing.
pub fn apply(tag: &str) {
    let Some(region) = Region::of(tag) else {
        return;
    };
    let mut collection = slint::fontique_012::shared_collection();
    let words = region.words();
    match arrange(&mut collection, tag, PLATFORM) {
        Some(Outcome::Placed(face)) => tracing::info!("{words} text ({tag}) in {face}"),
        Some(Outcome::Kept(face)) => {
            tracing::info!("{words} text ({tag}) in {face}, the system's choice, kept")
        }
        Some(Outcome::Elsewhere) => tracing::info!(
            "no {words} typeface among the known ones; {tag} text takes the system's CJK \
             face, whose forms may be another region's"
        ),
        Some(Outcome::Uncovered) => {
            tracing::warn!("{}", uncovered(tag, region, cfg!(target_os = "linux")))
        }
        None => {}
    }
}

#[cfg(test)]
mod tests;
