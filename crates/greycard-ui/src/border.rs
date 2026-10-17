//! A border round an export: a band of one color added outside the
//! finished picture, after its resize and sharpening and before the
//! watermark and the encode, never on the viewport.
//!
//! Its width is a fraction of the picture's long edge, as the
//! watermark's size is, so a preset gives the same proportion on a
//! 2048 px export as on a full-size one. The band is added, not cut
//! from the picture: the picture keeps every pixel and the long edge
//! the sheet asked for, and the file is larger by the band on each
//! side.
//!
//! The colors are neutral fractions of the encoded range, the same
//! numbers in every output space (all three share D65 white), and
//! Match is the picture's own mean in its encoded values.

use crate::watermark::Channel;

/// The border's color, by the sheet's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    White,
    LightGrey,
    DarkGrey,
    Black,
    /// The picture's mean.
    Match,
}

impl Color {
    pub const ALL: [Color; 5] = [
        Color::White,
        Color::LightGrey,
        Color::DarkGrey,
        Color::Black,
        Color::Match,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Color::White => "White",
            Color::LightGrey => "Light grey",
            Color::DarkGrey => "Dark grey",
            Color::Black => "Black",
            Color::Match => "Match",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The fraction of the encoded range a neutral color stands at;
    /// none for Match, which the picture decides.
    fn level(self) -> Option<f32> {
        match self {
            Color::White => Some(1.0),
            Color::LightGrey => Some(0.8),
            Color::DarkGrey => Some(0.2),
            Color::Black => Some(0.0),
            Color::Match => None,
        }
    }

    /// The color in a picture's channel type: the neutral's level, or
    /// the mean of `pixels` (interleaved RGB) for Match.
    #[cfg(test)]
    pub fn fill<T: Channel>(self, pixels: &[T]) -> [T; 3] {
        self.fill_in(pixels, |l| l)
    }

    /// As `fill`, with a neutral's level (given for the sRGB curve)
    /// recoded for the picture's own curve by `recode`.
    pub fn fill_in<T: Channel>(self, pixels: &[T], recode: impl Fn(f32) -> f32) -> [T; 3] {
        match self.level() {
            Some(l) => [T::from_f32(recode(l) * T::MAX); 3],
            None => mean(pixels),
        }
    }
}

/// A border: its width as a fraction of the picture's long edge, and
/// its color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Border {
    pub width: f32,
    pub color: Color,
}

/// The narrowest and widest a border may be, as fractions of the long
/// edge.
pub const MIN_WIDTH: f32 = 0.005;
pub const MAX_WIDTH: f32 = 0.25;

impl Border {
    /// The band's width in pixels on a picture `width` by `height`:
    /// at least one, however small the picture.
    pub fn pixels(&self, width: u32, height: u32) -> u32 {
        let long = width.max(height) as f32;
        ((self.width.clamp(MIN_WIDTH, MAX_WIDTH) * long).round() as u32).max(1)
    }

    /// `pixels`, `width` by `height`, with the border round it, and
    /// the new size.
    #[cfg(test)]
    pub fn apply<T: Channel>(&self, pixels: &[T], width: u32, height: u32) -> (Vec<T>, u32, u32) {
        self.apply_in(pixels, width, height, |l| l)
    }

    /// As `apply`, for a picture on another curve than sRGB's: see
    /// `Color::fill_in`.
    pub fn apply_in<T: Channel>(
        &self,
        pixels: &[T],
        width: u32,
        height: u32,
        recode: impl Fn(f32) -> f32,
    ) -> (Vec<T>, u32, u32) {
        let band = self.pixels(width, height);
        let color = self.color.fill_in(pixels, recode);
        let out = add(pixels, width, height, band, color);
        (out, width + 2 * band, height + 2 * band)
    }
}

/// The mean of interleaved RGB `pixels`, channel by channel, in their
/// own values; black for no pixels.
pub fn mean<T: Channel>(pixels: &[T]) -> [T; 3] {
    let n = (pixels.len() / 3) as f64;
    if n == 0.0 {
        return [T::from_f32(0.0); 3];
    }
    let mut sum = [0f64; 3];
    for px in pixels.as_chunks::<3>().0 {
        for c in 0..3 {
            sum[c] += px[c].to_f32() as f64;
        }
    }
    sum.map(|s| T::from_f32((s / n) as f32))
}

/// Interleaved RGB `pixels`, `width` by `height`, inside a band `band`
/// pixels wide of `color` on every side: the result is
/// `width + 2 band` by `height + 2 band`, the picture centered in it
/// and untouched.
pub fn add<T: Channel>(pixels: &[T], width: u32, height: u32, band: u32, color: [T; 3]) -> Vec<T> {
    let (w, h, b) = (width as usize, height as usize, band as usize);
    debug_assert_eq!(pixels.len(), w * h * 3);
    let ow = w + 2 * b;
    let oh = h + 2 * b;
    let mut out = Vec::with_capacity(ow * oh * 3);
    let fill = |out: &mut Vec<T>, n: usize| {
        for _ in 0..n {
            out.extend_from_slice(&color);
        }
    };
    fill(&mut out, ow * b);
    for row in pixels.chunks_exact(w * 3).take(h) {
        fill(&mut out, b);
        out.extend_from_slice(row);
        fill(&mut out, b);
    }
    fill(&mut out, ow * b);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture whose every pixel says where it is.
    fn picture(w: u32, h: u32) -> Vec<u16> {
        let mut p = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                p.extend_from_slice(&[x as u16, y as u16, 1000 + (x * y) as u16]);
            }
        }
        p
    }

    #[test]
    fn a_border_grows_the_picture_by_twice_its_band_and_leaves_it_untouched() {
        let (w, h, b) = (7u32, 4u32, 3u32);
        let src = picture(w, h);
        let color = [9u16, 8, 7];
        let out = add(&src, w, h, b, color);
        let (ow, oh) = (w + 2 * b, h + 2 * b);
        assert_eq!(out.len(), (ow * oh * 3) as usize);
        let at = |x: u32, y: u32| {
            let i = ((y * ow + x) * 3) as usize;
            [out[i], out[i + 1], out[i + 2]]
        };
        for y in 0..oh {
            for x in 0..ow {
                let inside = (b..b + w).contains(&x) && (b..b + h).contains(&y);
                if inside {
                    let (px, py) = (x - b, y - b);
                    let i = ((py * w + px) * 3) as usize;
                    assert_eq!(at(x, y), [src[i], src[i + 1], src[i + 2]], "({x}, {y})");
                } else {
                    assert_eq!(at(x, y), color, "({x}, {y})");
                }
            }
        }
        // The corners and the middle of each edge, by name.
        for (x, y) in [
            (0, 0),
            (ow - 1, 0),
            (0, oh - 1),
            (ow - 1, oh - 1),
            (ow / 2, 0),
            (ow / 2, oh - 1),
            (0, oh / 2),
            (ow - 1, oh / 2),
        ] {
            assert_eq!(at(x, y), color);
        }
    }

    #[test]
    fn a_zero_band_is_the_picture() {
        let src = picture(5, 3);
        assert_eq!(add(&src, 5, 3, 0, [0, 0, 0]), src);
    }

    #[test]
    fn the_band_is_a_share_of_the_long_edge_at_any_size() {
        let border = Border {
            width: 0.05,
            color: Color::White,
        };
        assert_eq!(border.pixels(2048, 1365), 102);
        assert_eq!(border.pixels(1365, 2048), 102);
        assert_eq!(border.pixels(6000, 4000), 300);
        // Never under a pixel, and held to its range.
        assert_eq!(border.pixels(10, 4), 1);
        let wide = Border {
            width: 0.9,
            ..border
        };
        assert_eq!(wide.pixels(1000, 500), 250);
        let (out, ow, oh) = border.apply(&vec![0u8; 200 * 100 * 3], 200, 100);
        assert_eq!((ow, oh), (220, 120));
        assert_eq!(out.len(), (220 * 120 * 3) as usize);
        assert_eq!(&out[..3], &[255, 255, 255]);
    }

    #[test]
    fn the_colors_are_neutral_levels_and_match_is_the_mean() {
        let p8 = [10u8, 20, 30, 30, 40, 51];
        assert_eq!(Color::White.fill(&p8), [255; 3]);
        assert_eq!(Color::LightGrey.fill(&p8), [204; 3]);
        assert_eq!(Color::DarkGrey.fill(&p8), [51; 3]);
        assert_eq!(Color::Black.fill(&p8), [0; 3]);
        assert_eq!(Color::Match.fill(&p8), [20, 30, 41]);
        let p16 = [0u16, 65535, 1000, 2, 65535, 3000];
        assert_eq!(Color::White.fill(&p16), [65535; 3]);
        assert_eq!(Color::LightGrey.fill(&p16), [52428; 3]);
        assert_eq!(Color::Match.fill(&p16), [1, 65535, 2000]);
        for c in Color::ALL {
            assert_eq!(Color::from_name(c.name()), Some(c));
        }
        assert_eq!(Color::from_name("Grey"), None);
    }
}
