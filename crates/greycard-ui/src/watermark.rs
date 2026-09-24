//! The watermark: a line of text or a picture laid over an export,
//! after its resize and before its encode, never on the viewport.
//!
//! Its size is a fraction of the export's long edge, and so is its
//! margin, so a mark lands in the same place at the same proportion on
//! a 2048 px export as on a full-size one. It is composited in the
//! output's encoded values, the sRGB curve and all, as every other
//! editor and every browser composites: a white mark at half opacity
//! comes out half-way between the picture and white in the numbers the
//! file holds, which is what "50 %" looks like to the eye.
//!
//! Text is set in the system's sans-serif, found through fontique (the
//! font discovery Slint's own text already carries: fontconfig on
//! Linux, Core Text on the Mac, DirectWrite on Windows) and rasterized
//! with ab_glyph; both were in the tree before this.

use crate::export::Space;
use ab_glyph::{Font, FontRef, ScaleFont};
use anyhow::{Context, Result, bail};
use greycard_core::RgbSpace;
use greycard_core::color::{CAT, srgb_decode, srgb_encode};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// Where on the picture the mark sits: a corner, the middle of an
/// edge, or the center.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

impl Position {
    /// Row by row, as the sheet's grid lays them out.
    pub const ALL: [Position; 9] = [
        Position::TopLeft,
        Position::Top,
        Position::TopRight,
        Position::Left,
        Position::Center,
        Position::Right,
        Position::BottomLeft,
        Position::Bottom,
        Position::BottomRight,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Position::TopLeft => "Top left",
            Position::Top => "Top",
            Position::TopRight => "Top right",
            Position::Left => "Left",
            Position::Center => "Center",
            Position::Right => "Right",
            Position::BottomLeft => "Bottom left",
            Position::Bottom => "Bottom",
            Position::BottomRight => "Bottom right",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(name))
    }

    /// Column and row, 0 to 2 each.
    fn cell(self) -> (u8, u8) {
        let i = Self::ALL.iter().position(|p| *p == self).unwrap_or(8) as u8;
        (i % 3, i / 3)
    }
}

/// What the mark is.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A line of text, white or black.
    Text { text: String, white: bool },
    /// A picture, a PNG with its alpha, by path.
    Image { path: PathBuf },
}

/// A watermark as an export asks for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub kind: Kind,
    pub position: Position,
    /// The mark's width over the export's long edge.
    pub size: f32,
    /// The gap to the nearest edges, over the export's long edge.
    pub margin: f32,
    /// 0 to 1.
    pub opacity: f32,
}

/// A mark made for one export: its pixels in the output's encoded
/// values, premultiplied, with the alpha beside them.
#[derive(Debug, Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// RGBA, premultiplied, 0 to 1.
    pub rgba: Vec<f32>,
}

impl Mark {
    /// The mark's width in pixels on an export whose long edge is
    /// `long`: at least one pixel, never more than the edge.
    pub fn width_on(&self, long: u32) -> u32 {
        ((self.size.clamp(0.0, 1.0) * long as f32).round() as u32).clamp(1, long.max(1))
    }

    /// The margin in pixels on an export whose long edge is `long`.
    pub fn margin_on(&self, long: u32) -> u32 {
        (self.margin.clamp(0.0, 0.5) * long as f32).round() as u32
    }

    /// Whether there is anything to draw: text that is not blank, a
    /// PNG chosen and there. A mark asked for with nothing to draw
    /// fails the export rather than let it out unmarked.
    pub fn check(&self) -> Result<()> {
        match &self.kind {
            Kind::Text { text, .. } if text.trim().is_empty() => {
                bail!("the watermark's text is empty")
            }
            Kind::Image { path } if path.as_os_str().is_empty() => {
                bail!("no PNG is chosen for the watermark")
            }
            Kind::Image { path } if !path.is_file() => {
                bail!("the watermark PNG {} is not there", path.display())
            }
            _ => Ok(()),
        }
    }

    /// Draw the mark `target` pixels wide for an export in `space`.
    pub fn raster(&self, target: u32, space: Space) -> Result<Raster> {
        self.check()?;
        match &self.kind {
            Kind::Text { text, white } => {
                let (w, h, coverage) = set_text(text.trim(), target)?;
                // White and black are the same numbers in every space.
                let v = if *white { 1.0 } else { 0.0 };
                let rgba = coverage
                    .iter()
                    .flat_map(|&a| [v * a, v * a, v * a, a])
                    .collect();
                Ok(Raster {
                    width: w,
                    height: h,
                    rgba,
                })
            }
            Kind::Image { path } => {
                let picture = image::open(path)
                    .with_context(|| format!("the watermark {}", path.display()))?;
                Ok(picture_mark(&picture.into_rgba32f(), target, space))
            }
        }
    }

    /// Lay the mark over a finished picture, interleaved RGB whose
    /// channels run 0 to `max`.
    pub fn apply<T: Channel>(
        &self,
        pixels: &mut [T],
        width: u32,
        height: u32,
        space: Space,
    ) -> Result<()> {
        let (raster, at) = self.fitted(width, height, space)?;
        composite(pixels, width, height, &raster, at, self.opacity);
        Ok(())
    }

    /// The mark drawn and placed on a picture `width` by `height`:
    /// its size by the long edge, shrunk to fit inside the margins
    /// when a narrow picture has no room for it (a 1:2 crop with the
    /// size at its limit), and its box kept on the picture.
    pub fn fitted(&self, width: u32, height: u32, space: Space) -> Result<(Raster, (i64, i64))> {
        let long = width.max(height);
        let margin = self.margin_on(long);
        let room = (
            width.saturating_sub(2 * margin).max(1),
            height.saturating_sub(2 * margin).max(1),
        );
        let mut target = self.width_on(long);
        let mut raster = self.raster(target, space)?;
        // Scaled once by how much it is over, then a pixel at a time
        // for the text's rounding.
        for _ in 0..8 {
            if raster.width <= room.0 && raster.height <= room.1 {
                break;
            }
            let over =
                (room.0 as f64 / raster.width as f64).min(room.1 as f64 / raster.height as f64);
            let next = ((target as f64 * over).floor() as u32)
                .min(target.saturating_sub(1))
                .max(1);
            if next == target {
                break;
            }
            target = next;
            raster = self.raster(target, space)?;
        }
        let at = place(
            (width, height),
            (raster.width, raster.height),
            self.position,
            margin,
        );
        let keep = |v: i64, room: u32, size: u32| v.clamp(0, (room as i64 - size as i64).max(0));
        let at = (
            keep(at.0, width, raster.width),
            keep(at.1, height, raster.height),
        );
        Ok((raster, at))
    }
}

/// Where the mark's top-left corner goes: the margin in from the edges
/// its position names, centered along the others. `Mark::fitted`
/// sizes the mark so it fits.
pub fn place(picture: (u32, u32), mark: (u32, u32), position: Position, margin: u32) -> (i64, i64) {
    let axis = |cell: u8, room: u32, size: u32| -> i64 {
        let (room, size, margin) = (room as i64, size as i64, margin as i64);
        match cell {
            0 => margin,
            1 => (room - size) / 2,
            _ => room - margin - size,
        }
    };
    let (col, row) = position.cell();
    (axis(col, picture.0, mark.0), axis(row, picture.1, mark.1))
}

/// A channel of a finished picture.
pub trait Channel: Copy {
    const MAX: f32;
    fn to_f32(self) -> f32;
    fn from_f32(v: f32) -> Self;
}

impl Channel for u8 {
    const MAX: f32 = 255.0;
    fn to_f32(self) -> f32 {
        self as f32
    }
    fn from_f32(v: f32) -> Self {
        v.round().clamp(0.0, 255.0) as u8
    }
}

impl Channel for u16 {
    const MAX: f32 = 65535.0;
    fn to_f32(self) -> f32 {
        self as f32
    }
    fn from_f32(v: f32) -> Self {
        v.round().clamp(0.0, 65535.0) as u16
    }
}

/// `over` on the picture's encoded values: the picture times one less
/// the mark's alpha at `opacity`, plus the mark's premultiplied color
/// at the same opacity.
pub fn composite<T: Channel>(
    pixels: &mut [T],
    width: u32,
    height: u32,
    raster: &Raster,
    at: (i64, i64),
    opacity: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity == 0.0 {
        return;
    }
    let (w, h) = (width as i64, height as i64);
    for my in 0..raster.height as i64 {
        let y = at.1 + my;
        if !(0..h).contains(&y) {
            continue;
        }
        for mx in 0..raster.width as i64 {
            let x = at.0 + mx;
            if !(0..w).contains(&x) {
                continue;
            }
            let m = ((my * raster.width as i64 + mx) * 4) as usize;
            let a = raster.rgba[m + 3] * opacity;
            if a <= 0.0 {
                continue;
            }
            let p = ((y * w + x) * 3) as usize;
            for c in 0..3 {
                let under = pixels[p + c].to_f32();
                let mark = raster.rgba[m + c] * opacity * T::MAX;
                pixels[p + c] = T::from_f32(under * (1.0 - a) + mark);
            }
        }
    }
}

/// A picture mark `target` pixels wide: premultiplied before its
/// resize, so its soft edges keep no fringe of whatever color its
/// clear pixels hold, and its colors taken from sRGB, which is what a
/// PNG without a profile means, into the export's space.
fn picture_mark(picture: &image::Rgba32FImage, target: u32, space: Space) -> Raster {
    let convert = (space != Space::Srgb).then(|| {
        RgbSpace::SRGB
            .to_space_matrix(&space_rgb(space), CAT)
            .expect("sRGB reaches every output space")
            .rows
            .map(|row| row.map(|v| v as f32))
    });
    let mut premultiplied = picture.clone();
    for p in premultiplied.pixels_mut() {
        let a = p.0[3].clamp(0.0, 1.0);
        let mut rgb = [p.0[0], p.0[1], p.0[2]].map(|v| v.clamp(0.0, 1.0));
        if let Some(m) = &convert {
            let lin = rgb.map(srgb_decode);
            rgb = [0, 1, 2].map(|r| {
                srgb_encode(
                    (m[r][0] * lin[0] + m[r][1] * lin[1] + m[r][2] * lin[2]).clamp(0.0, 1.0),
                )
            });
        }
        p.0 = [rgb[0] * a, rgb[1] * a, rgb[2] * a, a];
    }
    let (pw, ph) = premultiplied.dimensions();
    let height = ((target as f64 * ph as f64 / pw.max(1) as f64).round() as u32).max(1);
    let resized = if (pw, ph) == (target, height) {
        premultiplied
    } else {
        image::imageops::resize(
            &premultiplied,
            target,
            height,
            image::imageops::FilterType::Lanczos3,
        )
    };
    // The resize rings a little past the ends; a color is never more
    // than its alpha once premultiplied.
    let mut rgba = resized.into_raw();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3].clamp(0.0, 1.0);
        let [r, g, b, _] = *px;
        *px = [r.clamp(0.0, a), g.clamp(0.0, a), b.clamp(0.0, a), a];
    }
    Raster {
        width: target,
        height,
        rgba,
    }
}

fn space_rgb(space: Space) -> RgbSpace {
    match space {
        Space::Srgb => RgbSpace::SRGB,
        Space::DisplayP3 => RgbSpace::DISPLAY_P3,
        Space::Rec2020 => RgbSpace::REC2020,
    }
}

/// A line's faces, and each character with the index of its face.
pub type Setting = (Vec<Arc<Face>>, Vec<(char, usize)>);

/// A font's file and its face within it.
pub struct Face {
    pub bytes: Vec<u8>,
    pub index: u32,
    /// The family's name, for the log.
    pub family: String,
}

impl Face {
    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::try_from_slice_and_index(&self.bytes, self.index).ok()
    }

    /// Whether the face draws `c` as an outline: it has a glyph for it
    /// that is not the missing-glyph box, and the glyph is not a
    /// bitmap only (a color emoji font's), which ab_glyph cannot draw.
    fn draws(&self, c: char) -> bool {
        self.font().is_some_and(|f| {
            let id = f.glyph_id(c);
            id.0 != 0 && f.outline(id).is_some()
        })
    }
}

/// The system's fonts, opened once: fontconfig's scan is not free.
fn collection() -> &'static Mutex<fontique::Collection> {
    static COLLECTION: OnceLock<Mutex<fontique::Collection>> = OnceLock::new();
    COLLECTION.get_or_init(|| {
        Mutex::new(fontique::Collection::new(fontique::CollectionOptions {
            shared: false,
            system_fonts: true,
        }))
    })
}

/// A family's regular face, loaded, when ab_glyph can read it.
fn load_family(collection: &mut fontique::Collection, id: fontique::FamilyId) -> Option<Face> {
    let family = collection.family(id)?;
    let font = family.default_font()?;
    let bytes = font.load(None)?.as_ref().to_vec();
    let face = Face {
        bytes,
        index: font.index(),
        family: family.name().to_string(),
    };
    face.font().is_some().then_some(face)
}

/// The system's sans-serif, the regular face, found once.
pub fn system_font() -> Result<Arc<Face>> {
    static FONT: OnceLock<Option<Arc<Face>>> = OnceLock::new();
    FONT.get_or_init(|| {
        use fontique::GenericFamily;
        let mut c = collection().lock().unwrap_or_else(|e| e.into_inner());
        let mut ids: Vec<_> = c.generic_families(GenericFamily::SansSerif).collect();
        ids.extend(c.generic_families(GenericFamily::SystemUi));
        let face = ids.into_iter().find_map(|id| load_family(&mut c, id))?;
        tracing::info!("watermark font: {}", face.family);
        Some(Arc::new(face))
    })
    .clone()
    .context("no sans-serif font on this system for the watermark's text")
}

/// The system's fallback for `c`'s script, when it draws `c`: the
/// family fontique's fallback chain (fontconfig, Core Text,
/// DirectWrite) names for the script. Remembered by script.
fn fallback_font(c: char) -> Option<Arc<Face>> {
    use unicode_script::UnicodeScript;
    type ByScript = HashMap<&'static str, Option<Arc<Face>>>;
    static CACHE: OnceLock<Mutex<ByScript>> = OnceLock::new();
    let script = c.script().short_name();
    let cache = CACHE.get_or_init(Default::default);
    let known = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(script)
        .cloned();
    let face = match known {
        Some(face) => face,
        None => {
            let face = {
                let mut col = collection().lock().unwrap_or_else(|e| e.into_inner());
                let key = fontique::Script::from_str_unchecked(script);
                let ids: Vec<_> = col.fallback_families(key).collect();
                ids.into_iter()
                    .find_map(|id| load_family(&mut col, id))
                    .map(Arc::new)
            };
            cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(script, face.clone());
            face
        }
    };
    face.filter(|f| f.draws(c))
}

/// The faces a line of text needs and, for each of its characters,
/// which face draws it: the system's sans-serif where it can, the
/// script's fallback where it cannot. A character no font here draws
/// is an error naming it, rather than a box in the export.
pub fn faces_for(text: &str) -> Result<Setting> {
    let mut faces = vec![system_font()?];
    let mut out = Vec::new();
    for c in text.chars().filter(|c| !c.is_control()) {
        if c.is_whitespace() {
            out.push((c, 0));
            continue;
        }
        if let Some(i) = faces.iter().position(|f| f.draws(c)) {
            out.push((c, i));
            continue;
        }
        let Some(face) = fallback_font(c) else {
            bail!(
                "the watermark's text has {c} (U+{:04X}), which no font on this system can draw",
                c as u32
            );
        };
        tracing::info!("watermark font for {c}: {}", face.family);
        faces.push(face);
        out.push((c, faces.len() - 1));
    }
    Ok((faces, out))
}

/// The text's glyphs laid along one line at `px`, each in its own
/// face, on the first face's baseline; kerning within a face.
fn lay_out(fonts: &[FontRef], chars: &[(char, usize)], px: f32) -> Vec<ab_glyph::OutlinedGlyph> {
    let ascent = fonts[0].as_scaled(px).ascent();
    let mut x = 0.0;
    let mut last: Option<(usize, ab_glyph::GlyphId)> = None;
    let mut out = Vec::new();
    for &(ch, face) in chars {
        let font = &fonts[face];
        let scaled = font.as_scaled(px);
        let id = scaled.glyph_id(ch);
        if let Some((prev_face, prev)) = last
            && prev_face == face
        {
            x += scaled.kern(prev, id);
        }
        let glyph = id.with_scale_and_position(px, ab_glyph::point(x, ascent));
        x += scaled.h_advance(id);
        last = Some((face, id));
        if let Some(outlined) = font.outline_glyph(glyph) {
            out.push(outlined);
        }
    }
    out
}

/// The union of the glyphs' pixel bounds.
fn ink(glyphs: &[ab_glyph::OutlinedGlyph]) -> Option<ab_glyph::Rect> {
    glyphs
        .iter()
        .map(|g| g.px_bounds())
        .reduce(|a, b| ab_glyph::Rect {
            min: ab_glyph::point(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
            max: ab_glyph::point(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
        })
}

/// The text set so its ink is `target` pixels wide: its width and
/// height and its coverage, 0 to 1, row by row.
fn set_text(text: &str, target: u32) -> Result<(u32, u32, Vec<f32>)> {
    let (faces, chars) = faces_for(text)?;
    let fonts: Vec<FontRef> = faces
        .iter()
        .map(|f| f.font().context("a font that read a moment ago"))
        .collect::<Result<_>>()?;
    // Measured at a size, then set at the size that makes it the
    // width asked; outlines scale linearly, and ab_glyph does not
    // hint, so one measure is enough to within the rounding of the
    // pixel bounds.
    const PROBE: f32 = 200.0;
    let probe = ink(&lay_out(&fonts, &chars, PROBE)).context("the watermark's text has no ink")?;
    let probe_width = (probe.max.x - probe.min.x).max(1.0);
    let px = PROBE * target as f32 / probe_width;
    let glyphs = lay_out(&fonts, &chars, px);
    let bounds = ink(&glyphs).context("the watermark's text has no ink")?;
    let (ox, oy) = (bounds.min.x, bounds.min.y);
    let w = (bounds.max.x - ox).round().max(1.0) as u32;
    let h = (bounds.max.y - oy).round().max(1.0) as u32;
    let mut coverage = vec![0f32; (w * h) as usize];
    for g in &glyphs {
        let b = g.px_bounds();
        let (gx, gy) = ((b.min.x - ox) as i64, (b.min.y - oy) as i64);
        g.draw(|x, y, c| {
            let (x, y) = (gx + x as i64, gy + y as i64);
            if (0..w as i64).contains(&x) && (0..h as i64).contains(&y) {
                let i = (y * w as i64 + x) as usize;
                coverage[i] = (coverage[i] + c).min(1.0);
            }
        });
    }
    Ok((w, h, coverage))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A picture `size` high and twice as wide, clear but for a white
    /// bar across its middle half, written as a PNG under `dir`.
    pub(crate) fn bar_png(dir: &std::path::Path, size: u32) -> PathBuf {
        let mut img = image::RgbaImage::new(size * 2, size);
        for (_, y, p) in img.enumerate_pixels_mut() {
            let inside = y >= size / 4 && y < size * 3 / 4;
            p.0 = if inside {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 0]
            };
        }
        let path = dir.join("mark.png");
        img.save(&path).unwrap();
        path
    }

    /// Whether this machine has no font for a text mark, said out loud
    /// so a skip is never a silent pass.
    fn no_font(test: &str) -> bool {
        match system_font() {
            Ok(_) => false,
            Err(e) => {
                eprintln!("skipped {test}: {e}");
                true
            }
        }
    }

    #[test]
    fn a_character_the_font_lacks_comes_from_a_fallback_or_is_refused() {
        if no_font("a_character_the_font_lacks_comes_from_a_fallback_or_is_refused") {
            return;
        }
        // A private-use character past anything a font carries: an
        // error that names it, not a box in the export.
        let Err(err) = faces_for("photo \u{10FFFD}") else {
            panic!("drawn")
        };
        let err = err.to_string();
        assert!(err.contains("U+10FFFD"), "{err}");
        let mark = Mark {
            kind: Kind::Text {
                text: "a \u{10FFFD}".into(),
                white: true,
            },
            position: Position::Center,
            size: 0.2,
            margin: 0.0,
            opacity: 1.0,
        };
        let mut p = flat(200, 100, 0);
        assert!(mark.apply(&mut p, 200, 100, Space::Srgb).is_err());
        // Japanese: the sans-serif here is Latin, so a second face
        // comes from the script's fallback, and every character is
        // drawn by a face that has it.
        match faces_for("写真 photo") {
            Ok((faces, chars)) => {
                assert!(faces.len() >= 2, "{}", faces[0].family);
                for (c, i) in chars {
                    assert!(c.is_whitespace() || faces[i].draws(c), "{c}");
                }
            }
            Err(e) => eprintln!("skipped the fallback half: {e}"),
        }
    }

    #[test]
    fn a_mark_too_big_for_a_narrow_picture_shrinks_inside_its_margins() {
        let dir = std::env::temp_dir().join(format!("greycard-mark-fit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = bar_png(&dir, 40);
        // A 1:3 strip, the mark half the long edge: 600 wide on a
        // picture 200 wide. Every position keeps it inside the margins.
        let (w, h) = (200u32, 600u32);
        for position in Position::ALL {
            let mark = Mark {
                kind: Kind::Image { path: png.clone() },
                position,
                size: 0.5,
                margin: 0.02,
                opacity: 1.0,
            };
            let (raster, at) = mark.fitted(w, h, Space::Srgb).unwrap();
            let m = mark.margin_on(h) as i64;
            assert!(raster.width <= w - 2 * m as u32, "{}", raster.width);
            assert!(
                at.0 >= m && at.0 + raster.width as i64 <= w as i64 - m,
                "{position:?} {at:?}"
            );
            assert!(
                at.1 >= 0 && at.1 + raster.height as i64 <= h as i64,
                "{position:?} {at:?}"
            );
        }
        // Text too, one pixel at a time past its rounding.
        if !no_font("a_mark_too_big_for_a_narrow_picture_shrinks_inside_its_margins, its text half")
        {
            let mark = Mark {
                kind: Kind::Text {
                    text: "© a long line of text".into(),
                    white: true,
                },
                position: Position::Left,
                size: 1.0,
                margin: 0.05,
                opacity: 1.0,
            };
            let (raster, at) = mark.fitted(150, 500, Space::Srgb).unwrap();
            assert!(raster.width <= 150 - 50, "{}", raster.width);
            assert_eq!(at.0, 25);
        }
        // An empty mark is refused, not skipped.
        for kind in [
            Kind::Text {
                text: " ".into(),
                white: true,
            },
            Kind::Image {
                path: PathBuf::new(),
            },
            Kind::Image {
                path: dir.join("gone.png"),
            },
        ] {
            let mark = Mark {
                kind,
                position: Position::Center,
                size: 0.2,
                margin: 0.0,
                opacity: 1.0,
            };
            assert!(mark.fitted(100, 100, Space::Srgb).is_err());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn flat(w: u32, h: u32, v: u8) -> Vec<u8> {
        vec![v; (w * h * 3) as usize]
    }

    /// The box of pixels that differ from `v`.
    fn changed(pixels: &[u8], w: u32, h: u32, v: u8) -> Option<(u32, u32, u32, u32)> {
        let mut b: Option<(u32, u32, u32, u32)> = None;
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                if pixels[i..i + 3].iter().any(|&c| c != v) {
                    b = Some(match b {
                        None => (x, y, x, y),
                        Some((a, b2, c, d)) => (a.min(x), b2.min(y), c.max(x), d.max(y)),
                    });
                }
            }
        }
        b
    }

    #[test]
    fn positions_read_back_by_name_and_place_by_cell() {
        for p in Position::ALL {
            assert_eq!(Position::from_name(p.name()), Some(p));
        }
        assert_eq!(
            Position::from_name("bottom right"),
            Some(Position::BottomRight)
        );
        assert_eq!(Position::from_name("nowhere"), None);
        let (pic, mark, m) = ((1000, 600), (100, 40), 20);
        assert_eq!(place(pic, mark, Position::TopLeft, m), (20, 20));
        assert_eq!(place(pic, mark, Position::Top, m), (450, 20));
        assert_eq!(place(pic, mark, Position::TopRight, m), (880, 20));
        assert_eq!(place(pic, mark, Position::Left, m), (20, 280));
        assert_eq!(place(pic, mark, Position::Center, m), (450, 280));
        assert_eq!(place(pic, mark, Position::Right, m), (880, 280));
        assert_eq!(place(pic, mark, Position::BottomLeft, m), (20, 540));
        assert_eq!(place(pic, mark, Position::Bottom, m), (450, 540));
        assert_eq!(place(pic, mark, Position::BottomRight, m), (880, 540));
    }

    #[test]
    fn an_image_mark_lands_where_placed_at_its_opacity() {
        let dir = std::env::temp_dir().join(format!("greycard-mark-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = bar_png(&dir, 64);
        let (w, h) = (800u32, 500u32);
        let bg = 40u8;
        let mark = Mark {
            kind: Kind::Image { path: png },
            position: Position::TopLeft,
            size: 0.25,
            margin: 0.02,
            opacity: 0.5,
        };
        let mut pixels = flat(w, h, bg);
        mark.apply(&mut pixels, w, h, Space::Srgb).unwrap();
        // 200 wide (a quarter of 800), 100 high (the PNG is 2:1), the
        // bar its middle half; 16 in from the top and the left.
        // Everything that moved is inside the mark's box, the bar runs
        // its full width, and the bar's edges are where the PNG's are
        // give or take the resize's ringing.
        let (x0, y0, x1, y1) = changed(&pixels, w, h, bg).expect("a mark");
        assert_eq!((x0, x1), (16, 16 + 200 - 1));
        assert!(y0 >= 16 && y1 < 16 + 100, "{y0}..{y1}");
        let row_moved = |y: u32| pixels[((y * w + 116) * 3) as usize] > bg + 8;
        let first = (16..116).find(|&y| row_moved(y)).unwrap();
        let last = (16..116).rev().find(|&y| row_moved(y)).unwrap();
        assert!((first as i32 - 41).abs() <= 1, "{first}");
        assert!((last as i32 - 90).abs() <= 1, "{last}");
        // Inside the bar: half-way from the ground to white, in the
        // encoded numbers.
        let i = (((16 + 50) * w + 16 + 100) * 3) as usize;
        let want = bg as f32 + (255.0 - bg as f32) * 0.5;
        assert!(
            (pixels[i] as f32 - want).abs() <= 1.0,
            "{} vs {want}",
            pixels[i]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_text_mark_lands_bottom_right_and_leaves_the_rest() {
        if no_font("a_text_mark_lands_bottom_right_and_leaves_the_rest") {
            return;
        }
        let (w, h) = (1200u32, 800u32);
        let bg = 100u8;
        let mark = Mark {
            kind: Kind::Text {
                text: "© greycard".into(),
                white: true,
            },
            position: Position::BottomRight,
            size: 0.3,
            margin: 0.025,
            opacity: 0.5,
        };
        let mut pixels = flat(w, h, bg);
        mark.apply(&mut pixels, w, h, Space::DisplayP3).unwrap();
        let (x0, y0, x1, y1) = changed(&pixels, w, h, bg).expect("a mark");
        // 360 wide, its right and bottom 30 in from the edges.
        let width = x1 - x0 + 1;
        assert!((width as i32 - 360).abs() <= 1, "{width}");
        assert!((x1 as i32 - (1200 - 30 - 1)).abs() <= 1, "{x1}");
        assert!((y1 as i32 - (800 - 30 - 1)).abs() <= 1, "{y1}");
        assert!(y0 < y1 && y0 > 600, "{y0}");
        // The strokes' cores, fully covered, are half-way from the
        // ground to white; nothing is brighter.
        let top = *pixels.iter().max().unwrap();
        let want = bg as f32 + (255.0 - bg as f32) * 0.5;
        assert!((top as f32 - want).abs() <= 1.0, "{top} vs {want}");
    }

    #[test]
    fn a_mark_is_the_same_proportion_at_any_size() {
        let dir = std::env::temp_dir().join(format!("greycard-mark-scale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = bar_png(&dir, 50);
        let text = Kind::Text {
            text: "greycard".into(),
            white: false,
        };
        let have_font = !no_font("a_mark_is_the_same_proportion_at_any_size, its text half");
        for kind in [Kind::Image { path: png }, text] {
            if matches!(kind, Kind::Text { .. }) && !have_font {
                continue;
            }
            let mark = Mark {
                kind,
                position: Position::Center,
                size: 0.2,
                margin: 0.0,
                opacity: 1.0,
            };
            let ratio = |w: u32, h: u32| {
                let mut p = flat(w, h, 128);
                mark.apply(&mut p, w, h, Space::Srgb).unwrap();
                let (x0, _, x1, _) = changed(&p, w, h, 128).unwrap();
                (x1 - x0 + 1) as f64 / w as f64
            };
            let (small, large) = (ratio(2048, 1365), ratio(6000, 4000));
            // Within a pixel of the small one.
            assert!((small - large).abs() <= 1.0 / 2048.0, "{small} vs {large}");
            assert!((small - 0.2).abs() <= 1.0 / 2048.0, "{small}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sixteen_bits_take_the_same_blend() {
        let raster = Raster {
            width: 1,
            height: 1,
            rgba: vec![1.0, 1.0, 1.0, 1.0],
        };
        let mut p = vec![10000u16; 3];
        composite(&mut p, 1, 1, &raster, (0, 0), 0.5);
        assert_eq!(p[0], ((10000.0 + 65535.0) / 2.0f32).round() as u16);
        // Off the picture: nothing, and no panic.
        let mut q = vec![7u8; 3];
        composite(&mut q, 1, 1, &raster, (5, -3), 1.0);
        assert_eq!(q, vec![7, 7, 7]);
    }
}
