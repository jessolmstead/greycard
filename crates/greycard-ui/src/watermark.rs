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
use anyhow::{Context, Result, anyhow, bail};
use greycard_core::RgbSpace;
use greycard_core::color::{CAT, srgb_decode, srgb_encode};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Where on the picture the mark sits: a corner, the middle of an
/// edge, or the centre.
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

    /// Draw the mark for an export `width` by `height` in `space`.
    pub fn raster(&self, width: u32, height: u32, space: Space) -> Result<Raster> {
        let target = self.width_on(width.max(height));
        match &self.kind {
            Kind::Text { text, white } => {
                let text = text.trim();
                if text.is_empty() {
                    bail!("the watermark's text is empty");
                }
                let font = system_font()?;
                let font = FontRef::try_from_slice_and_index(&font.bytes, font.index)
                    .map_err(|e| anyhow!("the system font: {e}"))?;
                let (w, h, coverage) = set_text(&font, text, target)?;
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
        let raster = self.raster(width, height, space)?;
        let margin = self.margin_on(width.max(height));
        let at = place(
            (width, height),
            (raster.width, raster.height),
            self.position,
            margin,
        );
        composite(pixels, width, height, &raster, at, self.opacity);
        Ok(())
    }
}

/// Where the mark's top-left corner goes: the margin in from the edges
/// its position names, centred along the others. A mark larger than
/// the picture less its margins may start before the picture does;
/// what falls outside is not drawn.
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
/// the mark's alpha at `opacity`, plus the mark's premultiplied colour
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
/// resize, so its soft edges keep no fringe of whatever colour its
/// clear pixels hold, and its colours taken from sRGB, which is what a
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
    // The resize rings a little past the ends; a colour is never more
    // than its alpha once premultiplied.
    let mut rgba = resized.into_raw();
    for px in rgba.chunks_exact_mut(4) {
        px[3] = px[3].clamp(0.0, 1.0);
        for c in 0..3 {
            px[c] = px[c].clamp(0.0, px[3]);
        }
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

/// A font's file and its face within it.
pub struct SystemFont {
    pub bytes: Vec<u8>,
    pub index: u32,
}

/// The system's sans-serif, the regular face, found once.
pub fn system_font() -> Result<&'static SystemFont> {
    static FONT: OnceLock<Option<SystemFont>> = OnceLock::new();
    FONT.get_or_init(find_sans_serif)
        .as_ref()
        .context("no sans-serif font on this system for the watermark's text")
}

fn find_sans_serif() -> Option<SystemFont> {
    use fontique::{Collection, CollectionOptions, GenericFamily};
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    let mut ids: Vec<_> = collection
        .generic_families(GenericFamily::SansSerif)
        .collect();
    ids.extend(collection.generic_families(GenericFamily::SystemUi));
    for id in ids {
        let Some(family) = collection.family(id) else {
            continue;
        };
        let Some(font) = family.default_font() else {
            continue;
        };
        let Some(blob) = font.load(None) else {
            continue;
        };
        let bytes = blob.as_ref().to_vec();
        // Only a face ab_glyph can read is any use.
        if FontRef::try_from_slice_and_index(&bytes, font.index()).is_err() {
            continue;
        }
        tracing::info!("watermark font: {}", family.name());
        return Some(SystemFont {
            bytes,
            index: font.index(),
        });
    }
    None
}

/// The text's glyphs laid along one line at `px`, each with its pen
/// position; kerning included.
fn lay_out(font: &FontRef, text: &str, px: f32) -> Vec<ab_glyph::OutlinedGlyph> {
    let scaled = font.as_scaled(px);
    let mut x = 0.0;
    let mut last = None;
    let mut out = Vec::new();
    for ch in text.chars().filter(|c| !c.is_control()) {
        let id = scaled.glyph_id(ch);
        if let Some(prev) = last {
            x += scaled.kern(prev, id);
        }
        let glyph = id.with_scale_and_position(px, ab_glyph::point(x, scaled.ascent()));
        x += scaled.h_advance(id);
        last = Some(id);
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
fn set_text(font: &FontRef, text: &str, target: u32) -> Result<(u32, u32, Vec<f32>)> {
    // Measured at a size, then set at the size that makes it the
    // width asked; outlines scale linearly, and ab_glyph does not
    // hint, so one measure is enough to within the rounding of the
    // pixel bounds.
    const PROBE: f32 = 200.0;
    let probe = ink(&lay_out(font, text, PROBE)).context("the watermark's text has no ink")?;
    let probe_width = (probe.max.x - probe.min.x).max(1.0);
    let px = PROBE * target as f32 / probe_width;
    let glyphs = lay_out(font, text, px);
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
        if cfg!(not(target_os = "linux")) && system_font().is_err() {
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
            opacity: 1.0,
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
        // Some pixels are fully white: the strokes' cores.
        assert!(pixels.iter().any(|&c| c == 255));
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
        let have_font = system_font().is_ok();
        for kind in [Kind::Image { path: png }, text] {
            if matches!(kind, Kind::Text { .. }) && !have_font && cfg!(not(target_os = "linux")) {
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
