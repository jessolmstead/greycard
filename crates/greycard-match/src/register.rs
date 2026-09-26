//! Laying the camera's JPEG over the develop.
//!
//! The JPEG is the same frame cropped a little, scaled and possibly a
//! pixel or two off. A similarity about the center is enough: a scale
//! on top of the fit-to-size factor and a shift, searched on
//! luminance at 512 wide and refined at 1024, scored by normalized
//! cross-correlation with the border left out.

use crate::color::{decode3, luminance};

/// An RGB picture in encoded sRGB, rows top down.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// `width * height` pixels, row-major.
    pub data: Vec<[f32; 3]>,
}

/// One plane of floats, rows top down.
#[derive(Debug, Clone, PartialEq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

/// How the JPEG lays over the render: the scale on top of fitting
/// the JPEG into the render's frame, the shift in render pixels, and
/// the correlation at that placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Registration {
    pub scale: f32,
    pub dy: f32,
    pub dx: f32,
    pub ncc: f32,
}

/// A registration under this correlation is not the same picture.
pub const NCC_MIN: f32 = 0.9;

impl Picture {
    pub fn new(width: usize, height: usize, data: Vec<[f32; 3]>) -> Self {
        assert_eq!(data.len(), width * height);
        Picture {
            width,
            height,
            data,
        }
    }

    pub fn filled(width: usize, height: usize, v: [f32; 3]) -> Self {
        Picture::new(width, height, vec![v; width * height])
    }

    pub fn at(&self, y: usize, x: usize) -> [f32; 3] {
        self.data[y * self.width + x]
    }

    /// Linear luminance of the encoded picture.
    pub fn luminance(&self) -> Plane {
        Plane {
            width: self.width,
            height: self.height,
            data: self.data.iter().map(|p| luminance(decode3(*p))).collect(),
        }
    }

    /// The picture resampled onto a `width × height` grid: this
    /// picture's center at the grid's center, scaled by `scale` on top
    /// of the factor that fits it inside, then shifted by `(dy, dx)`
    /// grid pixels; bilinear, the edge pixel repeated outside.
    pub fn warp_to(&self, width: usize, height: usize, scale: f32, dy: f32, dx: f32) -> Picture {
        let map = Mapping::new(self.width, self.height, width, height, scale, dy, dx);
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let (sy, sx) = map.source(y, x);
                data.push(bilinear(
                    self.width,
                    self.height,
                    |yy, xx| self.data[yy * self.width + xx],
                    sy,
                    sx,
                ));
            }
        }
        Picture::new(width, height, data)
    }

    /// Which grid pixels the warp fills from inside this picture,
    /// 1 where every corner of the sample lies in the source.
    ///
    /// The script this came from warped a plane of ones with the edge
    /// repeated, so its inside mask was one everywhere and cut nothing;
    /// this is the mask it meant to have, and block pairs near the
    /// border differ from the script's for it.
    pub fn coverage(&self, width: usize, height: usize, scale: f32, dy: f32, dx: f32) -> Plane {
        let map = Mapping::new(self.width, self.height, width, height, scale, dy, dx);
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let (sy, sx) = map.source(y, x);
                let inside = sy >= 0.0
                    && sx >= 0.0
                    && sy <= (self.height - 1) as f32
                    && sx <= (self.width - 1) as f32;
                data.push(if inside { 1.0 } else { 0.0 });
            }
        }
        Plane {
            width,
            height,
            data,
        }
    }
}

/// The source coordinate a grid pixel samples under a similarity.
struct Mapping {
    fit: f32,
    sh: f32,
    sw: f32,
    th: f32,
    tw: f32,
    dy: f32,
    dx: f32,
}

impl Mapping {
    fn new(sw: usize, sh: usize, tw: usize, th: usize, scale: f32, dy: f32, dx: f32) -> Self {
        let fit = (th as f32 / sh as f32).min(tw as f32 / sw as f32) * scale;
        Mapping {
            fit,
            sh: sh as f32,
            sw: sw as f32,
            th: th as f32,
            tw: tw as f32,
            dy,
            dx,
        }
    }

    fn source(&self, y: usize, x: usize) -> (f32, f32) {
        (
            (y as f32 - self.th / 2.0 - self.dy) / self.fit + self.sh / 2.0,
            (x as f32 - self.tw / 2.0 - self.dx) / self.fit + self.sw / 2.0,
        )
    }
}

/// Bilinear sample of a `width × height` field at a fractional
/// position, clamped to the edge (scipy's `mode="nearest"`).
fn bilinear<T, F>(width: usize, height: usize, at: F, y: f32, x: f32) -> T
where
    T: Lerp,
    F: Fn(usize, usize) -> T,
{
    let y = y.clamp(0.0, (height - 1) as f32);
    let x = x.clamp(0.0, (width - 1) as f32);
    let y0 = y.floor() as usize;
    let x0 = x.floor() as usize;
    let y1 = (y0 + 1).min(height - 1);
    let x1 = (x0 + 1).min(width - 1);
    let fy = y - y0 as f32;
    let fx = x - x0 as f32;
    let top = at(y0, x0).lerp(at(y0, x1), fx);
    let bottom = at(y1, x0).lerp(at(y1, x1), fx);
    top.lerp(bottom, fy)
}

trait Lerp: Copy {
    fn lerp(self, other: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(self, other: f32, t: f32) -> f32 {
        self + (other - self) * t
    }
}

impl Lerp for [f32; 3] {
    fn lerp(self, other: [f32; 3], t: f32) -> [f32; 3] {
        [0, 1, 2].map(|k| self[k] + (other[k] - self[k]) * t)
    }
}

impl Plane {
    pub fn at(&self, y: usize, x: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// The plane resampled to `width` across, keeping the aspect,
    /// bilinear.
    pub fn resized_to_width(&self, width: usize) -> Plane {
        let f = self.width as f32 / width as f32;
        let height = ((self.height as f32 / f).round() as usize).max(1);
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                data.push(bilinear(
                    self.width,
                    self.height,
                    |yy, xx| self.at(yy, xx),
                    y as f32 * f,
                    x as f32 * f,
                ));
            }
        }
        Plane {
            width,
            height,
            data,
        }
    }

    /// The plane laid onto a grid by a similarity, as [`Picture::warp_to`].
    pub fn warp_to(&self, width: usize, height: usize, scale: f32, dy: f32, dx: f32) -> Plane {
        let map = Mapping::new(self.width, self.height, width, height, scale, dy, dx);
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let (sy, sx) = map.source(y, x);
                data.push(bilinear(
                    self.width,
                    self.height,
                    |yy, xx| self.at(yy, xx),
                    sy,
                    sx,
                ));
            }
        }
        Plane {
            width,
            height,
            data,
        }
    }
}

/// Normalized cross-correlation of two same-size planes, with a
/// border of `margin` pixels left out on every side.
pub fn ncc(a: &Plane, b: &Plane, margin: usize) -> f32 {
    ncc_shifted(a, b, 0, 0, margin, 1)
}

/// The correlation of `a` moved down by `dy` and right by `dx` whole
/// pixels against `b`, over `b`'s interior inside `margin`, which must
/// cover the shift, sampling every `step`th pixel each way.
pub fn ncc_shifted(a: &Plane, b: &Plane, dy: i32, dx: i32, margin: usize, step: usize) -> f32 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    assert!(margin as i32 >= dy.abs() && margin as i32 >= dx.abs());
    if a.width <= 2 * margin || a.height <= 2 * margin {
        return 0.0;
    }
    let src = |y: usize, x: usize| a.at((y as i32 - dy) as usize, (x as i32 - dx) as usize);
    let mut n = 0.0f64;
    let (mut sa, mut sb) = (0.0f64, 0.0f64);
    for y in (margin..a.height - margin).step_by(step) {
        for x in (margin..a.width - margin).step_by(step) {
            sa += src(y, x) as f64;
            sb += b.at(y, x) as f64;
            n += 1.0;
        }
    }
    let (ma, mb) = (sa / n, sb / n);
    let (mut sab, mut saa, mut sbb) = (0.0f64, 0.0f64, 0.0f64);
    for y in (margin..a.height - margin).step_by(step) {
        for x in (margin..a.width - margin).step_by(step) {
            let da = src(y, x) as f64 - ma;
            let db = b.at(y, x) as f64 - mb;
            sab += da * db;
            saa += da * da;
            sbb += db * db;
        }
    }
    (sab / (saa * sbb + 1e-12).sqrt()) as f32
}

/// The similarity that lays `jpeg` over `render`, searched on their
/// luminance: scales from 0.94 to 1.06 in 25 steps and shifts of up
/// to six pixels at 512 wide, then ±0.006 in 13 steps and three
/// pixels about the first pass's shift at 1024. The shift comes back
/// in render pixels.
///
/// The second pass searches about the first's answer, not about
/// zero: the script it came from searched about zero and so lost any
/// shift over three of its pixels, letting a frame a dozen render
/// pixels off into the fit with a correlation still over the cutoff.
pub fn register(render: &Picture, jpeg: &Picture) -> Registration {
    let r_lum = render.luminance();
    let j_lum = jpeg.luminance();
    let mut best: Option<Registration> = None;
    for width in [512usize, 1024] {
        let r_small = r_lum.resized_to_width(width);
        let f = render.width as f32 / width as f32;
        let j_width =
            ((width as f32 * jpeg.width as f32 / render.width as f32).round() as usize).max(1);
        let j_small = j_lum.resized_to_width(j_width);
        // The scales and shifts to try, and the sampling step: every
        // other pixel in the coarse pass, where the peak is broad,
        // every pixel in the refinement.
        let (scales, shifts, step): (Vec<f32>, Vec<(i32, i32)>, usize) = match best {
            None => (
                (0..25).map(|i| 0.94 + 0.12 * i as f32 / 24.0).collect(),
                (-6..=6)
                    .flat_map(|dy| (-6..=6).map(move |dx| (dy, dx)))
                    .collect(),
                2,
            ),
            Some(b) => {
                let (cy, cx) = ((b.dy / f).round() as i32, (b.dx / f).round() as i32);
                (
                    (0..13)
                        .map(|i| b.scale - 0.006 + 0.012 * i as f32 / 12.0)
                        .collect(),
                    (-3..=3)
                        .flat_map(|dy| (-3..=3).map(move |dx| (cy + dy, cx + dx)))
                        .collect(),
                    1,
                )
            }
        };
        let margin = shifts
            .iter()
            .map(|(dy, dx)| dy.abs().max(dx.abs()) as usize)
            .max()
            .unwrap_or(0)
            .max(8);
        let mut cand: Option<Registration> = None;
        for &s in &scales {
            let warped = j_small.warp_to(r_small.width, r_small.height, s, 0.0, 0.0);
            for &(dy, dx) in &shifts {
                let c = ncc_shifted(&warped, &r_small, dy, dx, margin, step);
                if cand.is_none_or(|b| c > b.ncc) {
                    cand = Some(Registration {
                        scale: s,
                        dy: dy as f32 * f,
                        dx: dx as f32 * f,
                        ncc: c,
                    });
                }
            }
        }
        best = cand;
    }
    best.expect("two passes ran")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture with structure at every scale: sums of sines plus a
    /// few blobs, so a correlation has something to lock onto.
    fn scene(width: usize, height: usize) -> Picture {
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
                // Waves at several scales, the finest a few pixels
                // wide, so the correlation has a sharp peak.
                let mut v = 0.5
                    + 0.15 * (fx * 23.0).sin() * (fy * 17.0).cos()
                    + 0.1 * (fx * 61.0 + fy * 43.0).sin()
                    + 0.08 * (fx * 7.0).cos()
                    + 0.06 * (fx * 397.0).sin() * (fy * 311.0).sin();
                for (cx, cy, rad) in [(0.3, 0.4, 0.05), (0.7, 0.6, 0.08), (0.5, 0.2, 0.03)] {
                    let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                    if d < rad {
                        v += 0.25;
                    }
                }
                let v = v.clamp(0.05, 0.95);
                data.push([v, v * 0.9, v * 0.8]);
            }
        }
        Picture::new(width, height, data)
    }

    /// The "render" is the scene at 1365 × 2048 (a portrait frame);
    /// the "JPEG" is a crop of a bigger version of it, so that it lays
    /// over the render at a scale a little under one and a shift. A
    /// JPEG showing 1/`crop` of the frame is magnified by `crop` once
    /// fitted to the render's size, so the warp that lays it back
    /// samples it at 1/`crop`; and a window cut `dy` render pixels up
    /// in the scene has its content `dy` down, so the warp's shift is
    /// the window's offset negated.
    fn recover(crop: f32, dy: f32, dx: f32) -> Registration {
        let render = scene(1365, 2048);
        let big = scene(1365 * 3, 2048 * 3);
        let (jw, jh) = (
            (1365.0 * 3.0 / crop) as usize,
            (2048.0 * 3.0 / crop) as usize,
        );
        let (oy, ox) = (
            ((2048.0 * 3.0 - jh as f32) / 2.0 - dy * 3.0) as usize,
            ((1365.0 * 3.0 - jw as f32) / 2.0 - dx * 3.0) as usize,
        );
        let mut data = Vec::with_capacity(jw * jh);
        for y in 0..jh {
            for x in 0..jw {
                data.push(big.at(oy + y, ox + x));
            }
        }
        let jpeg = Picture::new(jw, jh, data);
        let r = register(&render, &jpeg);
        assert!(r.ncc > 0.95, "{r:?}");
        assert!((r.scale - 1.0 / crop).abs() <= 0.006, "{r:?}");
        assert!((r.dy + dy).abs() <= 2.5, "{r:?}");
        assert!((r.dx + dx).abs() <= 2.5, "{r:?}");
        // And the warp under it lays the JPEG back over the render.
        let laid = jpeg.warp_to(render.width, render.height, r.scale, r.dy, r.dx);
        let c = ncc(&laid.luminance(), &render.luminance(), 32);
        assert!(c > 0.98, "{c}");
        r
    }

    #[test]
    fn a_known_scale_and_shift_are_recovered() {
        recover(1.02, 4.0, -3.0);
    }

    #[test]
    fn a_shift_past_the_refinement_window_is_recovered() {
        // Over eight render pixels: more than the second pass's
        // window about zero, so only a search about the first pass's
        // answer finds it.
        // The crop leaves room for the window to move by the shift
        // (at 1.03 the big scene has 89 rows and 60 columns spare).
        let r = recover(1.03, 10.0, -9.0);
        assert!(r.dy < -7.0 && r.dx > 6.0, "{r:?}");
        let r = recover(1.03, 16.0, 0.0);
        assert!(r.dy < -13.0, "{r:?}");
    }

    #[test]
    fn a_featureless_frame_does_not_register() {
        let flat = Picture::filled(400, 300, [0.5, 0.5, 0.5]);
        let render = scene(400, 300);
        let r = register(&render, &flat);
        assert!(r.ncc < NCC_MIN, "{r:?}");
    }

    #[test]
    fn coverage_marks_the_border_the_jpeg_does_not_reach() {
        let jpeg = Picture::filled(100, 100, [0.5; 3]);
        // Scaled down, the JPEG leaves a border of the grid unfilled.
        let cov = jpeg.coverage(100, 100, 0.9, 0.0, 0.0);
        assert_eq!(cov.at(50, 50), 1.0);
        assert_eq!(cov.at(0, 0), 0.0);
        assert_eq!(cov.at(99, 99), 0.0);
    }

    #[test]
    fn warp_at_identity_is_the_picture() {
        let p = scene(64, 48);
        let w = p.warp_to(64, 48, 1.0, 0.0, 0.0);
        for (a, b) in p.data.iter().zip(&w.data) {
            assert!((a[0] - b[0]).abs() < 1e-5);
        }
    }
}
