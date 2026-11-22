use super::*;
use slint::fontique_012::fontique::{Blob, CollectionOptions, FontInfoOverride, SourceKind};
use std::sync::{Arc, OnceLock};

/// Every family of every table, as a machine with all of them has
/// them, the Latin faces first.
fn everything() -> Vec<&'static str> {
    let mut all = vec!["Noto Sans", "DejaVu Sans", "Segoe UI", "System Font"];
    for table in [&LINUX, &WINDOWS, &MACOS] {
        for region in Region::ALL {
            all.extend(table.families(region));
        }
    }
    all
}

/// The installed family a table's name finds among `installed`, ignoring
/// case, as fontique's lookup by name does.
fn among<'a>(installed: &'a [&'a str]) -> impl FnMut(&str) -> Option<&'a str> {
    |name| {
        installed
            .iter()
            .find(|i| i.eq_ignore_ascii_case(name))
            .copied()
    }
}

/// [`choices`] for the language of `tag`, among `installed`.
fn picks<'a>(table: &Table, tag: &str, installed: &'a [&'a str]) -> Vec<&'a str> {
    Region::of(tag).map_or_else(Vec::new, |r| choices(table, r, among(installed)))
}

/// A collection of the test's own, not Slint's, and without the
/// system's fonts: its generic lists hold only what the test sets.
fn bare() -> Collection {
    Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    })
}

/// A collection of the system's fonts, not Slint's.
fn system() -> Collection {
    Collection::new(CollectionOptions {
        shared: false,
        system_fonts: true,
    })
}

fn lists(c: &mut Collection) -> Vec<Vec<FamilyId>> {
    GENERICS
        .iter()
        .map(|g| c.generic_families(*g).collect())
        .collect()
}

fn set(c: &mut Collection, system_ui: &[FamilyId], sans_serif: &[FamilyId]) {
    c.set_generic_families(GenericFamily::SystemUi, system_ui.iter().copied());
    c.set_generic_families(GenericFamily::SansSerif, sans_serif.iter().copied());
}

/// The file of the system's family `name`'s default face.
fn file(system: &mut Collection, name: &str) -> Option<Blob<u8>> {
    let id = system.family_id(name)?;
    let info = system.family(id)?;
    let SourceKind::Path(path) = &info.default_font()?.source().kind else {
        return None;
    };
    let bytes = std::fs::read(path).ok()?;
    Some(Blob::new(Arc::new(bytes)))
}

/// The Latin faces a test may take, in this order: the first the system
/// has that maps Latin and no ideograph.
const LATIN: [&str; 6] = [
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "Cantarell",
    "Arial",
    "Helvetica",
];

/// The font files the tests that draw on real faces register: a Latin
/// face, by name, and Noto Sans CJK's Japanese, Korean, and Simplified
/// Chinese cuts (one file or three), read
/// from the system's fonts. None on a machine without them.
struct Files {
    latin_name: &'static str,
    latin: Blob<u8>,
    cjk: Vec<Blob<u8>>,
}

fn files() -> Option<&'static Files> {
    static FILES: OnceLock<Option<Files>> = OnceLock::new();
    FILES
        .get_or_init(|| {
            let mut system = system();
            let mut cjk: Vec<Blob<u8>> = Vec::new();
            for name in ["Noto Sans CJK JP", "Noto Sans CJK KR", "Noto Sans CJK SC"] {
                let blob = file(&mut system, name)?;
                if !cjk.iter().any(|b| b.as_ref() == blob.as_ref()) {
                    cjk.push(blob);
                }
            }
            let latin_name = LATIN.into_iter().find(|name| {
                system.family_id(name).is_some_and(|id| {
                    coverage(&mut system, id, &['A']) == 1
                        && coverage(&mut system, id, &[IDEOGRAPH]) == 0
                })
            })?;
            let latin = file(&mut system, latin_name)?;
            Some(Files {
                latin_name,
                latin,
                cjk,
            })
        })
        .as_ref()
}

fn named(name: &str) -> Option<FontInfoOverride<'_>> {
    Some(FontInfoOverride {
        family_name: Some(name),
        ..Default::default()
    })
}

/// The faces a test arranges, registered in a bare collection.
struct Faces {
    c: Collection,
    latin: FamilyId,
    jp: FamilyId,
    kr: FamilyId,
    sc: FamilyId,
    /// The CJK cuts under a name no table knows: a user's own choice.
    own: FamilyId,
}

/// [`Faces`], or none (with a note) on a machine without Noto Sans
/// CJK, where CI runs: the tests that need it pass there unrun.
fn faces(test: &str) -> Option<Faces> {
    let found = files().and_then(|files| {
        let mut c = bare();
        c.register_fonts(files.latin.clone(), None);
        for blob in &files.cjk {
            c.register_fonts(blob.clone(), None);
        }
        c.register_fonts(files.cjk[0].clone(), named("Own Gothic"));
        Some(Faces {
            latin: c.family_id(files.latin_name)?,
            jp: c.family_id("Noto Sans CJK JP")?,
            kr: c.family_id("Noto Sans CJK KR")?,
            sc: c.family_id("Noto Sans CJK SC")?,
            own: c.family_id("Own Gothic")?,
            c,
        })
    });
    if found.is_none() {
        eprintln!("{test}: skipped, no Noto Sans CJK (JP, KR, and SC) installed");
    }
    found
}

#[test]
fn a_tag_names_its_region() {
    assert_eq!(Region::of("ja"), Some(Region::Japan));
    assert_eq!(Region::of("ja_JP.UTF-8"), Some(Region::Japan));
    assert_eq!(Region::of("ko"), Some(Region::Korea));
    assert_eq!(Region::of("zh_CN"), Some(Region::SimplifiedChinese));
    assert_eq!(Region::of("zh-Hans"), Some(Region::SimplifiedChinese));
    assert_eq!(Region::of("zh"), Some(Region::SimplifiedChinese));
    assert_eq!(Region::of("zh_TW"), Some(Region::TraditionalChinese));
    assert_eq!(Region::of("zh-Hant"), Some(Region::TraditionalChinese));
    assert_eq!(Region::of("zh_HK"), Some(Region::TraditionalChinese));
    assert_eq!(Region::of("zh_MO.UTF-8"), Some(Region::TraditionalChinese));
    assert_eq!(Region::of("zh-Hant-HK"), Some(Region::TraditionalChinese));
    // A script given outweighs the region's usual one.
    assert_eq!(Region::of("zh-Hans-HK"), Some(Region::SimplifiedChinese));
    assert_eq!(Region::of("zh_SG"), Some(Region::SimplifiedChinese));
    assert_eq!(Region::of(""), None);
    assert_eq!(Region::of("jv"), None);
}

#[test]
fn each_platform_prefers_its_regions_first_choice() {
    let all = everything();
    let cases: [(&Table, [&str; 4]); 3] = [
        (
            &LINUX,
            [
                "Noto Sans CJK JP",
                "Noto Sans CJK SC",
                "Noto Sans CJK TC",
                "Noto Sans CJK KR",
            ],
        ),
        (
            &WINDOWS,
            [
                "Yu Gothic UI",
                "Microsoft YaHei UI",
                "Microsoft JhengHei UI",
                "Malgun Gothic",
            ],
        ),
        (
            &MACOS,
            [
                "Hiragino Sans",
                "PingFang SC",
                "PingFang TC",
                "Apple SD Gothic Neo",
            ],
        ),
    ];
    for (table, [ja, zh_cn, zh_tw, ko]) in cases {
        let first = |tag| picks(table, tag, &all).first().copied();
        assert_eq!(first("ja"), Some(ja));
        assert_eq!(first("zh_CN"), Some(zh_cn));
        assert_eq!(first("zh-Hans"), Some(zh_cn));
        assert_eq!(first("zh_TW"), Some(zh_tw));
        assert_eq!(first("zh-Hant"), Some(zh_tw));
        assert_eq!(first("ko"), Some(ko));
    }
}

#[test]
fn the_choices_are_the_installed_ones_in_the_tables_order() {
    // Source Han Sans on its own, as Adobe's release installs it.
    let installed = ["Noto Sans", "Source Han Sans SC", "Source Han Sans JP"];
    assert_eq!(picks(&LINUX, "ja", &installed), ["Source Han Sans JP"]);
    assert_eq!(picks(&LINUX, "zh_CN", &installed), ["Source Han Sans SC"]);
    // Only the Korean cut: no choice for the others.
    let installed = ["Noto Sans", "Noto Sans CJK KR"];
    assert_eq!(picks(&LINUX, "ko", &installed), ["Noto Sans CJK KR"]);
    assert!(picks(&LINUX, "ja", &installed).is_empty());
    // An older Windows with Meiryo and no Yu Gothic, in the table's order.
    let installed = ["Segoe UI", "Meiryo", "Meiryo UI"];
    assert_eq!(picks(&WINDOWS, "ja", &installed), ["Meiryo UI", "Meiryo"]);
    // A family is matched whatever the case of its name.
    let installed = ["noto sans cjk jp"];
    assert_eq!(picks(&LINUX, "ja", &installed), ["noto sans cjk jp"]);
    // No CJK face at all.
    let installed = ["Noto Sans", "DejaVu Sans", "Segoe UI", "Helvetica"];
    for table in [&LINUX, &WINDOWS, &MACOS] {
        for tag in ["ja", "zh_CN", "zh_TW", "ko"] {
            assert!(picks(table, tag, &installed).is_empty(), "{tag}");
        }
    }
}

/// A lookup by name that follows aliases to one id, as fontique's does:
/// WenQuanYi Micro Hei's first name is its Chinese one.
fn wenquanyi(name: &str) -> Option<u32> {
    match name.to_lowercase().as_str() {
        "文泉驿微米黑" | "wenquanyi micro hei" => Some(7),
        "noto sans cjk jp" => Some(1),
        _ => None,
    }
}

#[test]
fn a_family_is_known_by_its_id_whatever_its_first_name() {
    assert_eq!(choices(&LINUX, Region::SimplifiedChinese, wenquanyi), [7]);
    // For Japanese it is another region's cut, for Chinese the region's
    // own.
    assert_eq!(kinship(&LINUX, Region::Japan, &7, wenquanyi), (false, true));
    assert_eq!(
        kinship(&LINUX, Region::SimplifiedChinese, &7, wenquanyi),
        (true, false)
    );
    assert_eq!(kinship(&LINUX, Region::Japan, &1, wenquanyi), (true, false));
    assert_eq!(
        kinship(&LINUX, Region::Japan, &9, wenquanyi),
        (false, false)
    );
}

#[test]
fn every_other_language_leaves_the_collection_alone() {
    let mut tags: Vec<&str> = vec![
        "", "en", "en_US", "ca", "cs", "da", "de", "el", "es", "fi", "fr", "hu", "it", "nb", "nl",
        "pl", "pt", "pt_BR", "ro", "ru", "sk", "sv", "tr", "uk", "vi",
    ];
    for tag in &tags {
        assert_eq!(Region::of(tag), None, "{tag}");
    }
    tags.extend(
        crate::i18n::languages()
            .iter()
            .map(|(tag, _)| *tag)
            .filter(|tag| Region::of(tag).is_none()),
    );
    let all = everything();
    let mut c = bare();
    let (a, b) = (FamilyId::new(), FamilyId::new());
    set(&mut c, &[a, b], &[b]);
    for tag in tags {
        for table in [&LINUX, &WINDOWS, &MACOS] {
            assert!(picks(table, tag, &all).is_empty(), "{tag}");
            assert_eq!(arrange(&mut c, tag, table), None, "{tag}");
        }
        assert_eq!(lists(&mut c), [vec![a, b], vec![b]], "{tag}");
    }
}

#[test]
fn the_first_drawing_family_decides_the_step() {
    let first = |own, another, full| Some(First { own, another, full });
    // None draws the language: after them all.
    assert_eq!(step(None), Step::Append);
    // Another region's known cut, whatever it maps: before it.
    assert_eq!(step(first(false, true, true)), Step::Before);
    assert_eq!(step(first(false, true, false)), Step::Before);
    // The region's own cut, even one listed for another region too: left.
    assert_eq!(step(first(true, false, true)), Step::Leave);
    assert_eq!(step(first(true, true, true)), Step::Leave);
    // A family no table knows: the user's choice when it maps all the
    // region's samples (Sarasa Gothic SC, for Chinese), another region's
    // when it misses some (a Chinese font without kana or 込, for
    // Japanese).
    assert_eq!(step(first(false, false, true)), Step::Leave);
    assert_eq!(step(first(false, false, false)), Step::Before);
}

#[test]
fn each_regions_samples_hold_what_another_regions_legacy_set_lacks() {
    // Kana alone is in KS X 1001 and GB2312: Japanese also asks the
    // kokuji 込.
    assert!(Region::Japan.samples().contains(&'あ'));
    assert!(Region::Japan.samples().contains(&'込'));
    assert!(Region::SimplifiedChinese.samples().contains(&'这'));
    assert!(Region::TraditionalChinese.samples().contains(&'們'));
    assert!(Region::Korea.samples().contains(&'한'));
    // No region's samples are all another's.
    for a in Region::ALL {
        for b in Region::ALL.into_iter().filter(|b| *b != a) {
            assert!(
                a.samples().iter().any(|c| !b.samples().contains(c)),
                "{a:?} against {b:?}"
            );
        }
    }
}

#[test]
fn the_warning_names_the_font_and_its_packages_on_linux_only() {
    let linux = uncovered("ja", Region::Japan, true);
    assert!(
        linux.starts_with("no installed font covers ja (Japanese): "),
        "{linux}"
    );
    for words in [
        "Noto Sans CJK",
        "noto-fonts-cjk on Arch",
        "fonts-noto-cjk on Debian and Ubuntu",
        "google-noto-sans-cjk-fonts on Fedora",
    ] {
        assert!(linux.contains(words), "{linux}");
    }
    assert_eq!(
        uncovered("zh_CN", Region::SimplifiedChinese, false),
        "no installed font covers zh_CN (Simplified Chinese)"
    );
}

#[test]
fn with_no_font_covering_the_language_it_warns_and_changes_nothing() {
    // Families with no faces behind them: nothing covers anything.
    let mut c = bare();
    let (a, b) = (FamilyId::new(), FamilyId::new());
    set(&mut c, &[a], &[b]);
    for tag in ["ja", "zh_CN", "zh_TW", "ko"] {
        assert_eq!(
            arrange(&mut c, tag, &LINUX),
            Some(Outcome::Uncovered),
            "{tag}"
        );
    }
    assert_eq!(lists(&mut c), [vec![a], vec![b]]);
    assert_eq!(arrange(&mut bare(), "ja", &LINUX), Some(Outcome::Uncovered));
}

#[test]
fn the_generics_are_slints_own() {
    assert_eq!(GENERICS, i_slint_common::sharedfontique::FALLBACK_FAMILIES);
}

#[test]
fn coverage_counts_the_characters_a_face_maps() {
    let Some(Faces {
        mut c, latin, jp, ..
    }) = faces("coverage_counts_the_characters_a_face_maps")
    else {
        return;
    };
    let samples = Region::Japan.samples();
    assert_eq!(coverage(&mut c, jp, samples), samples.len());
    assert!(full(&mut c, jp, Region::Japan));
    assert!(draws(&mut c, jp, Region::Korea));
    assert_eq!(coverage(&mut c, latin, samples), 0);
    assert!(!draws(&mut c, latin, Region::Japan));
    assert_eq!(coverage(&mut c, latin, &['A', IDEOGRAPH]), 1);
    assert_eq!(coverage(&mut c, FamilyId::new(), &['A']), 0);
}

#[test]
fn another_regions_cut_first_gets_the_face_before_it() {
    let Some(Faces {
        mut c,
        latin,
        jp,
        kr,
        ..
    }) = faces("another_regions_cut_first_gets_the_face_before_it")
    else {
        return;
    };
    set(&mut c, &[latin, kr], &[latin]);
    assert_eq!(
        arrange(&mut c, "ja", &LINUX),
        Some(Outcome::Placed("Noto Sans CJK JP".into()))
    );
    assert_eq!(lists(&mut c), [vec![latin, jp, kr], vec![latin]]);
    // Korean the other way about.
    set(&mut c, &[latin, jp, kr], &[latin]);
    assert_eq!(
        arrange(&mut c, "ko", &LINUX),
        Some(Outcome::Placed("Noto Sans CJK KR".into()))
    );
    assert_eq!(lists(&mut c), [vec![latin, kr, jp], vec![latin]]);
}

#[test]
fn the_regions_own_cut_or_the_users_choice_is_left() {
    let Some(Faces {
        mut c,
        latin,
        jp,
        kr,
        own,
        ..
    }) = faces("the_regions_own_cut_or_the_users_choice_is_left")
    else {
        return;
    };
    set(&mut c, &[latin, jp, kr], &[latin]);
    assert_eq!(
        arrange(&mut c, "ja", &LINUX),
        Some(Outcome::Kept("Noto Sans CJK JP".into()))
    );
    assert_eq!(lists(&mut c), [vec![latin, jp, kr], vec![latin]]);
    set(&mut c, &[latin, own, kr], &[latin]);
    assert_eq!(
        arrange(&mut c, "ja", &LINUX),
        Some(Outcome::Kept("Own Gothic".into()))
    );
    assert_eq!(lists(&mut c), [vec![latin, own, kr], vec![latin]]);
}

#[test]
fn with_none_covering_the_face_goes_after_them_all() {
    let Some(Faces {
        mut c,
        latin,
        jp,
        kr,
        ..
    }) = faces("with_none_covering_the_face_goes_after_them_all")
    else {
        return;
    };
    set(&mut c, &[latin], &[latin]);
    assert_eq!(
        arrange(&mut c, "ja", &LINUX),
        Some(Outcome::Placed("Noto Sans CJK JP".into()))
    );
    assert_eq!(lists(&mut c), [vec![latin, jp], vec![latin]]);
    // With no UI list, into the sans-serif one, before another region's
    // cut there.
    set(&mut c, &[], &[latin, kr]);
    assert_eq!(
        arrange(&mut c, "ja", &LINUX),
        Some(Outcome::Placed("Noto Sans CJK JP".into()))
    );
    assert_eq!(lists(&mut c), [vec![], vec![latin, jp, kr]]);
}

#[test]
fn an_unknown_family_missing_some_samples_counts_as_another_regions() {
    // Noto Traditional Nushu maps 这 and no other ideograph: for
    // Simplified Chinese it draws something, and is no Chinese font.
    let Some(Faces {
        mut c, latin, sc, ..
    }) = faces("an_unknown_family_missing_some_samples_counts_as_another_regions")
    else {
        return;
    };
    let Some(nushu) = file(&mut system(), "Noto Traditional Nushu") else {
        eprintln!(
            "an_unknown_family_missing_some_samples_counts_as_another_regions: skipped, no Noto Traditional Nushu installed"
        );
        return;
    };
    c.register_fonts(nushu, None);
    let nushu = c.family_id("Noto Traditional Nushu").expect("registered");
    let region = Region::SimplifiedChinese;
    if !draws(&mut c, nushu, region) || full(&mut c, nushu, region) {
        eprintln!(
            "an_unknown_family_missing_some_samples_counts_as_another_regions: skipped, this Nushu font's coverage differs"
        );
        return;
    }
    set(&mut c, &[latin, nushu], &[latin]);
    assert_eq!(
        arrange(&mut c, "zh_CN", &LINUX),
        Some(Outcome::Placed("Noto Sans CJK SC".into()))
    );
    assert_eq!(lists(&mut c), [vec![latin, sc, nushu], vec![latin]]);
}

#[test]
fn a_table_name_finds_its_family_through_an_alias() {
    // Fontconfig gives each weight of Noto Sans CJK a second name, an
    // alias of the family's: a table that knows the cuts only by those
    // still knows them.
    let mut c = system();
    let (Some(jp), Some(kr)) = (
        c.family_id("Noto Sans CJK JP"),
        c.family_id("Noto Sans CJK KR"),
    ) else {
        eprintln!(
            "a_table_name_finds_its_family_through_an_alias: skipped, no Noto Sans CJK installed"
        );
        return;
    };
    if c.family_id("Noto Sans CJK JP Black") != Some(jp)
        || c.family_id("Noto Sans CJK KR Black") != Some(kr)
    {
        eprintln!(
            "a_table_name_finds_its_family_through_an_alias: skipped, no Black weights' aliases"
        );
        return;
    }
    let Some(latin) = files().and_then(|f| c.family_id(f.latin_name)) else {
        eprintln!("a_table_name_finds_its_family_through_an_alias: skipped, no Latin face");
        return;
    };
    let table = Table {
        japan: &["noto sans cjk jp black"],
        simplified_chinese: &[],
        traditional_chinese: &[],
        korea: &["Noto Sans CJK KR Black"],
    };
    set(&mut c, &[latin, kr], &[latin]);
    assert_eq!(
        arrange(&mut c, "ja", &table),
        Some(Outcome::Placed("Noto Sans CJK JP".into()))
    );
    assert!(lists(&mut c)[0].starts_with(&[latin, jp, kr]));
}

#[test]
fn without_the_regions_face_another_covering_is_logged_not_warned() {
    let Some(files) = files() else {
        eprintln!(
            "without_the_regions_face_another_covering_is_logged_not_warned: skipped, no Noto Sans CJK installed"
        );
        return;
    };
    // The CJK cuts under a name no table knows, and nothing else of them.
    let mut c = bare();
    c.register_fonts(files.latin.clone(), None);
    c.register_fonts(files.cjk[0].clone(), named("Own Gothic"));
    let latin = c.family_id(files.latin_name).expect("registered");
    let own = c.family_id("Own Gothic").expect("registered");
    set(&mut c, &[latin, own], &[latin]);
    assert_eq!(arrange(&mut c, "ja", &LINUX), Some(Outcome::Elsewhere));
    assert_eq!(lists(&mut c), [vec![latin, own], vec![latin]]);
    // Reached only through the fallback for the script.
    set(&mut c, &[latin], &[latin]);
    assert_eq!(arrange(&mut c, "ja", &LINUX), Some(Outcome::Uncovered));
    c.set_fallbacks(
        FallbackKey::new(Script::from_bytes(*b"Hani"), None),
        [own].into_iter(),
    );
    assert_eq!(arrange(&mut c, "ja", &LINUX), Some(Outcome::Elsewhere));
    assert_eq!(lists(&mut c), [vec![latin], vec![latin]]);
}

#[test]
fn a_first_choice_not_covering_the_language_is_passed_over() {
    let Some(files) = files() else {
        eprintln!(
            "a_first_choice_not_covering_the_language_is_passed_over: skipped, no Noto Sans CJK installed"
        );
        return;
    };
    // A Latin face going by the first choice's name, and the CJK cuts by
    // a later choice's.
    let mut c = bare();
    c.register_fonts(files.latin.clone(), None);
    c.register_fonts(files.latin.clone(), named("Noto Sans CJK JP"));
    c.register_fonts(files.cjk[0].clone(), named("Source Han Sans JP"));
    let latin = c.family_id(files.latin_name).expect("registered");
    let later = c.family_id("Source Han Sans JP").expect("registered");
    set(&mut c, &[latin], &[latin]);
    assert_eq!(
        arrange(&mut c, "ja", &LINUX),
        Some(Outcome::Placed("Source Han Sans JP".into()))
    );
    assert_eq!(lists(&mut c), [vec![latin, later], vec![latin]]);
}
