//! The filter language: what a listing is narrowed by, parsed here
//! and translated to SQL here, so that the CLI, the filter bar and a
//! smart collection all mean the same thing by `iso>=3200`.
//!
//! A filter is terms separated by spaces, and a file passes when it
//! passes every term (§133's rule: narrowing is an and). A term is a
//! bare word, matched against the file's name and its keywords the
//! way the browser's search box matches, or a test on one field:
//!
//! ```text
//! camera:"R6"  lens:RF  iso>=3200  focal<=35  aperture:1.8  shutter<1/60
//! date:2026-09  rating>=3  flag:pick  label!=red  keyword:wedding
//! name:IMG_00  folder:/shoots/skye  missing:yes
//! ```
//!
//! `:` on a text field is a case-insensitive substring, `=` the
//! whole value and `!=` not that value; a value with a space in it
//! goes in double quotes. On a number `:` and `=` are equality and
//! `<`, `<=`, `>`, `>=` compare; a shutter may be written `1/250`,
//! a focal length `50mm`, an aperture `f/2.8`, and ISO and rating
//! are whole numbers. A date is a prefix, `2026`, `2026-09` or
//! `2026-09-21`, spelled with dashes or EXIF's colons, and every
//! operator compares that much of the file's own date, so
//! `date<=2026-09` is everything through September. A flag is
//! `pick`, `reject` or `none`, a label one of Lightroom's five or
//! `none`, and `missing` is `yes` or `no`. `!=` on any field leaves
//! out a file that does not say: a JPEG with no EXIF is not "not
//! ISO 100", it is unknown.
//!
//! Case is folded with Unicode's rules on both sides, as the
//! browser's search box folds it (`greycard-ui`'s `filter.rs`), so
//! `ärger` finds `Ärger.jpg`; the library registers `ulower` with
//! SQLite for it, since SQLite's own `LIKE` folds ASCII only. Where
//! the two differ: the box has no quotes and no fields, so there
//! `"low tide"` is two words with quote marks and `a:b` is a word,
//! and here the quotes make a phrase and `a:b` is refused as a
//! field this language does not have. The box is §133's and will
//! take this parser when the filter bar lands.
//!
//! The parser refuses what it cannot answer — a field it has no
//! column for, a rating of nine or of two and a half, `lens>50`, a
//! lone quote — because a filter that silently matched nothing, or
//! everything, would look like the library and not like a mistake.
//! Nothing here touches the database: [`Filter::to_sql`] returns a
//! `WHERE` body and its parameters, and the library binds them.

use std::fmt;

use greycard_edit::meta::{Flag, Label, STARS};

/// A parsed filter: the terms a file must pass, all of them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Filter {
    pub terms: Vec<Term>,
}

/// One test.
#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    /// A bare word: somewhere in the file's name or in one of its
    /// keywords, case ignored.
    Word(String),
    Text {
        field: TextField,
        op: TextOp,
        value: String,
    },
    Number {
        field: NumberField,
        op: Cmp,
        value: f64,
    },
    /// A date prefix, normalized to `YYYY-MM-DD HH:MM:SS` as far as
    /// it goes, compared against that much of the file's own.
    Date {
        op: Cmp,
        value: String,
    },
    Flag {
        is: bool,
        flag: Flag,
    },
    Label {
        is: bool,
        label: Label,
    },
    /// Whether the file is one the index last found gone from disk.
    Missing(bool),
}

/// The fields that hold words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextField {
    /// The body's name as the panel shows it, make and model joined
    /// (`greycard_core::raw::camera_name`).
    Camera,
    Make,
    Model,
    Lens,
    /// One of the file's keywords.
    Keyword,
    /// The file's name, without its folder.
    Name,
    /// The folder the file is in, as a path.
    Folder,
}

/// The fields that hold a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberField {
    Iso,
    /// Millimeters.
    Focal,
    /// The f-number.
    Aperture,
    /// Seconds.
    Shutter,
    /// Stars, 0 to 5.
    Rating,
}

/// How a text test reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOp {
    /// `:` — the value is somewhere in the field, case ignored.
    Contains,
    /// `=` — the field is the value, case ignored.
    Equals,
    /// `!=` — the field is not the value; a field with nothing in
    /// it passes.
    NotEquals,
}

/// How a number or a date compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    fn sql(self) -> &'static str {
        match self {
            Cmp::Eq => "=",
            Cmp::Ne => "<>",
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
        }
    }
}

/// A value bound into the SQL.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Text(String),
    Int(i64),
    Real(f64),
}

/// Why a filter did not parse: the term, and what was wrong with
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub term: String,
    pub reason: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.term, self.reason)
    }
}

impl std::error::Error for ParseError {}

/// The field names the language knows, for the error that lists
/// them.
const FIELDS: [&str; 16] = [
    "camera", "make", "model", "lens", "iso", "focal", "aperture", "shutter", "date", "rating",
    "flag", "label", "keyword", "name", "folder", "missing",
];

impl Filter {
    /// Parse a filter; an empty or blank string is the filter that
    /// passes everything.
    pub fn parse(text: &str) -> Result<Filter, ParseError> {
        let mut terms = Vec::new();
        for token in tokens(text) {
            terms.push(parse_term(&token)?);
        }
        Ok(Filter { terms })
    }

    /// A filter from arguments a shell has already split: each one
    /// is one term, quotes optional, so `lens:RF 24` typed as
    /// `'lens:RF 24'` is a lens test with a space in it and not two
    /// terms. A blank argument is nothing.
    pub fn from_terms<S: AsRef<str>>(args: &[S]) -> Result<Filter, ParseError> {
        let mut terms = Vec::new();
        for arg in args {
            let arg = arg.as_ref().trim();
            if arg.is_empty() {
                continue;
            }
            terms.push(parse_term(arg)?);
        }
        Ok(Filter { terms })
    }

    /// Whether nothing is asked.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether any term asks about missing files. A listing leaves
    /// the missing out unless one does.
    pub fn mentions_missing(&self) -> bool {
        self.terms.iter().any(|t| matches!(t, Term::Missing(_)))
    }

    /// The `WHERE` body and its parameters, one `?` a parameter in
    /// order. An empty filter is `1`.
    pub fn to_sql(&self) -> (String, Vec<Param>) {
        if self.terms.is_empty() {
            return ("1".to_string(), Vec::new());
        }
        let mut clauses = Vec::with_capacity(self.terms.len());
        let mut params = Vec::new();
        for term in &self.terms {
            clauses.push(term.sql(&mut params));
        }
        (clauses.join(" AND "), params)
    }
}

/// The tokens of a filter: split on whitespace outside double
/// quotes, the quotes kept for the term parser to strip.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// A token's surrounding quotes taken off.
fn unquote(s: &str) -> String {
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_suffix('"').unwrap_or(s);
    s.to_string()
}

/// One token to a term: `field op value`, or a word.
fn parse_term(token: &str) -> Result<Term, ParseError> {
    let err = |reason: String| ParseError {
        term: token.to_string(),
        reason,
    };
    // A field name is letters; a token that starts otherwise, or
    // has no operator after its letters, is a word.
    let name_len = token
        .char_indices()
        .find(|(_, c)| !c.is_ascii_alphabetic())
        .map(|(i, _)| i)
        .unwrap_or(token.len());
    let (name, rest) = token.split_at(name_len);
    // The two-character operators first, or `>=` would read as `>`
    // with a value starting `=`.
    let ops = ["!=", ">=", "<=", ":", "=", ">", "<"];
    let Some((op, value)) = (!name.is_empty())
        .then(|| {
            ops.iter()
                .find_map(|o| rest.strip_prefix(o).map(|v| (*o, v)))
        })
        .flatten()
    else {
        let word = unquote(token);
        if word.trim().is_empty() {
            return Err(err(
                "a word is wanted; a quote on its own is none".to_string()
            ));
        }
        one_term_only(&word, &err)?;
        return Ok(Term::Word(word.to_lowercase()));
    };
    let value = unquote(value);
    if value.is_empty() {
        return Err(err(format!("{name} wants a value after {op}")));
    }
    one_term_only(&value, &err)?;
    let field = name.to_ascii_lowercase();
    let text_field = match field.as_str() {
        "camera" => Some(TextField::Camera),
        "make" => Some(TextField::Make),
        "model" => Some(TextField::Model),
        "lens" => Some(TextField::Lens),
        "keyword" | "keywords" | "kw" => Some(TextField::Keyword),
        "name" | "file" => Some(TextField::Name),
        "folder" | "dir" => Some(TextField::Folder),
        _ => None,
    };
    if let Some(field) = text_field {
        let op = match op {
            ":" => TextOp::Contains,
            "=" => TextOp::Equals,
            "!=" => TextOp::NotEquals,
            _ => return Err(err(format!("{name} is words; use :, = or !=, not {op}"))),
        };
        // Folded here, once; the column is folded the same way in
        // the SQL.
        return Ok(Term::Text {
            field,
            op,
            value: value.to_lowercase(),
        });
    }
    let number_field = match field.as_str() {
        "iso" => Some(NumberField::Iso),
        "focal" | "mm" => Some(NumberField::Focal),
        "aperture" | "f" => Some(NumberField::Aperture),
        "shutter" | "exposure" => Some(NumberField::Shutter),
        "rating" | "stars" => Some(NumberField::Rating),
        _ => None,
    };
    let cmp = match op {
        ":" | "=" => Cmp::Eq,
        "!=" => Cmp::Ne,
        "<" => Cmp::Lt,
        "<=" => Cmp::Le,
        ">" => Cmp::Gt,
        ">=" => Cmp::Ge,
        _ => unreachable!("every operator is listed"),
    };
    if let Some(field) = number_field {
        let value = number(&value, field)
            .ok_or_else(|| err(format!("{name} wants a number, not {value:?}")))?;
        if field == NumberField::Rating && (value < 0.0 || value > f64::from(STARS)) {
            return Err(err(format!("a rating is 0 to {STARS} stars")));
        }
        // A whole-number field takes a whole number: `rating>2.5`
        // rounded to `rating>3` would drop the three-star frames it
        // was asking for.
        if field.is_whole() && value.fract() != 0.0 {
            return Err(err(format!("{name} is a whole number; {value} is not one")));
        }
        return Ok(Term::Number {
            field,
            op: cmp,
            value,
        });
    }
    match field.as_str() {
        "date" | "taken" => {
            let value = date_prefix(&value).ok_or_else(|| {
                err(format!(
                    "a date is 2026, 2026-09 or 2026-09-21, optionally with a time; not {value:?}"
                ))
            })?;
            Ok(Term::Date { op: cmp, value })
        }
        "flag" => {
            let is = yes_or_not(op, name, &err)?;
            let flag = match value.to_ascii_lowercase().as_str() {
                "pick" | "picked" | "p" => Flag::Pick,
                "reject" | "rejected" | "x" => Flag::Reject,
                "none" | "unflagged" | "u" => Flag::None,
                _ => {
                    return Err(err(format!(
                        "a flag is pick, reject or none, not {value:?}"
                    )));
                }
            };
            Ok(Term::Flag { is, flag })
        }
        "label" | "color" | "colour" => {
            let is = yes_or_not(op, name, &err)?;
            let label = match value.to_ascii_lowercase().as_str() {
                "red" => Label::Red,
                "yellow" => Label::Yellow,
                "green" => Label::Green,
                "blue" => Label::Blue,
                "purple" => Label::Purple,
                "none" => Label::None,
                _ => {
                    return Err(err(format!(
                        "a label is red, yellow, green, blue, purple or none, not {value:?}"
                    )));
                }
            };
            Ok(Term::Label { is, label })
        }
        "missing" => {
            let is = yes_or_not(op, name, &err)?;
            let yes = match value.to_ascii_lowercase().as_str() {
                "yes" | "true" | "1" => true,
                "no" | "false" | "0" => false,
                _ => return Err(err(format!("missing is yes or no, not {value:?}"))),
            };
            Ok(Term::Missing(is == yes))
        }
        _ => Err(err(format!(
            "no field called {name}; the fields are {}",
            FIELDS.join(", ")
        ))),
    }
}

/// Whether a token is shaped like a term: letters, an operator, and
/// something after it.
fn looks_like_a_term(token: &str) -> bool {
    let name_len = token
        .char_indices()
        .find(|(_, c)| !c.is_ascii_alphabetic())
        .map(|(i, _)| i)
        .unwrap_or(token.len());
    if name_len == 0 {
        return false;
    }
    // Only a field this language has: `C:\Users\...` in a quoted
    // folder is a drive letter, not a term.
    let name = token[..name_len].to_ascii_lowercase();
    if !FIELDS.contains(&name.as_str()) {
        return false;
    }
    let rest = &token[name_len..];
    ["!=", ">=", "<=", ":", "=", ">", "<"]
        .iter()
        .any(|op| rest.strip_prefix(op).is_some_and(|v| !v.is_empty()))
}

/// A value with another term inside it is a whole filter handed over
/// as one argument — `list 'camera:R6 iso>=3200'` — which would
/// otherwise be a camera called "R6 iso>=3200" that matches nothing.
fn one_term_only(value: &str, err: &dyn Fn(String) -> ParseError) -> Result<(), ParseError> {
    match value.split_whitespace().find(|t| looks_like_a_term(t)) {
        Some(inner) => Err(err(format!(
            "{inner:?} looks like another term; one term per argument, spaces inside quotes"
        ))),
        None => Ok(()),
    }
}

/// `:` and `=` ask for the value, `!=` for anything else; the
/// ordering operators mean nothing on a flag or a label.
fn yes_or_not(
    op: &str,
    name: &str,
    err: &dyn Fn(String) -> ParseError,
) -> Result<bool, ParseError> {
    match op {
        ":" | "=" => Ok(true),
        "!=" => Ok(false),
        _ => Err(err(format!("{name} takes :, = or !=, not {op}"))),
    }
}

/// A number as a filter spells it: plain, a fraction (`1/250`), or
/// with the unit the field is read in (`50mm`, `f/2.8`, `1/60s`,
/// `ISO 800` without the space).
fn number(value: &str, field: NumberField) -> Option<f64> {
    let v = value.trim().to_ascii_lowercase();
    let v = match field {
        NumberField::Focal => v.strip_suffix("mm").unwrap_or(&v),
        NumberField::Aperture => v.strip_prefix("f/").or(v.strip_prefix('f')).unwrap_or(&v),
        NumberField::Shutter => v.strip_suffix('s').unwrap_or(&v),
        NumberField::Iso => v.strip_prefix("iso").unwrap_or(&v),
        NumberField::Rating => &v,
    }
    .trim();
    let parsed = match v.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
            (d != 0.0).then(|| n / d)?
        }
        None => v.parse().ok()?,
    };
    (parsed.is_finite() && parsed >= 0.0).then_some(parsed)
}

/// A date as a filter spells it, to the prefix of `YYYY-MM-DD
/// HH:MM:SS` it names: `2026`, `2026-09`, `2026:09:21`,
/// `2026-09-21T15:30`. Anything that is not digits in those places
/// is refused.
fn date_prefix(value: &str) -> Option<String> {
    let v = value.trim();
    // The date part: colons and dashes both, since EXIF spells it
    // with colons and everyone else with dashes.
    let (date, time) = match v.find(['T', 't', ' ']) {
        Some(i) => (&v[..i], Some(&v[i + 1..])),
        None => (v, None),
    };
    let mut out = String::new();
    for (i, part) in date.split(['-', ':']).enumerate() {
        let width = if i == 0 { 4 } else { 2 };
        if i > 2 || part.len() != width || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if i > 0 {
            out.push('-');
        }
        out.push_str(part);
    }
    if out.len() < 4 {
        return None;
    }
    if let Some(time) = time {
        if out.len() != 10 {
            return None;
        }
        out.push(' ');
        for (i, part) in time.split(':').enumerate() {
            if i > 2 || part.len() != 2 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            if i > 0 {
                out.push(':');
            }
            out.push_str(part);
        }
    }
    Some(out)
}

/// A `LIKE` pattern that finds `value` anywhere, its own wildcards
/// escaped; the clause says `ESCAPE '\'`.
fn contains_pattern(value: &str) -> String {
    let mut p = String::with_capacity(value.len() + 2);
    p.push('%');
    for c in value.chars() {
        if matches!(c, '%' | '_' | '\\') {
            p.push('\\');
        }
        p.push(c);
    }
    p.push('%');
    p
}

impl TextField {
    /// The column, or `None` for the keyword table.
    fn column(self) -> Option<&'static str> {
        Some(match self {
            TextField::Camera => "files.camera",
            TextField::Make => "files.make",
            TextField::Model => "files.model",
            TextField::Lens => "files.lens",
            TextField::Name => "files.name",
            TextField::Folder => "files.folder_text",
            TextField::Keyword => return None,
        })
    }
}

impl NumberField {
    fn column(self) -> &'static str {
        match self {
            NumberField::Iso => "files.iso",
            NumberField::Focal => "files.focal",
            NumberField::Aperture => "files.aperture",
            NumberField::Shutter => "files.shutter",
            NumberField::Rating => "files.rating",
        }
    }

    /// Whether the field holds whole numbers, compared as such.
    pub fn is_whole(self) -> bool {
        matches!(self, NumberField::Iso | NumberField::Rating)
    }

    /// The window a real value is matched through: two f/2.8s are
    /// equal, and so are a shutter written 1/250 and one read as
    /// 0.004000000000000000083. For aperture and shutter it is half
    /// a percent either way, under a tenth of a stop; a focal length
    /// is a tenth of a millimetre, since 24 and 23.9 are different
    /// settings of a zoom and a window of a percent would join them.
    fn window(self, value: f64) -> (f64, f64) {
        match self {
            NumberField::Focal => (value - 0.05, value + 0.05),
            _ => (value * 0.995, value * 1.005),
        }
    }
}

/// A text column folded the way the values are.
fn folded(col: &str) -> String {
    format!("ulower({col})")
}

impl Term {
    /// This term's clause, its parameters pushed in order.
    fn sql(&self, params: &mut Vec<Param>) -> String {
        match self {
            Term::Word(word) => {
                let pattern = contains_pattern(word);
                params.push(Param::Text(pattern.clone()));
                params.push(Param::Text(pattern));
                "(ulower(files.name) LIKE ? ESCAPE '\\' OR EXISTS (SELECT 1 FROM keywords k \
                 WHERE k.file = files.id AND ulower(k.word) LIKE ? ESCAPE '\\'))"
                    .to_string()
            }
            Term::Text { field, op, value } => match field.column() {
                Some(col) => {
                    let col = folded(col);
                    match op {
                        TextOp::Contains => {
                            params.push(Param::Text(contains_pattern(value)));
                            format!("{col} LIKE ? ESCAPE '\\'")
                        }
                        TextOp::Equals => {
                            params.push(Param::Text(value.clone()));
                            format!("{col} = ?")
                        }
                        // A column that is NULL or empty is a file
                        // that did not say, and is left out.
                        TextOp::NotEquals => {
                            params.push(Param::Text(value.clone()));
                            format!("({col} <> ? AND {col} <> '')")
                        }
                    }
                }
                None => {
                    let (exists, test) = match op {
                        TextOp::Contains => {
                            params.push(Param::Text(contains_pattern(value)));
                            ("EXISTS", "ulower(k.word) LIKE ? ESCAPE '\\'")
                        }
                        TextOp::Equals => {
                            params.push(Param::Text(value.clone()));
                            ("EXISTS", "ulower(k.word) = ?")
                        }
                        // No keywords is a known thing, not an
                        // unknown one, so such a file passes.
                        TextOp::NotEquals => {
                            params.push(Param::Text(value.clone()));
                            ("NOT EXISTS", "ulower(k.word) = ?")
                        }
                    };
                    format!(
                        "{exists} (SELECT 1 FROM keywords k WHERE k.file = files.id AND {test})"
                    )
                }
            },
            Term::Number { field, op, value } => {
                let col = field.column();
                if field.is_whole() {
                    // Whole numbers compare as such; the parser
                    // refused a fraction. A NULL compares as nothing
                    // and is left out, `<>` included.
                    params.push(Param::Int(*value as i64));
                    return format!("{col} {} ?", op.sql());
                }
                // A real is compared through its window: equal is
                // inside it, `<` is below it and `<=` is not above
                // it, so `aperture<2.8` leaves the f/2.8 frames out
                // and `aperture<=2.8` takes them.
                let (lo, hi) = field.window(*value);
                match op {
                    Cmp::Eq => {
                        params.push(Param::Real(lo));
                        params.push(Param::Real(hi));
                        format!("({col} >= ? AND {col} <= ?)")
                    }
                    Cmp::Ne => {
                        params.push(Param::Real(lo));
                        params.push(Param::Real(hi));
                        format!("({col} < ? OR {col} > ?)")
                    }
                    Cmp::Lt | Cmp::Ge => {
                        params.push(Param::Real(lo));
                        format!("{col} {} ?", op.sql())
                    }
                    Cmp::Le | Cmp::Gt => {
                        params.push(Param::Real(hi));
                        format!("{col} {} ?", op.sql())
                    }
                }
            }
            Term::Date { op, value } => {
                params.push(Param::Int(value.len() as i64));
                params.push(Param::Text(value.clone()));
                format!("substr(files.taken, 1, ?) {} ?", op.sql())
            }
            Term::Flag { is, flag } => {
                params.push(Param::Text(flag_name(*flag).to_string()));
                format!("files.flag {} ?", if *is { "=" } else { "<>" })
            }
            Term::Label { is, label } => {
                params.push(Param::Text(label_name(*label).to_string()));
                format!("files.label {} ?", if *is { "=" } else { "<>" })
            }
            Term::Missing(yes) => if *yes {
                "files.missing_since IS NOT NULL"
            } else {
                "files.missing_since IS NULL"
            }
            .to_string(),
        }
    }
}

/// A flag as the `flag` column spells it: serde's lowercase names,
/// so the column and the sidecar agree.
pub fn flag_name(flag: Flag) -> &'static str {
    match flag {
        Flag::None => "none",
        Flag::Pick => "pick",
        Flag::Reject => "reject",
    }
}

pub fn flag_from_name(name: &str) -> Flag {
    match name {
        "pick" => Flag::Pick,
        "reject" => Flag::Reject,
        _ => Flag::None,
    }
}

/// A label as the `label` column spells it.
pub fn label_name(label: Label) -> &'static str {
    match label {
        Label::None => "none",
        Label::Red => "red",
        Label::Yellow => "yellow",
        Label::Green => "green",
        Label::Blue => "blue",
        Label::Purple => "purple",
    }
}

pub fn label_from_name(name: &str) -> Label {
    match name {
        "red" => Label::Red,
        "yellow" => Label::Yellow,
        "green" => Label::Green,
        "blue" => Label::Blue,
        "purple" => Label::Purple,
        _ => Label::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Term {
        let f = Filter::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(f.terms.len(), 1, "{text}: {:?}", f.terms);
        f.terms.into_iter().next().unwrap()
    }

    fn refused(text: &str) -> String {
        Filter::parse(text)
            .expect_err(&format!("{text} should not parse"))
            .to_string()
    }

    #[test]
    fn an_empty_filter_passes_everything() {
        assert!(Filter::parse("").unwrap().is_empty());
        assert!(Filter::parse("   \t ").unwrap().is_empty());
        assert_eq!(
            Filter::parse("").unwrap().to_sql(),
            ("1".to_string(), vec![])
        );
    }

    #[test]
    fn the_example_from_the_roadmap_parses_term_by_term() {
        let f = Filter::parse(
            r#"camera:"R6" iso>=3200 rating>=3 flag:pick keyword:wedding date:2026-09"#,
        )
        .unwrap();
        assert_eq!(
            f.terms,
            vec![
                Term::Text {
                    field: TextField::Camera,
                    op: TextOp::Contains,
                    value: "r6".into()
                },
                Term::Number {
                    field: NumberField::Iso,
                    op: Cmp::Ge,
                    value: 3200.0
                },
                Term::Number {
                    field: NumberField::Rating,
                    op: Cmp::Ge,
                    value: 3.0
                },
                Term::Flag {
                    is: true,
                    flag: Flag::Pick
                },
                Term::Text {
                    field: TextField::Keyword,
                    op: TextOp::Contains,
                    value: "wedding".into()
                },
                Term::Date {
                    op: Cmp::Eq,
                    value: "2026-09".into()
                },
            ]
        );
        let (sql, params) = f.to_sql();
        assert!(sql.starts_with("ulower(files.camera) LIKE ?"), "{sql}");
        assert!(sql.ends_with("substr(files.taken, 1, ?) = ?"), "{sql}");
        assert_eq!(sql.matches('?').count(), params.len(), "{sql}\n{params:?}");
        assert_eq!(params.len(), 7, "{params:?}");
    }

    #[test]
    fn quotes_hold_a_space_and_come_off() {
        assert_eq!(
            one(r#"lens:"RF 100-500mm""#),
            Term::Text {
                field: TextField::Lens,
                op: TextOp::Contains,
                value: "rf 100-500mm".into()
            }
        );
        // A quoted bare phrase is one word, lowercased as the
        // browser's search box lowercases.
        assert_eq!(one(r#""Low Tide""#), Term::Word("low tide".into()));
        // And two words are two terms.
        assert_eq!(
            Filter::parse("harbor 0002").unwrap().terms,
            vec![Term::Word("harbor".into()), Term::Word("0002".into())]
        );
    }

    #[test]
    fn text_fields_take_the_three_word_operators_and_no_other() {
        assert_eq!(
            one("camera=Canon"),
            Term::Text {
                field: TextField::Camera,
                op: TextOp::Equals,
                value: "canon".into()
            }
        );
        assert_eq!(
            one("make!=sony"),
            Term::Text {
                field: TextField::Make,
                op: TextOp::NotEquals,
                value: "sony".into()
            }
        );
        assert!(refused("lens>50").contains("words"));
        assert!(refused("name:").contains("wants a value"));
    }

    #[test]
    fn numbers_come_with_their_units_and_fractions() {
        let value = |t: &str| match one(t) {
            Term::Number { value, .. } => value,
            other => panic!("{t}: {other:?}"),
        };
        assert_eq!(value("focal:50mm"), 50.0);
        assert_eq!(value("focal<=35"), 35.0);
        assert_eq!(value("aperture:f/2.8"), 2.8);
        assert_eq!(value("aperture>f4"), 4.0);
        assert_eq!(value("shutter<1/250"), 1.0 / 250.0);
        assert_eq!(value("shutter>=2s"), 2.0);
        assert_eq!(value("iso:ISO800"), 800.0);
        assert_eq!(value("rating:0"), 0.0);
        assert_eq!(value("focal:23.9"), 23.9);
        assert!(refused("iso:high").contains("number"));
        assert!(refused("rating>9").contains("0 to 5"));
        assert!(refused("shutter:1/0").contains("number"));
    }

    #[test]
    fn a_date_is_a_prefix_spelled_either_way() {
        let value = |t: &str| match one(t) {
            Term::Date { value, .. } => value,
            other => panic!("{t}: {other:?}"),
        };
        assert_eq!(value("date:2026"), "2026");
        assert_eq!(value("date:2026-09"), "2026-09");
        assert_eq!(value("date:2026:09:21"), "2026-09-21");
        assert_eq!(value("date>=2026-09-21T15:30"), "2026-09-21 15:30");
        assert_eq!(
            value(r#"date:"2026-09-21 15:30:00""#),
            "2026-09-21 15:30:00"
        );
        assert_eq!(value("taken<2027"), "2027");
        assert!(refused("date:sept").contains("a date is"));
        assert!(refused("date:2026-9").contains("a date is"));
        assert!(refused("date:2026-09-21-01").contains("a date is"));
        assert!(refused("date:2026T15").contains("a date is"));
    }

    #[test]
    fn flags_labels_and_missing_are_named_values() {
        assert_eq!(
            one("flag:pick"),
            Term::Flag {
                is: true,
                flag: Flag::Pick
            }
        );
        assert_eq!(
            one("flag!=reject"),
            Term::Flag {
                is: false,
                flag: Flag::Reject
            }
        );
        assert_eq!(
            one("flag:unflagged"),
            Term::Flag {
                is: true,
                flag: Flag::None
            }
        );
        assert_eq!(
            one("label:Red"),
            Term::Label {
                is: true,
                label: Label::Red
            }
        );
        assert_eq!(
            one("label!=none"),
            Term::Label {
                is: false,
                label: Label::None
            }
        );
        assert_eq!(one("missing:yes"), Term::Missing(true));
        assert_eq!(one("missing!=yes"), Term::Missing(false));
        assert!(Filter::parse("missing:no").unwrap().mentions_missing());
        assert!(!Filter::parse("flag:pick").unwrap().mentions_missing());
        assert!(refused("flag:maybe").contains("pick, reject or none"));
        assert!(refused("label:grey").contains("red, yellow"));
        assert!(refused("flag>pick").contains("takes"));
    }

    #[test]
    fn an_unknown_field_is_refused_and_a_word_with_a_colon_is_a_word() {
        let said = refused("camra:R6");
        assert!(said.contains("no field called camra"), "{said}");
        assert!(said.contains("camera"), "{said}");
        // Starts with a digit: not a field, a word.
        assert_eq!(one("12:30"), Term::Word("12:30".into()));
        // Letters but no operator: a word.
        assert_eq!(one("IMG_0001"), Term::Word("img_0001".into()));
    }

    #[test]
    fn the_sql_binds_one_parameter_a_question_mark() {
        let checks = [
            (
                r#"camera:"EOS R6""#,
                "ulower(files.camera) LIKE ? ESCAPE '\\'",
            ),
            ("make=Canon", "ulower(files.make) = ?"),
            (
                "lens!=kit",
                "(ulower(files.lens) <> ? AND ulower(files.lens) <> '')",
            ),
            ("iso>=3200", "files.iso >= ?"),
            ("iso!=100", "files.iso <> ?"),
            ("rating:3", "files.rating = ?"),
            ("focal:50", "(files.focal >= ? AND files.focal <= ?)"),
            ("shutter!=1/250", "(files.shutter < ? OR files.shutter > ?)"),
            ("aperture<2", "files.aperture < ?"),
            ("date:2026-09", "substr(files.taken, 1, ?) = ?"),
            ("date<=2026", "substr(files.taken, 1, ?) <= ?"),
            ("date!=2026", "substr(files.taken, 1, ?) <> ?"),
            ("flag:pick", "files.flag = ?"),
            ("label!=red", "files.label <> ?"),
            ("missing:yes", "files.missing_since IS NOT NULL"),
            ("missing:no", "files.missing_since IS NULL"),
            (
                "keyword:wedding",
                "EXISTS (SELECT 1 FROM keywords k WHERE k.file = files.id AND ulower(k.word) LIKE ? ESCAPE '\\')",
            ),
            (
                "keyword=wedding",
                "EXISTS (SELECT 1 FROM keywords k WHERE k.file = files.id AND ulower(k.word) = ?)",
            ),
            (
                "keyword!=wedding",
                "NOT EXISTS (SELECT 1 FROM keywords k WHERE k.file = files.id AND ulower(k.word) = ?)",
            ),
        ];
        for (text, want) in checks {
            let (sql, params) = Filter::parse(text).unwrap().to_sql();
            assert_eq!(sql, want, "{text}");
            assert_eq!(sql.matches('?').count(), params.len(), "{text}: {params:?}");
        }
        // The date binds its own length, so the prefix is compared
        // against that much of the file's date.
        let (_, params) = Filter::parse("date:2026-09").unwrap().to_sql();
        assert_eq!(params, vec![Param::Int(7), Param::Text("2026-09".into())]);
        // A real's strict comparison is against the edge of its
        // window, the near edge for < and >=, the far one for <= and
        // >; equality binds both.
        let real = |t: &str| match Filter::parse(t).unwrap().to_sql().1.as_slice() {
            [Param::Real(v)] => *v,
            other => panic!("{t}: {other:?}"),
        };
        assert!((real("aperture<2.8") - 2.8 * 0.995).abs() < 1e-12);
        assert!((real("aperture>=2.8") - 2.8 * 0.995).abs() < 1e-12);
        assert!((real("aperture<=2.8") - 2.8 * 1.005).abs() < 1e-12);
        assert!((real("aperture>2.8") - 2.8 * 1.005).abs() < 1e-12);
        // A focal length's window is a tenth of a millimetre, not a
        // percent: 23.9 is not 24.
        let (_, params) = Filter::parse("focal:50").unwrap().to_sql();
        match params.as_slice() {
            [Param::Real(lo), Param::Real(hi)] => {
                assert!(
                    (lo - 49.95).abs() < 1e-9 && (hi - 50.05).abs() < 1e-9,
                    "{params:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!((real("focal>=24") - 23.95).abs() < 1e-9);
        // A whole number binds as one, whatever it was spelled as.
        assert_eq!(
            Filter::parse("iso:800").unwrap().to_sql().1,
            vec![Param::Int(800)]
        );
        // A word searches the name and the keywords with one
        // pattern, both sides folded.
        let (sql, params) = Filter::parse("Har%bor").unwrap().to_sql();
        assert_eq!(params, vec![Param::Text("%har\\%bor%".into()); 2]);
        assert!(sql.starts_with("(ulower(files.name) LIKE ?"), "{sql}");
        // And a text value is folded at parse time.
        assert_eq!(
            Filter::parse("camera:ÄRGER").unwrap().to_sql().1,
            vec![Param::Text("%ärger%".into())]
        );
    }

    #[test]
    fn a_whole_number_field_refuses_a_fraction() {
        assert!(refused("rating>2.5").contains("whole number"));
        assert!(refused("iso>3199.6").contains("whole number"));
        assert!(refused("iso:1/2").contains("whole number"));
        assert_eq!(
            one("rating>2"),
            Term::Number {
                field: NumberField::Rating,
                op: Cmp::Gt,
                value: 2.0
            }
        );
        assert_eq!(
            one("iso:3200.0"),
            Term::Number {
                field: NumberField::Iso,
                op: Cmp::Eq,
                value: 3200.0
            }
        );
        // The real fields still take one.
        assert!(matches!(one("focal:24.5"), Term::Number { value, .. } if value == 24.5));
    }

    #[test]
    fn a_lone_quote_is_refused_and_arguments_are_one_term_each() {
        assert!(refused("\"").contains("a word is wanted"));
        assert!(refused("iso>=800 \"").contains("a word is wanted"));
        assert!(refused("\"\"").contains("a word is wanted"));
        // From a shell: an argument with a space is one term, with
        // or without quotes, and a blank argument is nothing.
        let f = Filter::from_terms(&["lens:RF 24", "iso>=3200", "", "low tide"]).unwrap();
        assert_eq!(
            f.terms,
            vec![
                Term::Text {
                    field: TextField::Lens,
                    op: TextOp::Contains,
                    value: "rf 24".into()
                },
                Term::Number {
                    field: NumberField::Iso,
                    op: Cmp::Ge,
                    value: 3200.0
                },
                Term::Word("low tide".into()),
            ]
        );
        assert_eq!(
            Filter::from_terms(&["lens:\"RF 24\""]).unwrap().terms,
            Filter::from_terms(&["lens:RF 24"]).unwrap().terms
        );
        assert!(Filter::from_terms::<&str>(&[]).unwrap().is_empty());
        assert!(Filter::from_terms(&["camra:R6"]).is_err());
        // A whole filter in one argument is refused with the hint,
        // whether it starts with a field or a word.
        let said = Filter::from_terms(&["camera:R6 iso>=3200"])
            .unwrap_err()
            .to_string();
        assert!(said.contains("one term per argument"), "{said}");
        assert!(said.contains("iso>=3200"), "{said}");
        assert!(
            Filter::from_terms(&["harbor flag:pick"])
                .unwrap_err()
                .to_string()
                .contains("one term per argument")
        );
        // A value with a space and no operator in it is fine, and so
        // is a token that starts with a digit.
        assert!(Filter::from_terms(&["lens:RF 24-105", "keyword:noon 12:30"]).is_ok());
    }

    #[test]
    fn a_like_pattern_escapes_its_own_wildcards() {
        assert_eq!(contains_pattern("a_b%c\\d"), "%a\\_b\\%c\\\\d%");
        assert_eq!(contains_pattern("plain"), "%plain%");
    }

    #[test]
    fn the_names_round_trip() {
        for flag in Flag::ALL {
            assert_eq!(flag_from_name(flag_name(flag)), flag);
        }
        for label in Label::ALL {
            assert_eq!(label_from_name(label_name(label)), label);
        }
        assert_eq!(flag_from_name("picked"), Flag::None);
        assert_eq!(label_from_name("grey"), Label::None);
    }
}
