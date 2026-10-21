//! Numbers typed into the panel's fields: a slider's value and the
//! point curve's In and Out. The shown text is formatted where the
//! control is, in Slint, in the unit the user thinks in ("+0.40 EV",
//! "35%", "1.5"); a field starts on that text's number alone and
//! reads what is typed back through the control's scale, so the two
//! directions go through the one reading here.

use slint::ComponentHandle;

use crate::{App, TypedNumber, Typing};

/// The number in `text` and where it ends: an optional sign (a +, a
/// -, or the typographic minus), digits with at most one decimal
/// point (a comma counts as one), after any leading space and an "x"
/// or "×" a multiplier may be written with. None when there is no
/// digit where the number should start.
fn number(text: &str) -> Option<(f32, &str)> {
    let rest = text
        .trim_start()
        .trim_start_matches(['x', 'X', '\u{d7}'])
        .trim_start();
    let (negative, rest) = match rest.chars().next() {
        Some('-' | '\u{2212}') => (true, &rest[rest.chars().next()?.len_utf8()..]),
        Some('+') => (false, &rest[1..]),
        _ => (false, rest),
    };
    let mut end = 0;
    let mut point = false;
    let mut digits = false;
    for (i, c) in rest.char_indices() {
        match c {
            '0'..='9' => digits = true,
            '.' | ',' if !point => point = true,
            _ => break,
        }
        end = i + c.len_utf8();
    }
    if !digits {
        return None;
    }
    let value: f32 = rest[..end].replace(',', ".").parse().ok()?;
    Some((if negative { -value } else { value }, &rest[end..]))
}

/// The shown text as a field starts it: the number alone, without
/// the + sign or the unit. "+0.40 EV" edits as "0.40", "-12%" as
/// "-12", "1.50x" as "1.50"; a text with no number starts empty.
pub(crate) fn editable(shown: &str) -> String {
    // Past a word before the number ("auto 0.75"), to its first digit
    // or the sign in front of it.
    let Some(start) = shown.find(|c: char| c.is_ascii_digit()) else {
        return String::new();
    };
    let head = &shown[..start];
    let start = match head.chars().last() {
        Some('-' | '\u{2212}') => start - head.chars().last().map_or(0, char::len_utf8),
        Some('.') => start - 1,
        _ => start,
    };
    let from = &shown[start..];
    let end = from
        .char_indices()
        .find(|&(_, c)| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '\u{2212}'))
        .map_or(from.len(), |(i, _)| i);
    from[..end].replace('\u{2212}', "-")
}

/// What a typed `text` sets: its number over `scale`, clamped to
/// `minimum..=maximum`. None when it holds no number, carries
/// anything but a unit after it (a second number, say), or the scale
/// cannot be divided by.
pub(crate) fn parse(text: &str, scale: f32, minimum: f32, maximum: f32) -> Option<f32> {
    let (shown, rest) = number(text)?;
    // A unit may follow ("EV", "%", "°", "x"); another number may not.
    if rest.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if !scale.is_finite() || scale == 0.0 {
        return None;
    }
    let value = shown / scale;
    if !value.is_finite() {
        return None;
    }
    let (lo, hi) = (minimum.min(maximum), minimum.max(maximum));
    Some(value.clamp(lo, hi))
}

/// Answer the window's `Typing` global with the two above.
pub(crate) fn install(app: &App) {
    let entry = app.global::<Typing>();
    entry.on_editable(|shown| editable(shown.as_str()).into());
    entry.on_parse(|text, scale, minimum, maximum| {
        match parse(text.as_str(), scale, minimum, maximum) {
            Some(value) => TypedNumber { ok: true, value },
            None => TypedNumber {
                ok: false,
                value: 0.0,
            },
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_starts_on_the_number_alone() {
        assert_eq!(editable("+0.4 EV"), "0.4");
        assert_eq!(editable("-0.25 EV"), "-0.25");
        assert_eq!(editable("+12"), "12");
        assert_eq!(editable("-12%"), "-12");
        assert_eq!(editable("35%"), "35");
        assert_eq!(editable("1.5"), "1.5");
        assert_eq!(editable("1.50x"), "1.50");
        assert_eq!(editable("5600 K"), "5600");
        assert_eq!(editable("+1.5\u{2030}"), "1.5");
        assert_eq!(editable("123.4\u{b0}"), "123.4");
        assert_eq!(editable("auto 0.75"), "0.75");
        assert_eq!(editable("auto"), "");
        assert_eq!(editable(""), "");
    }

    #[test]
    fn a_typed_number_is_read_in_the_shown_unit_and_clamped() {
        // Exposure: shown as it is, in stops.
        assert_eq!(parse("0.5", 1.0, -5.0, 5.0), Some(0.5));
        assert_eq!(parse("+0.5 EV", 1.0, -5.0, 5.0), Some(0.5));
        assert_eq!(parse("-1,25", 1.0, -5.0, 5.0), Some(-1.25));
        assert_eq!(parse("\u{2212}2", 1.0, -5.0, 5.0), Some(-2.0));
        // Outside the range: clamped, not refused.
        assert_eq!(parse("9", 1.0, -5.0, 5.0), Some(5.0));
        assert_eq!(parse("-9", 1.0, -5.0, 5.0), Some(-5.0));
        // A percent: 40 is 0.4 of a 0 to 1 value.
        assert_eq!(parse("40", 100.0, 0.0, 1.0), Some(0.4));
        assert_eq!(parse("40%", 100.0, 0.0, 1.0), Some(0.4));
        assert_eq!(parse("-150", 100.0, -1.0, 1.0), Some(-1.0));
        // A multiplier, as a log slider shows it, with or without its x.
        assert_eq!(parse("1.5", 1.0, 0.5, 2.0), Some(1.5));
        assert_eq!(parse("x1.5", 1.0, 0.5, 2.0), Some(1.5));
        assert_eq!(parse("1.5x", 1.0, 0.5, 2.0), Some(1.5));
        assert_eq!(parse("0.1", 1.0, 0.5, 2.0), Some(0.5));
        // A leading point.
        assert_eq!(parse(".5", 1.0, -5.0, 5.0), Some(0.5));
    }

    #[test]
    fn what_is_not_a_number_is_refused() {
        assert_eq!(parse("abc", 1.0, -5.0, 5.0), None);
        assert_eq!(parse("", 1.0, -5.0, 5.0), None);
        assert_eq!(parse("  ", 1.0, -5.0, 5.0), None);
        assert_eq!(parse("+", 1.0, -5.0, 5.0), None);
        assert_eq!(parse(".", 1.0, -5.0, 5.0), None);
        assert_eq!(parse("ev 2", 1.0, -5.0, 5.0), None);
        assert_eq!(parse("1.2.3", 1.0, -5.0, 5.0), None);
        assert_eq!(parse("1 2", 1.0, -5.0, 5.0), None);
        // A scale of nothing: a black-and-white weight at a strength
        // of 0 reads 0 whatever the weight, and cannot be typed.
        assert_eq!(parse("1", 0.0, -1.0, 1.0), None);
    }

    /// Every slider says how its reading scales its value, and says
    /// it truly: where the text rounds the slider's own value times a
    /// factor, the slider's `scale` is that factor (1 when it has
    /// none). A slider whose text is someone else's reading must
    /// carry a `scale` of its own.
    #[test]
    fn every_slider_scale_matches_its_text() {
        let dir = std::path::Path::new(greycard_slint::PANEL_DIR);
        let (mut checked, mut indirect) = (0, 0);
        for file in std::fs::read_dir(dir).expect("the panel's sources") {
            let path = file.expect("a panel source").path();
            if path.extension().is_none_or(|e| e != "slint") {
                continue;
            }
            // A Windows checkout ends its lines in CRLF.
            let source = std::fs::read_to_string(&path)
                .expect("a panel source reads")
                .replace("\r\n", "\n");
            for (at, _) in source.match_indices("EditSlider {") {
                let body = block(&source[at + "EditSlider {".len()..]);
                let value = field(body, "value <=> root.").expect("a slider's value");
                let text = field(body, "text: ").expect("a slider's text");
                let scale = field(body, "scale: ");
                let own = format!("round(root.{value}");
                let factor = text.find(&own).map(|i| {
                    let after = &text[i + own.len()..];
                    match after.strip_prefix(" * ") {
                        None => 1.0,
                        Some(after) => {
                            let (times, after) = leading_number(after);
                            match after.strip_prefix(") / ") {
                                Some(over) => times / leading_number(over).0,
                                None => times,
                            }
                        }
                    }
                });
                let name = path.file_name().unwrap().to_string_lossy();
                match factor {
                    Some(f) => match scale {
                        Some(s) => assert_eq!(
                            s.parse::<f32>().ok(),
                            Some(f),
                            "{name}: {value}'s scale {s} against its text {text}"
                        ),
                        None => assert_eq!(f, 1.0, "{name}: {value} needs a scale of {f}"),
                    },
                    None => {
                        // A reading made from another property: listed
                        // here, text and scale both, so a change to
                        // either fails until it is looked at.
                        let (_, want_text, want_scale) = INDIRECT
                            .iter()
                            .find(|(v, _, _)| *v == value)
                            .unwrap_or_else(|| {
                                panic!("{name}: {value}'s text {text} is not its own; list it")
                            });
                        assert!(
                            text.contains(want_text),
                            "{name}: {value}'s text {text} no longer reads {want_text}"
                        );
                        assert_eq!(scale, *want_scale, "{name}: {value}'s scale");
                        assert!(
                            value != "bw-weight"
                                || source.contains(
                                    "property <float> bw-applied: root.bw-weight * root.bw-strength;"
                                ),
                            "the weight's reading is no longer weight times strength"
                        );
                        indirect += 1;
                    }
                }
                checked += 1;
            }
        }
        assert!(checked > 60, "only {checked} sliders found");
        assert_eq!(indirect, INDIRECT.len(), "a listed slider is gone");
    }

    /// The sliders whose reading goes through another property: the
    /// value, the property the text reads, the slider's scale. The
    /// weight reads the grey's move, weight times strength, so it goes
    /// back over the strength; the strength reads its own value in
    /// hundredths, and the tint's hue its own value from Rust, one to
    /// one.
    const INDIRECT: [(&str, &str, Option<&str>); 3] = [
        (
            "bw-weight",
            "round(root.bw-applied * 100) / 100",
            Some("root.bw-strength"),
        ),
        ("bw-strength", "root.bw-strength-cents", None),
        ("tint-hue", "root.tint-hue-text", None),
    ];

    /// The exposure's value typed in the window: a click on the
    /// reading, the number, Enter. It sets the value, fires the
    /// slider's change once, and the save that follows records one
    /// step. Escape and a text that is no number leave the value as
    /// it was and fire nothing; a number past the range is clamped.
    /// Enter and Escape both hand the keys back to the window.
    #[test]
    fn a_number_typed_into_a_slider_sets_it_as_a_drag_would() {
        use crate::testing;
        use slint::platform::Key;
        use std::cell::Cell;
        use std::rc::Rc;

        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        assert_eq!(state.borrow().current, Some(0));

        // The window's own handling of a change, counted.
        let changes = Rc::new(Cell::new(0));
        {
            let (changes, state, app_weak) = (changes.clone(), state.clone(), app.as_weak());
            app.on_view_changed(move || {
                changes.set(changes.get() + 1);
                crate::panel::edit::schedule_save(&mut state.borrow_mut(), app_weak.clone());
            });
        }
        let settle = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
                crate::panel::edit::SAVE_MS + 100,
            ));
            slint::platform::update_timers_and_animations();
        };
        settle();
        let states = || state.borrow().sidecars[0].states();
        let before = states();
        let done = || app.global::<Typing>().get_done();
        let open_field = || {
            let (at, size) = testing::labeled(&app, "Exposure value");
            testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        };

        open_field();
        testing::type_text(&app, "0.5");
        testing::press(&app, Key::Return);
        assert_eq!(app.get_exposure(), 0.5);
        assert_eq!(changes.get(), 1, "one change");
        assert_eq!(done(), 1, "the keys handed back");
        settle();
        assert_eq!(states(), before + 1, "one history step");
        assert_eq!(
            state.borrow().sidecars[0].current.look().light.exposure,
            0.5
        );
        // The window's keys work again: Tab puts the panels away and
        // brings them back.
        assert!(!app.get_left_hidden());
        testing::press(&app, Key::Tab);
        assert!(app.get_left_hidden(), "Tab reached the window");
        testing::press(&app, Key::Tab);
        assert!(!app.get_left_hidden());

        // Escape: nothing typed counts.
        open_field();
        testing::type_text(&app, "3");
        testing::press(&app, Key::Escape);
        assert_eq!(app.get_exposure(), 0.5);
        assert_eq!(changes.get(), 1);
        assert_eq!(done(), 2);

        // Not a number: the same.
        open_field();
        testing::type_text(&app, "abc");
        testing::press(&app, Key::Return);
        assert_eq!(app.get_exposure(), 0.5);
        assert_eq!(changes.get(), 1);
        settle();
        assert_eq!(states(), before + 1);

        // Past the range: the end of it.
        open_field();
        testing::type_text(&app, "9");
        testing::press(&app, Key::Return);
        assert_eq!(app.get_exposure(), 5.0);
        assert_eq!(changes.get(), 2);
        settle();
        assert_eq!(states(), before + 2);

        // Enter on the reading untouched: the reading is rounded, and
        // reading it back would move the value. Nothing happens.
        app.set_exposure(0.4437);
        open_field();
        testing::press(&app, Key::Return);
        assert_eq!(app.get_exposure(), 0.4437);
        assert_eq!(changes.get(), 2, "no change for the reading as it was");
        // Typed over with the reading's own number, the same: it is
        // the text that is compared.
        open_field();
        testing::type_text(&app, "0.44");
        testing::press(&app, Key::Return);
        assert_eq!(app.get_exposure(), 0.4437);
        assert_eq!(changes.get(), 2);
        // Any other number is set.
        open_field();
        testing::type_text(&app, "0.45");
        testing::press(&app, Key::Return);
        assert_eq!(app.get_exposure(), 0.45);
        assert_eq!(changes.get(), 3);
    }

    /// A field open when its section goes, with a tab, a click on a
    /// tab: the field closes and the window's keys work at once.
    #[test]
    fn a_field_taken_away_while_open_hands_the_keys_back() {
        use crate::testing;
        use std::cell::RefCell;
        use std::rc::Rc;

        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        let keys = Rc::new(RefCell::new(Vec::<String>::new()));
        {
            let keys = keys.clone();
            app.on_meta_key(move |k| {
                keys.borrow_mut().push(k.to_string());
                true
            });
        }

        // A tab switched by a click on it.
        let (at, size) = testing::labeled(&app, "Exposure value");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        testing::type_text(&app, "1");
        assert!(keys.borrow().is_empty(), "typed into the field");
        let (at, size) = testing::labeled(&app, "Crop");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(app.get_panel_tab(), "Crop");
        testing::press(&app, "3");
        assert_eq!(*keys.borrow(), ["3"], "the rating key reached the window");
        assert_eq!(app.get_exposure(), 0.0, "nothing typed counted");

        // A tab switched from elsewhere, the field's section gone
        // under it with nothing clicked.
        app.set_panel_tab("Develop".into());
        let (at, size) = testing::labeled(&app, "Exposure value");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        testing::type_text(&app, "1");
        assert_eq!(keys.borrow().len(), 1);
        app.set_panel_tab("Retouch".into());
        testing::press(&app, "4");
        assert_eq!(*keys.borrow(), ["3", "4"]);
        assert_eq!(app.get_exposure(), 0.0);
    }

    /// Every slider names its line of the one table of steps, and
    /// the table has every line some slider names.
    #[test]
    fn every_slider_takes_its_steps_from_the_table() {
        let dir = std::path::Path::new(greycard_slint::PANEL_DIR);
        let controls = std::fs::read_to_string(dir.parent().unwrap().join("controls.slint"))
            .expect("the controls' source")
            .replace("\r\n", "\n");
        let at = controls.find("export global Steps {").expect("the table");
        let table = block(&controls[at + "export global Steps {".len()..]);
        let lines: Vec<&str> = table
            .lines()
            .filter_map(|l| l.trim().strip_prefix("out property <Nudge> "))
            .map(|l| l.split(':').next().unwrap())
            .collect();
        let mut named = std::collections::BTreeSet::new();
        let mut checked = 0;
        for file in std::fs::read_dir(dir).expect("the panel's sources") {
            let path = file.expect("a panel source").path();
            if path.extension().is_none_or(|e| e != "slint") {
                continue;
            }
            let source = std::fs::read_to_string(&path)
                .expect("a panel source reads")
                .replace("\r\n", "\n");
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            for (at, _) in source.match_indices("EditSlider {") {
                let body = block(&source[at + "EditSlider {".len()..]);
                let steps = field(body, "steps: Steps.")
                    .unwrap_or_else(|| panic!("{name}: a slider without its steps"));
                assert!(lines.contains(&steps), "{name}: no line {steps}");
                named.insert(steps.to_string());
                checked += 1;
            }
        }
        assert!(checked > 60, "only {checked} sliders found");
        let idle: Vec<_> = lines.iter().filter(|l| !named.contains(**l)).collect();
        assert!(idle.is_empty(), "no slider takes {idle:?}");
    }

    /// The window, a frame open on the Develop tab, and the exposure
    /// slider focused by a press on its label, which moves nothing.
    fn exposure_focused() -> (
        App,
        std::rc::Rc<std::cell::RefCell<crate::State>>,
        std::rc::Rc<crate::worker::Worker>,
    ) {
        use crate::testing;
        let app = testing::window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = testing::state_for(&app, testing::folder(3));
        app.invoke_select(0);
        focus_slider(&app, "Exposure");
        assert_eq!(app.get_exposure(), 0.0, "the press moved nothing");
        (app, state, worker)
    }

    /// A press on a slider's label: focus, no jump.
    fn focus_slider(app: &App, label: &str) {
        let (at, size) = crate::testing::labeled(app, label);
        crate::testing::click(app, at.x + 8.0, at.y + size.height / 2.0);
    }

    fn shifted(app: &App, key: impl Into<slint::SharedString>) {
        use slint::platform::{Key, WindowEvent};
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
        crate::testing::press(app, key);
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Up and plus nudge the exposure by a tenth of a stop, Shift by a
    /// third; Down, minus and the other arrows the same ways. Each is
    /// one change of the slider, as a step of a drag is.
    #[test]
    fn a_focused_slider_is_nudged_by_its_steps() {
        use crate::testing::press;
        use slint::platform::Key;
        let (app, _state, _worker) = exposure_focused();
        let changes = std::rc::Rc::new(std::cell::Cell::new(0));
        {
            let changes = changes.clone();
            app.on_view_changed(move || changes.set(changes.get() + 1));
        }
        press(&app, Key::UpArrow);
        assert!(near(app.get_exposure(), 0.1), "{}", app.get_exposure());
        press(&app, "+");
        assert!(near(app.get_exposure(), 0.2), "{}", app.get_exposure());
        shifted(&app, Key::UpArrow);
        assert!(
            near(app.get_exposure(), 0.2 + 1.0 / 3.0),
            "{}",
            app.get_exposure()
        );
        assert_eq!(changes.get(), 3);
        // Shift is the arrows' alone: with plus and minus, the small
        // step, since on some boards Shift makes them other keys.
        shifted(&app, "+");
        assert!(
            near(app.get_exposure(), 0.3 + 1.0 / 3.0),
            "{}",
            app.get_exposure()
        );
        shifted(&app, "_");
        assert!(
            near(app.get_exposure(), 0.2 + 1.0 / 3.0),
            "{}",
            app.get_exposure()
        );
        assert_eq!(changes.get(), 5);
        shifted(&app, Key::DownArrow);
        press(&app, "-");
        press(&app, "=");
        press(&app, Key::LeftArrow);
        assert!(near(app.get_exposure(), 0.1), "{}", app.get_exposure());
        press(&app, Key::RightArrow);
        assert!(near(app.get_exposure(), 0.2), "{}", app.get_exposure());
        assert_eq!(changes.get(), 10);
        // Held at its end, it stays there.
        app.set_exposure(4.95);
        shifted(&app, Key::UpArrow);
        assert_eq!(app.get_exposure(), 5.0);
    }

    /// Period focuses the next slider down, comma the one above; the
    /// arrows then move the one focused and no other.
    #[test]
    fn comma_and_period_move_the_focus_between_sliders() {
        use crate::testing::press;
        use slint::platform::Key;
        let (app, _state, _worker) = exposure_focused();
        press(&app, ".");
        press(&app, Key::UpArrow);
        assert_eq!(app.get_exposure(), 0.0, "the exposure let go");
        assert!(
            near(app.get_contrast(), 2f32.powf(0.02)),
            "the contrast took the keys: {}",
            app.get_contrast()
        );
        press(&app, ".");
        press(&app, Key::UpArrow);
        assert!(near(app.get_highlights(), 0.05), "{}", app.get_highlights());
        press(&app, ",");
        press(&app, ",");
        press(&app, Key::UpArrow);
        assert!(near(app.get_exposure(), 0.1), "back on the exposure");
        assert!(near(app.get_highlights(), 0.05));
        assert!(near(app.get_contrast(), 2f32.powf(0.02)));
    }

    /// In a slider's typed value, Up and Down nudge it and Tab sets
    /// what was typed and opens the next slider's, Shift+Tab the one
    /// above. The last Enter hands the keys back.
    #[test]
    fn tab_goes_from_one_typed_value_to_the_next() {
        use crate::testing;
        use slint::platform::Key;
        let (app, _state, _worker) = exposure_focused();
        let (at, size) = testing::labeled(&app, "Exposure value");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        testing::type_text(&app, "1");
        testing::press(&app, Key::Tab);
        assert_eq!(app.get_exposure(), 1.0, "Tab set the typed value");
        assert!(!app.get_left_hidden(), "Tab never reached the window");
        // The contrast's value is open now, all selected.
        testing::type_text(&app, "1.5");
        testing::press(&app, Key::UpArrow);
        assert!(
            near(app.get_contrast(), 1.5 * 2f32.powf(0.02)),
            "typed, then nudged: {}",
            app.get_contrast()
        );
        shifted(&app, Key::Tab);
        // Back on the exposure's value.
        testing::press(&app, Key::DownArrow);
        assert!(near(app.get_exposure(), 0.9), "{}", app.get_exposure());
        testing::type_text(&app, "0.3");
        testing::press(&app, Key::Return);
        assert!(near(app.get_exposure(), 0.3));
        assert!(near(app.get_contrast(), 1.5 * 2f32.powf(0.02)));
        // The window's keys again.
        testing::press(&app, Key::Tab);
        assert!(app.get_left_hidden(), "Tab reached the window");
    }

    /// A nudge is saved as a drag is: once the panel has rested, one
    /// history step. A run of nudges inside the rest is one step too,
    /// as a drag's many changes are.
    #[test]
    fn a_nudge_is_one_history_step() {
        use crate::testing;
        use slint::platform::Key;
        let app = testing::window(1);
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        {
            let (state, app_weak) = (state.clone(), app.as_weak());
            app.on_view_changed(move || {
                crate::panel::edit::schedule_save(&mut state.borrow_mut(), app_weak.clone());
            });
        }
        let settle = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
                crate::panel::edit::SAVE_MS + 100,
            ));
            slint::platform::update_timers_and_animations();
        };
        settle();
        let states = || state.borrow().sidecars[0].states();
        let before = states();
        focus_slider(&app, "Exposure");
        testing::press(&app, Key::UpArrow);
        settle();
        assert_eq!(states(), before + 1, "one step");
        assert!(near(
            state.borrow().sidecars[0].current.look().light.exposure,
            0.1
        ));
        testing::press(&app, Key::UpArrow);
        testing::press(&app, Key::UpArrow);
        testing::press(&app, Key::UpArrow);
        settle();
        assert_eq!(states(), before + 2, "three in a run, one step");
        assert!(near(
            state.borrow().sidecars[0].current.look().light.exposure,
            0.4
        ));
    }

    /// The window's keys with no slider focused: a rating, the arrow
    /// to the next frame. A focused slider takes the arrow and leaves
    /// the rating to the window; Escape gives the arrow back.
    #[test]
    fn a_focused_slider_takes_only_its_own_keys() {
        use crate::testing::press;
        use slint::platform::Key;
        use std::cell::RefCell;
        use std::rc::Rc;
        let app = crate::testing::window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        app.invoke_select(0);
        let rated = Rc::new(RefCell::new(Vec::<String>::new()));
        {
            let rated = rated.clone();
            app.on_meta_key(move |k| {
                rated.borrow_mut().push(k.to_string());
                true
            });
        }
        // Nothing focused: the window's.
        press(&app, "3");
        assert_eq!(*rated.borrow(), ["3"]);
        press(&app, Key::RightArrow);
        assert_eq!(
            state.borrow().current,
            Some(1),
            "the arrow reached the strip"
        );

        focus_slider(&app, "Exposure");
        press(&app, Key::RightArrow);
        assert_eq!(state.borrow().current, Some(1), "the slider took the arrow");
        assert!(near(app.get_exposure(), 0.1));
        press(&app, "4");
        assert_eq!(*rated.borrow(), ["3", "4"], "the rating reached the window");
        assert!(near(app.get_exposure(), 0.1));
        // The pointer crossing the picture leaves the keys with it.
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(700.0, 400.0),
            });
        press(&app, Key::UpArrow);
        assert!(near(app.get_exposure(), 0.2), "the slider kept the keys");
        assert_eq!(state.borrow().current, Some(1));

        press(&app, Key::Escape);
        press(&app, Key::RightArrow);
        assert_eq!(state.borrow().current, Some(2), "the window's arrow again");
    }

    /// G with a slider focused: the grid takes the keys, so its arrow
    /// moves the selection and plus zooms the tiles; the exposure
    /// stays where it was.
    #[test]
    fn the_grid_takes_the_keys_from_a_focused_slider() {
        use crate::testing::press;
        use slint::platform::Key;
        let (app, state, _worker) = exposure_focused();
        press(&app, "g");
        assert!(app.get_grid_open());
        press(&app, Key::RightArrow);
        assert_eq!(app.get_selected(), 1, "the grid's arrow");
        assert_eq!(state.borrow().current, Some(1));
        press(&app, "+");
        press(&app, "-");
        assert_eq!(app.get_exposure(), 0.0, "the slider let go");
    }

    /// L with a slider focused: the lightbox puts the panel away, and
    /// its keys are the window's at once.
    #[test]
    fn the_lightbox_takes_the_keys_from_a_focused_slider() {
        use crate::testing::press;
        use slint::platform::Key;
        let (app, _state, _worker) = exposure_focused();
        press(&app, "l");
        assert!(app.get_lightbox());
        let across = app.get_lightbox_across();
        press(&app, Key::UpArrow);
        press(&app, "-");
        assert_eq!(app.get_exposure(), 0.0, "the slider let go");
        assert_ne!(app.get_lightbox_across(), across, "the lightbox's minus");
        press(&app, "l");
        assert!(!app.get_lightbox(), "L out again");
    }

    /// F8 with a slider focused: the panel goes, and the window's keys
    /// work at once.
    #[test]
    fn hiding_the_panel_takes_the_keys_from_a_focused_slider() {
        use crate::testing::press;
        use slint::platform::Key;
        let (app, state, _worker) = exposure_focused();
        press(&app, Key::F8);
        assert!(app.get_right_hidden());
        press(&app, Key::RightArrow);
        assert_eq!(state.borrow().current, Some(1), "the window's arrow");
        assert_eq!(app.get_exposure(), 0.0);
    }

    /// The text of a block that starts just inside its brace, to the
    /// brace that closes it.
    fn block(from: &str) -> &str {
        let mut depth = 1;
        for (i, c) in from.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &from[..i];
                    }
                }
                _ => {}
            }
        }
        from
    }

    /// A property's expression in a block, from `key` to its `;` at
    /// the end of a line.
    fn field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
        let at = body
            .match_indices(key)
            .find(|&(i, _)| {
                body[..i].ends_with(' ') && body[..i].trim_end_matches(' ').ends_with('\n')
            })?
            .0;
        let rest = &body[at + key.len()..];
        let end = rest.find(";\n")?;
        Some(&rest[..end])
    }

    fn leading_number(text: &str) -> (f32, &str) {
        let end = text
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(text.len());
        (text[..end].parse().expect("a factor"), &text[end..])
    }
}
