//! What the fuzz targets share: the round trips they assert, and a
//! filter printed back as the text that spells it.
//!
//! Every target holds one rule beyond "never panics": what a reader
//! accepts, the editor can write and read again. A float that is not
//! finite is the case in point. serde_json writes one as `null`, which
//! does not read back where a float belongs, so a reader that lets one
//! through writes a sidecar the editor then refuses.

use greycard_edit::{Edit, Preset, Sidecar};
use greycard_library::filter::{Cmp, Filter, NumberField, Term, TextField, TextOp};

/// `edit` writes as JSON and reads back as it was.
pub fn edit_round_trips(edit: &Edit, what: &str) {
    let json = edit.to_json();
    let back = Edit::from_json(&json)
        .unwrap_or_else(|e| panic!("{what}: the edit does not read back: {e}\n{json}"));
    assert_eq!(&back, edit, "{what}: the edit reads back changed");
}

/// `preset` writes as JSON and reads back as it was.
pub fn preset_round_trips(preset: &Preset, what: &str) {
    let json = preset.to_json();
    let back = Preset::from_json(&json)
        .unwrap_or_else(|e| panic!("{what}: the preset does not read back: {e}\n{json}"));
    assert_eq!(&back, preset, "{what}: the preset reads back changed");
}

/// `preset` laid on a fresh edit, and that edit round-trips: what an
/// import writes into a sidecar.
pub fn applied_round_trips(preset: &Preset, what: &str) {
    let mut edit = Edit::default();
    preset.apply(&mut edit);
    edit_round_trips(&edit, what);
}

/// `sidecar` writes as JSON, reads back, and writes the same JSON
/// again. A sidecar has no equality of its own, and what it writes is
/// all of what it keeps.
pub fn sidecar_round_trips(sidecar: &Sidecar, what: &str) {
    let json = sidecar.to_json();
    let back = Sidecar::from_json(&json)
        .unwrap_or_else(|e| panic!("{what}: the sidecar does not read back: {e}\n{json}"));
    assert_eq!(
        back.to_json(),
        json,
        "{what}: the sidecar reads back changed"
    );
}

/// The text a parsed filter is spelled by, or `None` when a value in
/// it holds a double quote. The language has no escape for one: a
/// quote in a value can be read (`a"b` is a word) but not always
/// written back, so those filters are left out of the round trip
/// rather than reported. A facet term has no spelling and is never
/// parsed from text; it is `None` too.
pub fn filter_text(filter: &Filter) -> Option<String> {
    let mut out = Vec::with_capacity(filter.terms.len());
    for term in &filter.terms {
        out.push(term_text(term)?);
    }
    Some(out.join(" "))
}

fn quoted(value: &str) -> Option<String> {
    (!value.contains('"')).then(|| format!("\"{value}\""))
}

fn cmp(op: Cmp) -> &'static str {
    match op {
        Cmp::Eq => "=",
        Cmp::Ne => "!=",
        Cmp::Lt => "<",
        Cmp::Le => "<=",
        Cmp::Gt => ">",
        Cmp::Ge => ">=",
    }
}

fn is(yes: bool) -> &'static str {
    if yes { "=" } else { "!=" }
}

fn term_text(term: &Term) -> Option<String> {
    Some(match term {
        Term::Word(word) => quoted(word)?,
        Term::Text { field, op, value } => {
            let field = match field {
                TextField::Camera => "camera",
                TextField::Make => "make",
                TextField::Model => "model",
                TextField::Lens => "lens",
                TextField::Keyword => "keyword",
                TextField::Name => "name",
                TextField::Folder => "folder",
            };
            let op = match op {
                TextOp::Contains => ":",
                TextOp::Equals => "=",
                TextOp::NotEquals => "!=",
            };
            format!("{field}{op}{}", quoted(value)?)
        }
        Term::Number { field, op, value } => {
            let field = match field {
                NumberField::Iso => "iso",
                NumberField::Focal => "focal",
                NumberField::Aperture => "aperture",
                NumberField::Shutter => "shutter",
                NumberField::Rating => "rating",
            };
            format!("{field}{}{value}", cmp(*op))
        }
        // Normalized with a space between the day and the time, which
        // would split the token; the parser takes a `T` there too.
        Term::Date { op, value } => format!("date{}{}", cmp(*op), value.replace(' ', "T")),
        Term::Flag { is: yes, flag } => format!(
            "flag{}{}",
            is(*yes),
            greycard_library::filter::flag_name(*flag)
        ),
        Term::Label { is: yes, label } => format!(
            "label{}{}",
            is(*yes),
            greycard_library::filter::label_name(*label)
        ),
        Term::Missing(yes) => format!("missing:{}", if *yes { "yes" } else { "no" }),
        Term::HasVersions(yes) => format!("has{}versions", is(*yes)),
        Term::Kind { is: yes, kind } => format!("kind{}{kind}", is(*yes)),
        Term::Facet { .. } => return None,
    })
}
