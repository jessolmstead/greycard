use super::*;
use std::path::PathBuf;

/// The names marked for the catalog are greycard-edit's own.
#[test]
fn the_marked_names_are_the_flags_and_the_labels() {
    let names: Vec<&str> = Flag::ALL
        .iter()
        .map(|f| f.name())
        .chain(Label::ALL.iter().map(|l| l.name()))
        .collect();
    assert_eq!(names, _MARK_NAMES);
    let lower: Vec<String> = _MARK_NAMES.iter().map(|n| n.to_lowercase()).collect();
    assert_eq!(lower, _MARK_LOWER_NAMES);
}

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
/// the text field's, ISO and the focal length among it, which have
/// ranges and no chips.
#[test]
fn the_command_line_filter_splits_into_chips_and_text() {
    let want = |facet, value: &str, exact| Wanted {
        facet,
        value: value.to_string(),
        exact,
    };
    let (text, wanted) = from_cli("camera:R6 iso>=3200 rating>=3 focal=50mm harbor");
    assert_eq!(text, "iso>=3200 rating>=3 focal=50mm harbor");
    assert_eq!(wanted, vec![want(Facet::Camera, "R6", false)]);
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
    // A kind as the term reads it, wanted exactly; `!=` and a
    // kind there is not are the field's, which refuses the second.
    let (text, wanted) = from_cli("kind:raws kind=JPG kind!=raw kind:gif");
    assert_eq!(text, "kind!=raw kind:gif");
    assert_eq!(
        wanted,
        vec![
            want(Facet::Kind, "raw", true),
            want(Facet::Kind, "jpeg", true)
        ]
    );
    let field = Filter {
        text,
        ..Filter::default()
    };
    let typed = field.typed();
    assert_eq!(typed.meta.len(), 1);
    assert_eq!(typed.errors.len(), 1);
    assert!(
        typed.errors[0]
            .to_string()
            .contains("raw, jpeg, png or tiff")
    );
}

/// The kind is answered from the path, which every frame has,
/// whether typed or a chip: a frame the index has no row for is
/// held to it all the same, and nothing is asked of the index.
/// Its chips read as a word or a format's capitals.
#[test]
fn a_kind_is_answered_from_the_path_typed_or_chipped() {
    let folder: Vec<(PathBuf, Meta)> = ["a.CR3", "b.jpg", "c.NEF", "d.tif", "e.JPEG"]
        .iter()
        .map(|n| (PathBuf::from("/shoot").join(n), Meta::default()))
        .collect();
    let typed = |text: &str| Filter {
        text: text.into(),
        ..Filter::default()
    };
    let raws = typed("kind:raw");
    assert_eq!(raws.typed().meta.len(), 1);
    assert!(!raws.asks_index(&raws.typed()), "the path is in hand");
    assert_eq!(raws.apply(&view(&folder)), vec![0, 2]);
    assert_eq!(typed("kind!=raw").apply(&view(&folder)), vec![1, 3, 4]);
    assert_eq!(typed("kind:jpg").apply(&view(&folder)), vec![1, 4]);

    let mut chips = Filter::default();
    chips.toggle_facet(Facet::Kind, "raw");
    assert!(!chips.asks_index(&chips.typed()), "the path is in hand");
    assert!(chips.index_filter(&chips.typed(), None).is_empty());
    // Every frame passes the index here, as one with no row does.
    assert_eq!(chips.apply(&view(&folder)), vec![0, 2]);
    chips.toggle_facet(Facet::Kind, "jpeg");
    assert_eq!(chips.apply(&view(&folder)), vec![0, 1, 2, 4]);
    // Its own row's count sets it aside; the others' keep it.
    let frames = view(&folder);
    assert!(chips.shows_in_hand(frames[3], Some(Facet::Kind)));
    assert!(!chips.shows_in_hand(frames[3], Some(Facet::Keyword)));
    assert_eq!(chips.describe(), "kind Raw or JPEG");
    assert_eq!(facet_caption(Facet::Kind), "Kind");
    let said: Vec<String> = greycard_library::filter::kinds()
        .into_iter()
        .map(|v| facet_chip_text(Facet::Kind, v))
        .collect();
    assert_eq!(said, ["Raw", "JPEG", "PNG", "TIFF"]);

    // `--filter kind:` needs no frame to name its chip: it is on
    // from the start, and the rest wait.
    let (_, wanted) = from_cli("kind:jpg camera:R6");
    let mut filter = Filter::default();
    let rest = filter.choose_at_once(wanted);
    assert_eq!(filter.chosen(Facet::Kind), ["jpeg".to_string()]);
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].facet, Facet::Camera);
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

/// A Ctrl+click holds a chip out: "everything but the rejects" is
/// one chip, and a held-out chip beside ticked ones narrows them.
/// Any press on a held-out chip lets it back to neither.
#[test]
fn a_chip_held_out_hides_what_it_names() {
    let folder = folder();
    let frames = view(&folder);
    let mut filter = Filter::default();
    filter.exclude_flag(Flag::Reject);
    assert_eq!(filter.not_flags, [Flag::Reject]);
    assert_eq!(filter.apply(&frames), vec![0, 1, 2, 4]);
    assert_eq!(filter.describe(), "not reject");
    // The row still reads the shoot: a count sets its own group
    // aside, held-out chips with the rest.
    let counts = Counts::of(&filter, &frames);
    assert_eq!(counts.flags, [2, 2, 1]);
    assert_eq!(counts.shown, 4);
    assert_eq!(
        counts.flags[2],
        filter.only_flag(Flag::Reject).apply(&frames).len()
    );
    // A plain click on it clears it; so does a second Ctrl+click.
    filter.toggle_flag(Flag::Reject);
    assert!(filter.is_empty());
    filter.exclude_flag(Flag::Reject);
    filter.exclude_flag(Flag::Reject);
    assert!(filter.is_empty());
    // A Ctrl+click on a ticked chip moves it across.
    filter.toggle_label(Label::Red);
    filter.exclude_label(Label::Red);
    assert!(filter.labels.is_empty());
    assert_eq!(filter.apply(&frames), vec![0, 2, 3]);
    // Ticked and held out in one group: Red or None, but not
    // what else is held out.
    filter.toggle_label(Label::Red);
    filter.toggle_label(Label::None);
    filter.exclude_label(Label::Red);
    assert_eq!(filter.labels, [Label::None]);
    assert_eq!(filter.not_labels, [Label::Red]);
    assert_eq!(filter.apply(&frames), vec![0, 3]);
    assert_eq!(filter.describe(), "no label, not red");
    filter.exclude_label(Label::None);
    assert_eq!(filter.describe(), "labeled, not red");
    filter.exclude_label(Label::None);

    // A keyword held out is answered in hand, case folded.
    let mut words = Filter::default();
    words.exclude_facet(Facet::Keyword, "sunset");
    assert!(!words.asks_index(&words.typed()));
    assert_eq!(words.apply(&frames), vec![0, 1, 4]);
    assert_eq!(words.describe(), "not keyword sunset");
    // An ISO held out goes to the index, as none of its values,
    // and its own count sets it aside.
    let mut iso = Filter::default();
    iso.exclude_facet(Facet::Iso, "6400");
    let typed = iso.typed();
    assert!(iso.asks_index(&typed));
    assert_eq!(
        iso.index_filter(&typed, None).terms,
        [Term::NotFacet {
            facet: Facet::Iso,
            values: vec!["6400".into()],
        }]
    );
    assert!(iso.index_filter(&typed, Some(Facet::Iso)).is_empty());
    assert_eq!(iso.describe(), "not ISO 6400");
    iso.toggle_facet(Facet::Iso, "6400");
    assert!(iso.is_empty(), "a plain click clears it");
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

/// The filter kept between sessions comes back as it was left:
/// every chip, the rows' reading, the text and the facets, by
/// name. A facet a later build dropped is left out, not a panic.
#[test]
fn the_filter_kept_comes_back_as_it_was_left() {
    let mut filter = Filter {
        stars: Stars::Exactly(3),
        flags: vec![Flag::Pick, Flag::None],
        labels: vec![Label::Red],
        text: "harbor iso>=3200".into(),
        ..Filter::default()
    };
    filter.toggle_facet(Facet::Camera, "Canon EOS R6m2");
    filter.toggle_facet(Facet::Lens, "RF 35mm");
    filter.toggle_facet(Facet::Keyword, "dusk");
    filter.toggle_facet(Facet::Kind, "raw");
    filter.exclude_flag(Flag::Reject);
    filter.exclude_label(Label::Purple);
    filter.exclude_facet(Facet::Style, "Canon Faithful");
    let saved = Saved::of(&filter);
    let text = serde_json::to_string(&saved).unwrap();
    let back: Saved = serde_json::from_str(&text).unwrap();
    assert_eq!(back.filter(), filter);
    // None held out writes the shape it always did.
    let plain = serde_json::to_string(&Saved::of(&Filter::default())).unwrap();
    assert!(!plain.contains("not_"), "{plain}");
    // Cleared, it is kept empty, and comes back empty.
    assert_eq!(Saved::of(&Filter::default()).filter(), Filter::default());
    assert!(Saved::of(&Filter::default()).facets.is_empty());
    // A facet this build does not know, and stars past five.
    let odd: Saved =
        serde_json::from_str(r#"{"stars": 9, "facets": {"colour": ["red"], "lens": ["RF 50"]}}"#)
            .unwrap();
    let read = odd.filter();
    assert_eq!(read.stars, Stars::AtLeast(STARS));
    assert_eq!(read.chosen(Facet::Lens), ["RF 50".to_string()]);
    assert_eq!(read.facets.iter().filter(|f| !f.is_empty()).count(), 1);
    // One kept by a build before the kind, whose facets sat in
    // other places on the bar: each comes back to its own.
    let before: Saved = serde_json::from_str(
        r#"{"stars": 2, "exact": false, "flags": [], "labels": [],
                "text": "harbor", "facets": {"camera": ["Canon EOS R5"],
                "style": ["Canon Faithful"], "keyword": ["dusk"]}}"#,
    )
    .unwrap();
    let read = before.filter();
    assert_eq!(read.chosen(Facet::Camera), ["Canon EOS R5".to_string()]);
    assert_eq!(read.chosen(Facet::Style), ["Canon Faithful".to_string()]);
    assert_eq!(read.chosen(Facet::Keyword), ["dusk".to_string()]);
    assert!(read.chosen(Facet::Kind).is_empty());
    assert_eq!(Saved::of(&read), before);
}
