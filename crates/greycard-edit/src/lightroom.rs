//! Lightroom presets read: the `crs:` settings of an XMP file, mapped
//! onto the parts of an edit that mean the same thing.
//!
//! Lightroom's sliders are on scales of its own, most of them -100 to
//! 100 over a tone pipeline this engine does not share, so every
//! mapping here is a scale chosen to land about where the slider
//! lands there; a preset comes across as a starting point, not a
//! match. What has no counterpart, the calibration, masks, is
//! passed over and named, so the user knows what to set by hand.
//!
//! `ConvertToGrayscale` (or a Black & White treatment) turns the
//! black and white section on, and only then are
//! `GrayMixerRed..GrayMixerMagenta` read as its eight weights, band
//! for band; the color mixer keeps its own HSL bands.
//!
//! A Lightroom catalog keeps the same keys for each image as a Lua
//! table rather than XMP ([`read_table`]), and its smart collections'
//! rules in the same form ([`parse_table`]).

use std::collections::BTreeMap;

pub mod crop;

use crate::curve::Point;
use crate::grading::Wheel;
use crate::mixer::BANDS;
use crate::preset::{Preset, Section};
use crate::{Edit, Error, Result, WhiteBalance};

/// The Camera Raw settings namespace.
const CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// A preset read from Lightroom, with what could not come across.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub preset: Preset,
    /// Settings the file had, set to something, that have no place
    /// here; Lightroom's names for them.
    pub unmapped: Vec<String>,
}

/// Read a Lightroom preset or a picture's XMP sidecar. `fallback` is
/// the name when the file gives none, usually its stem.
pub fn read(xml: &str, fallback: &str) -> Result<Imported> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error::Preset(e.to_string()))?;
    let mut keys: BTreeMap<String, String> = BTreeMap::new();
    let mut curves: BTreeMap<String, Vec<Point>> = BTreeMap::new();
    let mut name = None;
    for desc in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().namespace() == Some(RDF))
        .filter(|n| n.tag_name().name() == "Description")
    {
        for a in desc.attributes().filter(|a| a.namespace() == Some(CRS)) {
            keys.insert(a.name().to_string(), a.value().to_string());
        }
        for child in desc
            .children()
            .filter(|n| n.is_element() && n.tag_name().namespace() == Some(CRS))
        {
            let key = child.tag_name().name();
            if key == "Name" || key == "Group" {
                if key == "Name" {
                    name = alt_text(&child).map(str::to_string);
                }
            } else if key.starts_with("ToneCurve") && !key.ends_with("Name") {
                curves.insert(key.to_string(), seq_points(&child));
            } else if let Some(text) = child.text().map(str::trim).filter(|t| !t.is_empty())
                && !child.children().any(|n| n.is_element())
            {
                keys.insert(key.to_string(), text.to_string());
            } else if child.descendants().any(|n| {
                n.is_element()
                    && n != child
                    && !matches!(n.tag_name().name(), "Seq" | "Bag" | "Alt")
            }) {
                // A structure (the masks, the spots healed, the lens
                // blur) is kept as its key alone, as a catalog's
                // table is, so what it holds is named among what did
                // not come across. An empty list holds nothing.
                keys.insert(key.to_string(), String::new());
            }
        }
    }
    if keys.is_empty() && curves.is_empty() {
        return Err(Error::Preset("no Camera Raw settings in the file".into()));
    }
    let name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string());
    Ok(map(name, &keys, &curves))
}

/// Read the develop settings a Lightroom catalog keeps for an image:
/// the same Camera Raw keys as a preset's XMP, written as a Lua table
/// (`s = { Exposure2012 = 0.5, ToneCurvePV2012 = { 0, 0, 255, 255 },
/// ... }`), mapped as [`read`] maps them. The tables inside it that
/// are not curves (the masks, the spots healed, the Enhance filters)
/// are kept as their key alone, so what they hold is named among what
/// did not come across.
pub fn read_table(text: &str, name: &str) -> Result<Imported> {
    let top = parse_table(text).map_err(Error::Preset)?;
    let mut keys: BTreeMap<String, String> = BTreeMap::new();
    let mut curves: BTreeMap<String, Vec<Point>> = BTreeMap::new();
    for (key, value) in top {
        let Some(key) = key else { continue };
        match value {
            Value::Word(word) => {
                keys.insert(key, word);
            }
            Value::Table(items) if key.starts_with("ToneCurve") && !key.ends_with("Name") => {
                // The points flat, x then y, on 0..255.
                let numbers: Vec<f32> = items
                    .iter()
                    .filter_map(|(_, v)| match v {
                        Value::Word(w) => w.parse::<f32>().ok(),
                        Value::Table(_) => None,
                    })
                    .collect();
                let points = numbers
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|[x, y]| [x / 255.0, y / 255.0])
                    .collect();
                curves.insert(key, points);
            }
            Value::Table(items) if !items.is_empty() => {
                keys.insert(key, String::new());
            }
            Value::Table(_) => {}
        }
    }
    if keys.is_empty() && curves.is_empty() {
        return Err(Error::Preset("no Camera Raw settings in the table".into()));
    }
    Ok(map(name.to_string(), &keys, &curves))
}

/// One value of a serialized table: a word as written (a string's
/// text, a number, `true`), or a table of its own, each entry under
/// its key or, for a list, none.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Word(String),
    Table(Vec<(Option<String>, Value)>),
}

/// The entries of a Lua table as Lightroom writes one, `name = { ...
/// }` or the bare table: keys as names or `["quoted"]`, values strings,
/// numbers, booleans, `nil` and tables. Nothing else of Lua is read.
pub fn parse_table(text: &str) -> std::result::Result<Vec<(Option<String>, Value)>, String> {
    let mut p = Lua {
        s: text.as_bytes(),
        at: 0,
        depth: 0,
    };
    p.space();
    // `s = ` before the table, when there is one.
    let mark = p.at;
    if p.name().is_some() {
        p.space();
        if !p.eat(b'=') {
            p.at = mark;
        }
        p.space();
    }
    let Value::Table(items) = p.value()? else {
        return Err("the settings are not a table".into());
    };
    p.space();
    if p.at < p.s.len() {
        return Err(format!("text after the table, at byte {}", p.at));
    }
    Ok(items)
}

/// How deep tables may nest: Lightroom's go five or six deep, and a
/// damaged or hostile text nested further would run the stack out.
const DEEPEST: usize = 64;

struct Lua<'a> {
    s: &'a [u8],
    at: usize,
    /// The tables open around the reader.
    depth: usize,
}

impl Lua<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        let ate = self.peek() == Some(c);
        if ate {
            self.at += 1;
        }
        ate
    }

    fn space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn name(&mut self) -> Option<String> {
        let start = self.at;
        if !self
            .peek()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        {
            return None;
        }
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.at += 1;
        }
        Some(String::from_utf8_lossy(&self.s[start..self.at]).into_owned())
    }

    fn value(&mut self) -> std::result::Result<Value, String> {
        self.space();
        match self.peek() {
            Some(b'{') => self.table(),
            Some(q @ (b'"' | b'\'')) => {
                self.at += 1;
                self.quoted(q).map(Value::Word)
            }
            Some(b'[') if matches!(self.s.get(self.at + 1), Some(b'[' | b'=')) => {
                self.long().map(Value::Word)
            }
            Some(c) if c == b'-' || c == b'.' || c == b'+' || c.is_ascii_alphanumeric() => {
                let start = self.at;
                while self.peek().is_some_and(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'+' | b'_')
                }) {
                    self.at += 1;
                }
                Ok(Value::Word(
                    String::from_utf8_lossy(&self.s[start..self.at]).into_owned(),
                ))
            }
            Some(c) => Err(format!("{:?} at byte {}", c as char, self.at)),
            None => Err("the text ends inside the table".into()),
        }
    }

    fn table(&mut self) -> std::result::Result<Value, String> {
        if self.depth == DEEPEST {
            return Err(format!("tables nested past {DEEPEST}, at byte {}", self.at));
        }
        self.depth += 1;
        let read = self.entries();
        self.depth -= 1;
        read
    }

    /// A table's entries, at its opening brace.
    fn entries(&mut self) -> std::result::Result<Value, String> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.space();
            if self.eat(b'}') {
                return Ok(Value::Table(items));
            }
            // `name = value`, `["key"] = value`, or a list's value.
            let mark = self.at;
            let key = if self.peek() == Some(b'[')
                && matches!(self.s.get(self.at + 1), Some(b'"' | b'\''))
            {
                self.at += 1;
                let q = self.s[self.at];
                self.at += 1;
                let key = self.quoted(q)?;
                self.space();
                if !self.eat(b']') {
                    return Err(format!("no ] after a key, at byte {}", self.at));
                }
                self.space();
                if !self.eat(b'=') {
                    return Err(format!("no = after a key, at byte {}", self.at));
                }
                Some(key)
            } else if let Some(name) = self.name() {
                self.space();
                if self.eat(b'=') {
                    Some(name)
                } else {
                    // A bare word in a list: `true`, say.
                    self.at = mark;
                    None
                }
            } else {
                None
            };
            let value = self.value()?;
            items.push((key, value));
            self.space();
            if !self.eat(b',') && !self.eat(b';') {
                self.space();
                if !self.eat(b'}') {
                    return Err(format!("no , or }} at byte {}", self.at));
                }
                return Ok(Value::Table(items));
            }
        }
    }

    /// A quoted string's text, the opening quote read already.
    fn quoted(&mut self, q: u8) -> std::result::Result<String, String> {
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err("a string runs to the end".into());
            };
            self.at += 1;
            match c {
                c if c == q => break,
                b'\\' => {
                    let Some(e) = self.peek() else {
                        return Err("a string runs to the end".into());
                    };
                    self.at += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'0'..=b'9' => {
                            // Up to three digits, a byte.
                            let mut n = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'9') => {
                                        n = n * 10 + u32::from(d - b'0');
                                        self.at += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(n.min(255) as u8);
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// A long string, `[[...]]` or `[==[...]==]`.
    fn long(&mut self) -> std::result::Result<String, String> {
        self.at += 1;
        let mut level = 0;
        while self.eat(b'=') {
            level += 1;
        }
        if !self.eat(b'[') {
            return Err(format!("a [ that opens nothing, at byte {}", self.at));
        }
        let close: Vec<u8> = std::iter::once(b']')
            .chain(std::iter::repeat_n(b'=', level))
            .chain(std::iter::once(b']'))
            .collect();
        let start = self.at;
        let len = self.s[start..]
            .windows(close.len())
            .position(|w| w == close.as_slice())
            .ok_or("a long string runs to the end")?;
        self.at = start + len + close.len();
        // A newline straight after the opening is not part of it.
        let body = &self.s[start..start + len];
        let body = body.strip_prefix(b"\n").unwrap_or(body);
        Ok(String::from_utf8_lossy(body).into_owned())
    }
}

/// The `x-default` (or the first) text of an `rdf:Alt`.
fn alt_text<'a>(node: &roxmltree::Node<'a, 'a>) -> Option<&'a str> {
    let items: Vec<_> = node
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "li")
        .collect();
    items
        .iter()
        .find(|n| {
            n.attribute(("http://www.w3.org/XML/1998/namespace", "lang")) == Some("x-default")
        })
        .or(items.first())
        .and_then(|n| n.text())
        .map(str::trim)
}

/// The points of an `rdf:Seq` of "x, y" pairs on 0..255.
fn seq_points(node: &roxmltree::Node) -> Vec<Point> {
    node.descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "li")
        .filter_map(|n| n.text())
        .filter_map(|t| {
            let (x, y) = t.split_once(',')?;
            Some([
                x.trim().parse::<f32>().ok()? / 255.0,
                y.trim().parse::<f32>().ok()? / 255.0,
            ])
        })
        .collect()
}

/// The settings, mapped.
fn map(
    name: String,
    keys: &BTreeMap<String, String>,
    curves: &BTreeMap<String, Vec<Point>>,
) -> Imported {
    let get = |k: &str| keys.get(k).map(String::as_str);
    let num = |k: &str| get(k).and_then(|v| v.trim().parse::<f32>().ok());
    // A key in either the current or the 2010 process's spelling.
    let either = |new: &str, old: &str| num(new).or_else(|| num(old));
    let mut edit = Edit::default();
    let mut sections = Vec::new();
    let mut unmapped = Vec::new();

    // Light: exposure in stops as it is, the rest scaled onto the
    // tone curve's shifts.
    let exposure = either("Exposure2012", "Exposure");
    let contrast = either("Contrast2012", "Contrast");
    let highlights = num("Highlights2012");
    let shadows = num("Shadows2012");
    let whites = num("Whites2012");
    let blacks = either("Blacks2012", "Blacks");
    if [exposure, contrast, highlights, shadows, whites, blacks]
        .iter()
        .any(Option::is_some)
    {
        sections.push(Section::Light);
        edit.light.exposure = exposure.unwrap_or(0.0).clamp(-5.0, 5.0);
        edit.light.tone.contrast = (1.0 + contrast.unwrap_or(0.0) / 100.0 * 0.5).clamp(0.5, 2.0);
        // Lightroom's ±100 is about the slider's ±2 at the top (§146).
        edit.light.tone.highlights = (highlights.unwrap_or(0.0) / 100.0 * 2.0).clamp(-2.0, 2.0);
        edit.light.tone.shadows = (shadows.unwrap_or(0.0) / 100.0).clamp(-2.0, 2.0);
        edit.light.tone.whites = (whites.unwrap_or(0.0) / 100.0 * 0.75).clamp(-2.0, 2.0);
        // Lightroom's ±100 is the slider's ±0.3 both ways, measured (§143).
        edit.light.tone.blacks = (blacks.unwrap_or(0.0) / 100.0 * 0.3).clamp(-0.3, 0.3);
    }

    // White balance: a named one at its kelvin, a custom one as it
    // is; a JPEG's offsets (small numbers) have no kelvin to give.
    if let Some(wb) = get("WhiteBalance") {
        let named = match wb {
            "As Shot" | "Auto" => Some(WhiteBalance::AsShot),
            "Daylight" | "Flash" => Some(custom(5500.0, 0.0)),
            "Cloudy" => Some(custom(6500.0, 0.0)),
            "Shade" => Some(custom(7500.0, 0.0)),
            "Tungsten" => Some(custom(2850.0, 0.0)),
            "Fluorescent" => Some(custom(3800.0, 0.0)),
            _ => None,
        };
        let chosen = named.or_else(|| {
            let temperature = num("Temperature")?;
            (1000.0..=50000.0).contains(&temperature).then(|| {
                // Lightroom's tint names the light, as ours does: a
                // positive tint is a green white point that renders
                // magenta. The DNG SDK's `dng_temperature` sets it at
                // about 3000 a unit of Duv.
                let tint = num("Tint").unwrap_or(0.0) / 3000.0;
                custom(
                    temperature.clamp(2000.0, 12000.0) as f64,
                    f64::from(tint.clamp(-0.05, 0.05)),
                )
            })
        });
        match chosen {
            Some(w) => {
                sections.push(Section::WhiteBalance);
                edit.white_balance = w;
            }
            None => unmapped.push("WhiteBalance".into()),
        }
    }

    // The point curves, 0..255 both ways.
    let curve = |names: [&str; 2]| {
        names
            .iter()
            .find_map(|n| curves.get(*n))
            .filter(|p| p.len() >= 2)
            .cloned()
    };
    let rgb = curve(["ToneCurvePV2012", "ToneCurve"]);
    let red = curve(["ToneCurvePV2012Red", "ToneCurveRed"]);
    let green = curve(["ToneCurvePV2012Green", "ToneCurveGreen"]);
    let blue = curve(["ToneCurvePV2012Blue", "ToneCurveBlue"]);
    // The parametric curve: the amounts ±100, the splits 0 to 100
    // along the axis.
    let amount = |k: &str| num(k).map(|v| (v / 100.0).clamp(-1.0, 1.0));
    let split = |k: &str| num(k).map(|v| (v / 100.0).clamp(0.0, 1.0));
    let parametric = [
        amount("ParametricHighlights"),
        amount("ParametricLights"),
        amount("ParametricDarks"),
        amount("ParametricShadows"),
        split("ParametricShadowSplit"),
        split("ParametricMidtoneSplit"),
        split("ParametricHighlightSplit"),
    ];
    if rgb.is_some()
        || red.is_some()
        || green.is_some()
        || blue.is_some()
        || parametric.iter().any(Option::is_some)
    {
        sections.push(Section::Curves);
        let p = &mut edit.curves.parametric;
        let [highlights, lights, darks, shadows, s0, s1, s2] = parametric;
        p.highlights = highlights.unwrap_or(0.0);
        p.lights = lights.unwrap_or(0.0);
        p.darks = darks.unwrap_or(0.0);
        p.shadows = shadows.unwrap_or(0.0);
        p.splits = [
            s0.unwrap_or(p.splits[0]),
            s1.unwrap_or(p.splits[1]),
            s2.unwrap_or(p.splits[2]),
        ];
        p.splits = p.ordered_splits();
        if let Some(p) = rgb {
            edit.curves.rgb = p;
        }
        if let Some(p) = red {
            edit.curves.red = p;
        }
        if let Some(p) = green {
            edit.curves.green = p;
        }
        if let Some(p) = blue {
            edit.curves.blue = p;
        }
    }

    // The mixer: Lightroom's eight bands are these eight.
    const LR_BANDS: [&str; BANDS] = [
        "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
    ];
    let grey = get("ConvertToGrayscale").is_some_and(|v| v.eq_ignore_ascii_case("true"))
        || get("Treatment").is_some_and(|v| v.eq_ignore_ascii_case("Black & White"));
    let mut mixer_touched = false;
    for (i, band) in LR_BANDS.iter().enumerate() {
        if let Some(h) = num(&format!("HueAdjustment{band}")) {
            edit.mixer.hue[i] = (h / 100.0 * 30.0).clamp(-30.0, 30.0);
            mixer_touched = true;
        }
        if let Some(s) = num(&format!("SaturationAdjustment{band}")) {
            edit.mixer.saturation[i] = (s / 100.0).clamp(-1.0, 1.0);
            mixer_touched = true;
        }
        if let Some(l) = num(&format!("LuminanceAdjustment{band}")) {
            edit.mixer.luminance[i] = (l / 100.0).clamp(-1.0, 1.0);
            mixer_touched = true;
        }
        // GrayMixerRed..GrayMixerMagenta are the black and white
        // section's eight weights, band for band, ±100 to ±1. Only
        // when the conversion is asked for: Lightroom leaves the grey
        // mix in a color preset's file, and a preset that carried
        // the section with its switch off would turn a mono picture
        // back to color when laid over it.
        if grey && let Some(g) = num(&format!("GrayMixer{band}")) {
            edit.bw.weights[i] = (g / 100.0).clamp(-1.0, 1.0);
        }
    }
    if mixer_touched {
        sections.push(Section::Mixer);
    }
    if grey {
        sections.push(Section::BlackWhite);
        edit.bw.enabled = true;
    }

    // Vibrance and Saturation land on the global color, not the
    // mixer's bands.
    let vibrance = num("Vibrance");
    let saturation = num("Saturation");
    if vibrance.is_some() || saturation.is_some() {
        sections.push(Section::Color);
        edit.color.vibrance = (vibrance.unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0);
        edit.color.saturation = (saturation.unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0);
    }

    // Color grading: the split toning keys are the shadows' and the
    // highlights' wheels, the color grade's the mid-tones'.
    let wheel = |hue: &str, sat: &str| -> Option<Wheel> {
        let h = num(hue);
        let s = num(sat);
        let saturation = (s.unwrap_or(0.0) / 100.0).clamp(0.0, 1.0);
        (h.is_some() || s.is_some()).then(|| {
            // A wheel at nothing is the default, whatever its hue.
            if saturation == 0.0 {
                Wheel::default()
            } else {
                Wheel {
                    hue: oklab_hue(h.unwrap_or(0.0)),
                    saturation,
                }
            }
        })
    };
    let shadows = wheel("SplitToningShadowHue", "SplitToningShadowSaturation")
        .or_else(|| wheel("ColorGradeShadowHue", "ColorGradeShadowSat"));
    let midtones = wheel("ColorGradeMidtoneHue", "ColorGradeMidtoneSat");
    let highlights = wheel("SplitToningHighlightHue", "SplitToningHighlightSaturation")
        .or_else(|| wheel("ColorGradeHighlightHue", "ColorGradeHighlightSat"));
    let balance = num("SplitToningBalance");
    if shadows.is_some() || midtones.is_some() || highlights.is_some() || balance.is_some() {
        sections.push(Section::Grading);
        edit.grading.shadows = shadows.unwrap_or_default();
        edit.grading.midtones = midtones.unwrap_or_default();
        edit.grading.highlights = highlights.unwrap_or_default();
        edit.grading.balance = (balance.unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0);
    }

    // Texture and Clarity: ±100 onto ±1 of the local contrast, whose
    // scales are chosen to land about where Lightroom's do.
    // `Clarity2012` is the current one; the older `Clarity` stands in
    // for a file that has only that. Dehaze: ±100 onto ±1, the same
    // dark channel idea at about the same reach. The three are the
    // Detail section.
    let texture = num("Texture");
    let clarity = num("Clarity2012").or_else(|| num("Clarity"));
    let dehaze = num("Dehaze");
    if texture.is_some() || clarity.is_some() || dehaze.is_some() {
        sections.push(Section::Detail);
        edit.detail.texture = (texture.unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0);
        edit.detail.clarity = (clarity.unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0);
        edit.detail.dehaze = (dehaze.unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0);
    }

    // Sharpening: Lightroom's is an unsharp mask, this a deconvolution
    // with its own radius; the switch is what crosses.
    if let Some(amount) = num("Sharpness") {
        sections.push(Section::Sharpen);
        edit.sharpen.enabled = amount > 0.0;
    }

    // Noise: the luminance slider onto the profiled denoiser.
    if let Some(l) = num("LuminanceSmoothing") {
        sections.push(Section::Noise);
        edit.noise.profiled = l > 0.0;
        if l > 0.0 {
            edit.noise.strength = (l / 100.0 * 2.0).clamp(0.4, 2.0);
        }
    }

    // The post-crop vignette; the lens vignette is the profile's.
    let v_amount = num("PostCropVignetteAmount");
    if v_amount.is_some() {
        sections.push(Section::Vignette);
        edit.vignette.amount = (v_amount.unwrap_or(0.0) / 100.0 * 2.0).clamp(-2.0, 2.0);
        edit.vignette.midpoint =
            (num("PostCropVignetteMidpoint").unwrap_or(50.0) / 100.0).clamp(0.0, 1.0);
        edit.vignette.feather =
            (num("PostCropVignetteFeather").unwrap_or(50.0) / 100.0).clamp(0.0, 1.0);
        edit.vignette.roundness =
            (num("PostCropVignetteRoundness").unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0);
    }

    // Grain: Lightroom's size 25 is about this engine's default cell.
    if let Some(amount) = num("GrainAmount") {
        sections.push(Section::Grain);
        edit.grain.amount = (amount / 100.0).clamp(0.0, 1.0);
        edit.grain.size = (num("GrainSize").unwrap_or(25.0) / 50.0).clamp(0.1, 2.0);
    }

    // The lens: the profile's switch and the manual distortion, whose
    // positive corrects a barrel there and whose negative does here.
    let profile = num("LensProfileEnable");
    let manual = num("LensManualDistortionAmount");
    if profile.is_some() || manual.is_some() {
        sections.push(Section::Lens);
        if let Some(p) = profile {
            edit.lens.profile = p != 0.0;
        }
        edit.lens.manual = (-manual.unwrap_or(0.0) / 100.0 * 0.2).clamp(-0.5, 0.5);
    }

    // What was set and has no place here.
    const NO_PLACE: &[&str] = &[
        "ColorNoiseReduction",
        "DefringePurpleAmount",
        "DefringeGreenAmount",
        "ColorGradeGlobalSat",
        "RedHue",
        "RedSaturation",
        "GreenHue",
        "GreenSaturation",
        "BlueHue",
        "BlueSaturation",
        "ShadowTint",
        "PerspectiveUpright",
        "PerspectiveVertical",
        "PerspectiveHorizontal",
        "PerspectiveRotate",
    ];
    for k in NO_PLACE {
        if num(k).is_some_and(|v| v != 0.0) {
            unmapped.push(k.to_string());
        }
    }
    // The crop is the frame's, not a preset's. An XMP says so with
    // `HasCrop`; a catalog's table has the edges only when cropped.
    let cropped = get("HasCrop").is_some_and(|v| v.eq_ignore_ascii_case("true"))
        || num("CropAngle").is_some_and(|a| a != 0.0)
        || num("CropLeft").is_some_and(|v| v > 0.0)
        || num("CropTop").is_some_and(|v| v > 0.0)
        || num("CropRight").is_some_and(|v| v < 1.0)
        || num("CropBottom").is_some_and(|v| v < 1.0);
    if cropped {
        unmapped.push("crop".into());
    }
    // A catalog's tables of their own, named when they hold anything.
    for (k, what) in [
        ("RetouchAreas", "spots"),
        ("FilterList", "Enhance"),
        ("LensBlur", "lens blur"),
    ] {
        if keys.contains_key(k) {
            unmapped.push(what.into());
        }
    }
    if let Some(look) = get("CameraProfile").filter(|p| !p.starts_with("Adobe") && !p.is_empty()) {
        unmapped.push(format!("CameraProfile {look}"));
    }
    if keys
        .keys()
        .any(|k| k.starts_with("MaskGroupBasedCorrections"))
        || keys
            .keys()
            .any(|k| k.starts_with("CircularGradientBasedCorrections"))
        || keys
            .keys()
            .any(|k| k.starts_with("GradientBasedCorrections"))
        || keys.keys().any(|k| k.starts_with("PaintBasedCorrections"))
    {
        unmapped.push("masks".into());
    }

    let preset = Preset::from_edit(&name, &edit, &sections);
    Imported { preset, unmapped }
}

fn custom(temperature: f64, tint: f64) -> WhiteBalance {
    WhiteBalance::Custom { temperature, tint }
}

/// The Oklab hue, in degrees, of a Lightroom hue: an HSL hue angle
/// taken as the fully saturated sRGB color of that angle.
pub fn oklab_hue(hsl_degrees: f32) -> f32 {
    let h = hsl_degrees.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let lin = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(r), lin(g), lin(b));
    // Björn Ottosson's Oklab, from linear sRGB.
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let bb = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    bb.atan2(a).to_degrees().rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::Section;

    const SAMPLE: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:Version="15.0"
   crs:ProcessVersion="11.0"
   crs:WhiteBalance="Custom"
   crs:Temperature="4800"
   crs:Tint="+15"
   crs:Exposure2012="+0.50"
   crs:Contrast2012="+20"
   crs:Highlights2012="-40"
   crs:Shadows2012="+30"
   crs:Whites2012="+10"
   crs:Blacks2012="-25"
   crs:Texture="+10"
   crs:Clarity2012="-35"
   crs:Dehaze="0"
   crs:Vibrance="+20"
   crs:Saturation="-10"
   crs:HueAdjustmentRed="0"
   crs:HueAdjustmentOrange="-10"
   crs:SaturationAdjustmentBlue="+30"
   crs:LuminanceAdjustmentGreen="-50"
   crs:SplitToningShadowHue="220"
   crs:SplitToningShadowSaturation="20"
   crs:SplitToningHighlightHue="45"
   crs:SplitToningHighlightSaturation="10"
   crs:SplitToningBalance="+25"
   crs:ColorGradeMidtoneHue="0"
   crs:ColorGradeMidtoneSat="0"
   crs:Sharpness="40"
   crs:LuminanceSmoothing="25"
   crs:ColorNoiseReduction="25"
   crs:PostCropVignetteAmount="-30"
   crs:PostCropVignetteMidpoint="40"
   crs:PostCropVignetteFeather="60"
   crs:PostCropVignetteRoundness="+20"
   crs:GrainAmount="30"
   crs:GrainSize="40"
   crs:LensProfileEnable="1"
   crs:LensManualDistortionAmount="+10"
   crs:ToneCurveName2012="Custom"
   crs:ParametricShadows="-15"
   crs:ParametricDarks="0"
   crs:ParametricLights="+20"
   crs:ParametricHighlights="-30"
   crs:ParametricShadowSplit="20"
   crs:ParametricMidtoneSplit="50"
   crs:ParametricHighlightSplit="80"
   crs:CameraProfile="Adobe Color"
   crs:HasSettings="True">
   <crs:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Warm Film</rdf:li>
    </rdf:Alt>
   </crs:Name>
   <crs:Group>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Mine</rdf:li>
    </rdf:Alt>
   </crs:Group>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 10</rdf:li>
     <rdf:li>128, 120</rdf:li>
     <rdf:li>255, 245</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
   <crs:ToneCurvePV2012Red>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012Red>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn a_lightroom_preset_comes_across() {
        let Imported { preset, unmapped } = read(SAMPLE, "file").unwrap();
        assert_eq!(preset.name, "Warm Film");
        assert_eq!(
            preset.sections,
            vec![
                Section::WhiteBalance,
                Section::Light,
                Section::Color,
                Section::Curves,
                Section::Mixer,
                Section::Grading,
                Section::Noise,
                Section::Lens,
                Section::Detail,
                Section::Sharpen,
                Section::Vignette,
                Section::Grain,
            ]
        );
        let e = &preset.edit;
        assert_eq!(e.light.exposure, 0.5);
        assert!((e.light.tone.contrast - 1.1).abs() < 1e-6);
        assert!((e.light.tone.highlights + 0.8).abs() < 1e-6);
        assert!((e.light.tone.shadows - 0.3).abs() < 1e-6);
        assert!((e.light.tone.whites - 0.075).abs() < 1e-6);
        assert!((e.light.tone.blacks + 0.075).abs() < 1e-6);
        match e.white_balance {
            WhiteBalance::Custom { temperature, tint } => {
                assert_eq!(temperature, 4800.0);
                assert!((tint - 0.005).abs() < 1e-6, "a green light is positive Duv");
            }
            _ => panic!("custom"),
        }
        assert_eq!(e.curves.rgb.len(), 3);
        assert!((e.curves.rgb[1][0] - 128.0 / 255.0).abs() < 1e-6);
        assert!((e.curves.rgb[2][1] - 245.0 / 255.0).abs() < 1e-6);
        assert_eq!(e.curves.green, crate::curve::identity());
        let p = &e.curves.parametric;
        assert!((p.shadows + 0.15).abs() < 1e-6);
        assert_eq!(p.darks, 0.0);
        assert!((p.lights - 0.2).abs() < 1e-6);
        assert!((p.highlights + 0.3).abs() < 1e-6);
        assert_eq!(p.splits, [0.2, 0.5, 0.8]);
        // Orange's hue, blue's saturation band alone, green's
        // luminance; vibrance and saturation land on the color instead.
        assert!((e.mixer.hue[1] + 3.0).abs() < 1e-6);
        assert!((e.mixer.saturation[5] - 0.3).abs() < 1e-6);
        assert!((e.mixer.saturation[0] - 0.0).abs() < 1e-6);
        assert!((e.mixer.luminance[3] + 0.5).abs() < 1e-6);
        assert!((e.color.vibrance - 0.2).abs() < 1e-6);
        assert!((e.color.saturation + 0.1).abs() < 1e-6);
        assert!((e.grading.shadows.saturation - 0.2).abs() < 1e-6);
        assert!((e.grading.highlights.saturation - 0.1).abs() < 1e-6);
        assert_eq!(e.grading.midtones, Wheel::default());
        assert!((e.grading.balance - 0.25).abs() < 1e-6);
        // Hue 220 is a blue; 45 an orange.
        assert!(
            (240.0..290.0).contains(&e.grading.shadows.hue),
            "{}",
            e.grading.shadows.hue
        );
        assert!(
            (40.0..90.0).contains(&e.grading.highlights.hue),
            "{}",
            e.grading.highlights.hue
        );
        assert!(e.detail.enabled);
        assert!((e.detail.texture - 0.1).abs() < 1e-6);
        assert!((e.detail.clarity + 0.35).abs() < 1e-6);
        assert!(e.sharpen.enabled);
        assert!(e.noise.profiled);
        assert!((e.noise.strength - 0.5).abs() < 1e-6);
        assert!((e.vignette.amount + 0.6).abs() < 1e-6);
        assert!((e.vignette.midpoint - 0.4).abs() < 1e-6);
        assert!((e.vignette.feather - 0.6).abs() < 1e-6);
        assert!((e.vignette.roundness - 0.2).abs() < 1e-6);
        assert!((e.grain.amount - 0.3).abs() < 1e-6);
        assert!((e.grain.size - 0.8).abs() < 1e-6);
        assert!(e.lens.profile);
        assert!((e.lens.manual + 0.02).abs() < 1e-6);
        assert_eq!(unmapped, vec!["ColorNoiseReduction"]);
        // Not touched: the demosaic and the geometry are not a preset's.
        assert_eq!(e.demosaic, crate::Demosaic::default());
        assert_eq!(e.geometry, crate::Geometry::default());
        // And it writes and reads as a preset of this engine's.
        assert_eq!(Preset::from_json(&preset.to_json()).unwrap(), preset);
    }

    #[test]
    fn a_black_and_white_preset_turns_the_section_on() {
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
            crs:ConvertToGrayscale="True" crs:GrayMixerRed="+40" crs:GrayMixerBlue="-60"
            crs:Exposure2012="0.00"/></rdf:RDF></x:xmpmeta>"#;
        let Imported { preset, .. } = read(xml, "Mono").unwrap();
        assert_eq!(preset.name, "Mono", "the fallback when the file gives none");
        assert_eq!(preset.sections, vec![Section::Light, Section::BlackWhite]);
        // The mixer is left alone: the grey is the section's now.
        assert_eq!(preset.edit.mixer, crate::Mixer::default());
        assert!(preset.edit.bw.enabled);
        assert!((preset.edit.bw.weights[0] - 0.4).abs() < 1e-6);
        assert!((preset.edit.bw.weights[5] + 0.6).abs() < 1e-6);
        // The same keys without the conversion asked for are a color
        // preset with a stale grey mix in the file: the section is not
        // carried at all, so laying it over a mono picture leaves that
        // picture mono.
        let color = xml.replace(r#"crs:ConvertToGrayscale="True""#, "");
        let Imported { preset, .. } = read(&color, "Color").unwrap();
        assert_eq!(preset.sections, vec![Section::Light]);
        assert_eq!(preset.edit.bw, crate::BlackWhite::OFF);
    }

    #[test]
    fn keys_as_elements_and_named_whites_read_too() {
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
            <crs:WhiteBalance>Tungsten</crs:WhiteBalance>
            <crs:Exposure>-1.0</crs:Exposure>
            <crs:Sharpness>0</crs:Sharpness>
          </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let Imported { preset, unmapped } = read(xml, "x").unwrap();
        assert!(
            matches!(preset.edit.white_balance, WhiteBalance::Custom { temperature, .. } if temperature == 2850.0)
        );
        assert_eq!(preset.edit.light.exposure, -1.0);
        assert!(!preset.edit.sharpen.enabled);
        assert!(unmapped.is_empty());
        // A JPEG preset's white is an offset with no kelvin in it.
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
            crs:WhiteBalance="Custom" crs:Temperature="+12" crs:Tint="0"/></rdf:RDF></x:xmpmeta>"#;
        let Imported { preset, unmapped } = read(xml, "x").unwrap();
        assert!(preset.sections.is_empty());
        assert_eq!(unmapped, vec!["WhiteBalance"]);
    }

    /// A preset's masks and spots are structures in its XMP, not
    /// values: named among what did not come across, as a catalog's
    /// are, and an empty list of them is nothing to name.
    #[test]
    fn an_xmp_presets_masks_and_spots_are_named() {
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+0.5">
            <crs:MaskGroupBasedCorrections>
              <rdf:Seq><rdf:li rdf:parseType="Resource"><crs:What>Correction</crs:What></rdf:li></rdf:Seq>
            </crs:MaskGroupBasedCorrections>
            <crs:RetouchAreas><rdf:Seq><rdf:li>x</rdf:li></rdf:Seq></crs:RetouchAreas>
            <crs:PaintBasedCorrections><rdf:Seq/></crs:PaintBasedCorrections>
          </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let Imported { preset, unmapped } = read(xml, "x").unwrap();
        assert_eq!(preset.edit.light.exposure, 0.5);
        assert_eq!(unmapped, vec!["spots", "masks"]);
        // Only an empty list: nothing.
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+0.5">
            <crs:PaintBasedCorrections><rdf:Seq/></crs:PaintBasedCorrections>
          </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        assert!(read(xml, "x").unwrap().unmapped.is_empty());
    }

    #[test]
    fn a_catalog_table_reads_as_lightroom_writes_it() {
        let text = "s = { Name = \"x\",\n\
            Quoted = \"a \\\"b\\\"\\n\\065\",\n\
            [\"Odd key\"] = -1.5e-1,\n\
            Long = [==[one]]\ntwo]==],\n\
            Flag = true,\n\
            Nothing = nil,\n\
            List = { 1, \"two\", { Deep = false } },\n\
            Empty = {  },\n\
            }\n";
        let items = parse_table(text).unwrap();
        let get = |k: &str| {
            items
                .iter()
                .find(|(key, _)| key.as_deref() == Some(k))
                .map(|(_, v)| v)
        };
        let word = |w: &str| Value::Word(w.to_string());
        assert_eq!(get("Name"), Some(&word("x")));
        assert_eq!(get("Quoted"), Some(&word("a \"b\"\nA")));
        assert_eq!(get("Odd key"), Some(&word("-1.5e-1")));
        assert_eq!(get("Long"), Some(&word("one]]\ntwo")));
        assert_eq!(get("Flag"), Some(&word("true")));
        assert_eq!(get("Nothing"), Some(&word("nil")));
        assert_eq!(
            get("List"),
            Some(&Value::Table(vec![
                (None, word("1")),
                (None, word("two")),
                (
                    None,
                    Value::Table(vec![(Some("Deep".into()), word("false"))])
                ),
            ]))
        );
        assert_eq!(get("Empty"), Some(&Value::Table(Vec::new())));
        // The bare table reads too; what is not one is refused.
        assert_eq!(parse_table("{ a = 1 }").unwrap().len(), 1);
        assert!(parse_table("s = { a = 1").is_err());
        assert!(parse_table("s = { a = \"open }").is_err());
        assert!(parse_table("s = { a = 1 } extra").is_err());
        assert!(parse_table("s = 3").is_err());
        // Nesting past reason is refused rather than run the stack out.
        let deep = format!("s = {}{}", "{ ".repeat(100_000), "}".repeat(100_000));
        assert!(parse_table(&deep).unwrap_err().contains("nested"));
    }

    #[test]
    fn a_catalog_table_maps_as_the_xmp_does_and_names_its_tables() {
        let text = "s = { Exposure2012 = 0.5,\n\
            Contrast2012 = 20,\n\
            WhiteBalance = \"Custom\",\n\
            Temperature = 4800,\n\
            Tint = 15,\n\
            ConvertToGrayscale = false,\n\
            CropLeft = 0.05,\n\
            CropAngle = 1.2,\n\
            ToneCurvePV2012 = { 0,\n10,\n128,\n120,\n255,\n245 },\n\
            MaskGroupBasedCorrections = { { What = \"Correction\" } },\n\
            RetouchAreas = { { SpotType = \"heal\" } },\n\
            FilterList = { Filters = { { Name = \"Enhance\" } } },\n\
            LensBlur = {  },\n\
            }\n";
        let Imported { preset, unmapped } = read_table(text, "From the catalog").unwrap();
        assert_eq!(preset.name, "From the catalog");
        let e = &preset.edit;
        assert_eq!(e.light.exposure, 0.5);
        assert!((e.light.tone.contrast - 1.1).abs() < 1e-6);
        assert!(
            matches!(e.white_balance, WhiteBalance::Custom { temperature, .. } if temperature == 4800.0)
        );
        assert_eq!(e.curves.rgb.len(), 3);
        assert!((e.curves.rgb[2][1] - 245.0 / 255.0).abs() < 1e-6);
        assert!(!e.bw.enabled);
        // The crop, the masks, the spots and the Enhance named; an empty
        // lens blur is nothing.
        for what in ["crop", "masks", "spots", "Enhance"] {
            assert!(unmapped.iter().any(|u| u == what), "{what}: {unmapped:?}");
        }
        assert!(!unmapped.iter().any(|u| u == "lens blur"));
        assert!(read_table("s = { }", "x").is_err());
        assert!(read_table("not a table", "x").is_err());
    }

    #[test]
    fn a_file_without_settings_is_refused() {
        assert!(read("<a/>", "x").is_err());
        assert!(read("not xml", "x").is_err());
    }

    #[test]
    fn oklab_hues_land_where_the_bands_are() {
        // Red, yellow, green, blue in HSL land near the mixer's red,
        // yellow, green and blue centers in Oklab.
        let near = |h: f32, center: f32| {
            ((oklab_hue(h) - center + 180.0).rem_euclid(360.0) - 180.0).abs() < 20.0
        };
        assert!(near(0.0, 25.0), "{}", oklab_hue(0.0));
        assert!(near(60.0, 100.0), "{}", oklab_hue(60.0));
        assert!(near(120.0, 140.0), "{}", oklab_hue(120.0));
        assert!(near(240.0, 265.0), "{}", oklab_hue(240.0));
        assert!(near(360.0, 25.0));
    }
}
