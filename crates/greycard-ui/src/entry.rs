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
mod tests;
