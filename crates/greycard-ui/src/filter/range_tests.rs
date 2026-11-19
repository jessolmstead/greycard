use super::*;

const ISOS: [f64; 6] = [100.0, 200.0, 400.0, 800.0, 3200.0, 102400.0];

fn ends(text: &str) -> (usize, usize, bool) {
    range_ends(text, NumberField::Iso, &ISOS)
}

/// No term is the whole range, off; each end alone moves its own.
#[test]
fn a_range_reads_its_ends_from_the_text() {
    assert_eq!(ends(""), (0, 5, false));
    assert_eq!(ends("harbor rating>=3"), (0, 5, false));
    assert_eq!(ends("iso>=400"), (2, 5, true));
    assert_eq!(ends("ISO<=800"), (0, 3, true));
    assert_eq!(ends("iso>=200 harbor iso<=3200"), (1, 4, true));
    // The tighter of two bounds on one end, in either order.
    assert_eq!(ends("iso>=200 iso>=800"), (3, 5, true));
    assert_eq!(ends("iso>=800 iso>=200"), (3, 5, true));
    assert_eq!(ends("iso<=800 iso<=200"), (0, 1, true));
    // Strict bounds pass the next value in.
    assert_eq!(ends("iso>400 iso<3200"), (3, 3, true));
    // An exact value bounds both ends.
    assert_eq!(ends("iso:800"), (3, 3, true));
    // A `!=` is no bound.
    assert_eq!(ends("iso!=800"), (0, 5, false));
    // The focal length's are the focal field's, and the reverse.
    assert_eq!(ends("focal>=50"), (0, 5, false));
}

/// A bound on a value the folder lacks puts its end on the next
/// value the term lets through; bounds no value passes, on the
/// value nearest the one asked.
#[test]
fn a_value_the_folder_lacks_snaps_to_one_it_has() {
    assert_eq!(ends("iso>=1000"), (4, 5, true));
    assert_eq!(ends("iso<=1000"), (0, 3, true));
    assert_eq!(ends("iso>=50 iso<=200000"), (0, 5, true));
    assert_eq!(ends("iso:1000"), (3, 3, true));
    assert_eq!(ends("iso>=200000"), (5, 5, true));
    assert_eq!(ends("iso<=50"), (0, 0, true));
    // A focal length through its tenth of a millimeter, as the
    // index compares it.
    let mm = [23.9, 35.0, 70.0];
    assert_eq!(
        range_ends("focal>=24", NumberField::Focal, &mm),
        (1, 2, true)
    );
    assert_eq!(
        range_ends("focal<=23.9 focal>=23.9", NumberField::Focal, &mm),
        (0, 0, true)
    );
}

/// Bounds that cross put both ends at the tightest low asked,
/// whatever their order in the text; with no low, the tightest
/// high.
#[test]
fn crossed_bounds_take_the_tightest() {
    assert_eq!(ends("iso>=200 iso>=3200 iso<=100"), (4, 4, true));
    assert_eq!(ends("iso<=100 iso>=3200 iso>=200"), (4, 4, true));
    assert_eq!(ends("iso<=800 iso<=50 iso<=3200"), (0, 0, true));
}

/// An older build's ISO and focal length chips, kept in the settings
/// or a smart collection, load as the range's terms in the text: one
/// value exactly, more from the lowest to the highest; a held-out
/// value is dropped (see `ranged_terms`). The other facets' chips load as chips.
#[test]
fn kept_iso_and_focal_chips_load_as_the_ranges_terms() {
    let kept: Saved = serde_json::from_str(
        r#"{"stars": 0, "exact": false, "flags": [], "labels": [], "text": "\"harbor",
                "facets": {"iso": ["3200", "200", "800"], "focal": ["35"], "camera": ["R6"]},
                "not_facets": {"iso": ["6400"], "mm": ["50"]}}"#,
    )
    .unwrap();
    let read = kept.filter();
    assert_eq!(read.text, "\"harbor\" iso>=200 iso<=3200 focal:35");
    assert!(read.chosen(Facet::Iso).is_empty() && read.held_out(Facet::Iso).is_empty());
    assert!(read.chosen(Facet::Focal).is_empty() && read.held_out(Facet::Focal).is_empty());
    assert_eq!(read.chosen(Facet::Camera), ["R6".to_string()]);
    let typed = read.typed();
    assert!(typed.errors.is_empty(), "{:?}", typed.errors);
    assert_eq!(ends(&read.text), (1, 4, true));
    assert_eq!(
        range_ends(&read.text, NumberField::Focal, &[24.0, 35.0, 50.0]),
        (1, 1, true)
    );
    // Kept again, the terms are in the text and no chip is.
    let again = Saved::of(&read);
    assert!(!again.facets.contains_key("iso") && !again.not_facets.contains_key("mm"));
    assert_eq!(again.filter(), read);
    // `--filter` at launch: the same terms go to the text, under
    // the field's own name.
    let (rest, wanted) = from_cli("camera:R6 iso:800 mm>=24 harbor");
    assert_eq!(rest, "iso:800 focal>=24 harbor");
    // A word that names the field and asks nothing stays a word.
    assert_eq!(from_cli("mm iso focal: harbor").0, "mm iso focal: harbor");
    assert_eq!(wanted.len(), 1);
    assert_eq!(wanted[0].facet, Facet::Camera);
}

/// The held-out values dropped are logged when the filter is opened,
/// once, and not each time it is read, as every rebuild reads every
/// smart collection's.
#[test]
fn held_out_values_are_logged_once_when_opened() {
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Lines(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Lines {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let kept: Saved = serde_json::from_str(
        r#"{"facets": {"iso": ["200"]}, "not_facets": {"iso": ["6400"], "focal": ["50"]}}"#,
    )
    .unwrap();
    let lines = Lines::default();
    let writer = lines.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..5 {
            assert_eq!(kept.filter().text, "iso:200");
        }
        assert_eq!(kept.opened(), kept.filter());
    });
    let said = String::from_utf8(lines.0.lock().unwrap().clone()).unwrap();
    assert_eq!(
        said.matches("held out by an older build").count(),
        2,
        "{said}"
    );
    assert!(said.contains("iso values held out by an older build (6400)"));
    assert!(said.contains("focal values held out by an older build (50)"));
}

/// A moved end writes its own term only, none at the folder's own
/// end, and leaves the other end's and every other byte as typed.
#[test]
fn a_moved_end_writes_its_term_and_leaves_the_rest() {
    let moved = |text, low_moved, low, high| with_range(text, Facet::Iso, low_moved, low, high);
    assert_eq!(moved("", true, "400", ""), "iso>=400");
    assert_eq!(moved("", false, "", "800"), "iso<=800");
    assert_eq!(
        moved("harbor  iso>=200 \"low tide\"", true, "400", ""),
        "harbor \"low tide\" iso>=400"
    );
    // A space typed at the end is the separator.
    assert_eq!(moved("harbor ", true, "400", ""), "harbor iso>=400");
    // Back to the folder's end: the term goes, with its gap.
    assert_eq!(moved("iso>=400   iso<=800 ", true, "", "800"), "iso<=800 ");
    // The other end as typed, a value the folder lacks included.
    assert_eq!(
        moved("iso<=1000\tiso!=200", true, "400", "800"),
        "iso<=1000\tiso!=200 iso>=400"
    );
    // An exact value bounds both, so both are written as shown.
    assert_eq!(moved("iso:800", true, "400", "800"), "iso>=400 iso<=800");
    assert_eq!(moved("iso=800", false, "800", ""), "iso>=800");
    // A focal length's own field.
    assert_eq!(
        with_range("focal>=24 iso>=400", Facet::Focal, true, "35", ""),
        "iso>=400 focal>=35"
    );
    // A facet with no range leaves the text alone.
    assert_eq!(with_range("a  b", Facet::Lens, true, "x", ""), "a  b");
}

/// A quote left open is closed before a term is written, so the
/// term is not taken into the phrase.
#[test]
fn a_term_is_written_outside_an_open_quote() {
    let text = with_range("\"harbor", Facet::Iso, true, "400", "");
    assert_eq!(text, "\"harbor\" iso>=400");
    assert_eq!(ends(&text), (2, 5, true));
    let text = with_range("\"low tide ", Facet::Iso, false, "", "800");
    assert_eq!(text, "\"low tide \" iso<=800");
    assert_eq!(ends(&text), (0, 3, true));
}

/// The whole range asked for takes every bound on the field out,
/// and nothing else.
#[test]
fn the_whole_range_takes_the_fields_bounds_out() {
    assert_eq!(
        without_range(
            "harbor iso>=400 iso<=800  iso:3200 iso!=100 focal>=24",
            Facet::Iso
        ),
        "harbor iso!=100 focal>=24"
    );
    assert_eq!(without_range(" iso>=400  harbor ", Facet::Iso), " harbor ");
    // With no bound to take out, the text as it was.
    assert_eq!(without_range("a  b ", Facet::Iso), "a  b ");
}

/// What a range writes, the filter reads back to the same ends.
#[test]
fn a_range_written_reads_back_where_it_was_set() {
    let text = with_range("", Facet::Iso, true, "200", "");
    let text = with_range(&text, Facet::Iso, false, "200", "3200");
    assert_eq!(ends(&text), (1, 4, true));
    let typed = Filter {
        text,
        ..Filter::default()
    }
    .typed();
    assert!(typed.errors.is_empty(), "{:?}", typed.errors);
    assert_eq!(typed.index.len(), 2);
}

/// Filters made up at random: the spans split as the parser does,
/// and a range's splice leaves every byte of the tokens it keeps
/// and of the gaps between them as typed, its terms at the end.
#[test]
fn a_splice_leaves_every_other_byte_as_typed() {
    const PIECES: [&str; 12] = [
        "harbor",
        "\"low tide\"",
        "iso>=400",
        "iso<=3200",
        "iso:800",
        "iso!=100",
        "focal>=24",
        "ISO>200",
        "iso<1600",
        "rating>=3",
        "é",
        "\"open",
    ];
    const GAPS: [&str; 5] = ["", " ", "  ", "\t", " \t "];
    let mut seed: u64 = 0x5eed;
    let mut next = |n: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize % n
    };
    for _ in 0..2000 {
        let mut text = GAPS[next(GAPS.len())].to_string();
        for i in 0..next(6) {
            if i > 0 {
                text.push_str(GAPS[1 + next(GAPS.len() - 1)]);
            }
            text.push_str(PIECES[next(PIECES.len())]);
        }
        text.push_str(GAPS[next(GAPS.len())]);
        let tokens: Vec<String> = spans(&text)
            .iter()
            .map(|(s, e)| text[*s..*e].to_string())
            .collect();
        assert_eq!(tokens, greycard_library::filter::tokens(&text), "{text:?}");

        let gone = |t: &str| bound_on(t, NumberField::Iso).is_some();
        let out = without_range(&text, Facet::Iso);
        kept_as_typed(&text, &out, &gone);

        let low_moved = next(2) == 0;
        let exact = tokens
            .iter()
            .any(|t| bound_on(t, NumberField::Iso).is_some_and(|(op, _)| op == Cmp::Eq));
        let gone = |t: &str| {
            bound_on(t, NumberField::Iso)
                .is_some_and(|(op, _)| exact || if low_moved { is_low(op) } else { is_high(op) })
        };
        let out = with_range(&text, Facet::Iso, low_moved, "400", "3200");
        let base = cut(&text, gone);
        kept_as_typed(&text, &base, &gone);
        assert!(out.starts_with(&base), "{text:?} -> {out:?}");
        let terms = match (low_moved || exact, !low_moved || exact) {
            (true, true) => "iso>=400 iso<=3200",
            (true, false) => "iso>=400",
            _ => "iso<=3200",
        };
        assert!(out.ends_with(terms), "{text:?} -> {out:?}");
        let read = greycard_library::filter::tokens(&out);
        let n = terms.split(' ').count();
        assert_eq!(
            read[read.len() - n..].join(" "),
            terms,
            "{text:?} -> {out:?}"
        );
    }
}

/// `out` is `text` with the tokens `gone` picks taken out: the
/// tokens left in order, and each gap between two tokens left
/// that were neighbors, the lead before a first token kept, and
/// the tail after a last one, byte for byte.
fn kept_as_typed(text: &str, out: &str, gone: &dyn Fn(&str) -> bool) {
    let before = spans(text);
    let after = spans(out);
    let kept: Vec<usize> = (0..before.len())
        .filter(|i| !gone(&text[before[*i].0..before[*i].1]))
        .collect();
    assert_eq!(kept.len(), after.len(), "{text:?} -> {out:?}");
    if kept.len() == before.len() {
        assert_eq!(out, text);
    }
    for (k, i) in kept.iter().enumerate() {
        let (s, e) = before[*i];
        assert_eq!(
            &out[after[k].0..after[k].1],
            &text[s..e],
            "{text:?} -> {out:?}"
        );
        if k > 0 && kept[k - 1] + 1 == *i {
            assert_eq!(
                &out[after[k - 1].1..after[k].0],
                &text[before[i - 1].1..s],
                "{text:?} -> {out:?}"
            );
        }
    }
    if kept.first() == Some(&0) {
        assert_eq!(
            &out[..after[0].0],
            &text[..before[0].0],
            "{text:?} -> {out:?}"
        );
    }
    if let (Some(&l), Some(a)) = (kept.last(), after.last())
        && l + 1 == before.len()
    {
        assert_eq!(&out[a.1..], &text[before[l].1..], "{text:?} -> {out:?}");
    }
}
