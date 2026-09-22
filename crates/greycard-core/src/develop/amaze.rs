//! AMaZE: Aliasing Minimization and Zipper Elimination.
//!
//! Ported from RawTherapee's `rtengine/amaze_demosaic_RT.cc` (dev branch,
//! latest modification 2016-01-25), GPL-3.0-or-later. Copyright (c)
//! 2008-2010 Emil Martinec <ejmartin@uchicago.edu>, optimized for speed by
//! Ingo Weyrich, incorporating ideas of Luis Sanz Rodríguez and Paul Lee.
//! The algorithm, its constants and its pass structure are theirs; this
//! port takes the scalar branch. The tiling geometry is the same as the
//! original (160-pixel tiles with 16-pixel reflected margins); the buffers,
//! the threading and the border fill are ours.
//!
//! What it does, in the order the passes run on each tile:
//!
//! 1. Gradient weights along each axis and the summed squared gradient.
//! 2. Green at every site along each axis, both by Hamilton-Adams and by
//!    an adaptive color ratio (used when the ratio is near 1), giving
//!    vertical and horizontal color differences and an alternative pair.
//! 3. Pick per axis whichever pair has the smaller local variance, then
//!    bound the estimate in saturated regions with a median.
//! 4. A vertical/horizontal weight from the color-difference variance
//!    and from the disagreement of opposite-direction estimates.
//! 5. A Nyquist-texture test: where color differences fluctuate more
//!    than the gradient predicts, mark the site, clean the mask with a
//!    majority filter, and derive the weight by area statistics instead.
//! 6. Green at red and blue sites, refined by its own curvature inside
//!    Nyquist regions.
//! 7. The other chroma at red and blue sites along the better diagonal,
//!    using ratios and medians as for green; where the diagonal weight
//!    is more decisive than the axis weight, re-derive green from R+B.
//! 8. Chrominance at the remaining sites from diagonal, then cardinal,
//!    weighted color differences.
//!
//! Departures from the reference, all in the margins: the corner fill
//! reflects consistently with the edges (the original mirrors corners
//! about a different row), every buffer starts zeroed rather than
//! aliasing another, and the outer three pixels are bilinear rather than
//! the original's separate border routine.

use rayon::prelude::*;

use super::rcd::is_bayer;
use super::{bilinear_pixel, demosaic_bilinear};
use crate::error::{Error, Result};
use crate::raw::{CfaColor, CfaPattern};

/// Tile side including margins, as the reference: a multiple of 32.
const TS: usize = 160;
/// Margin on each side of a tile, reflected from the image.
const MARGIN: usize = 16;
/// Pixels at the image edge that come from bilinear instead.
const BORDER: usize = 3;

const EPS: f32 = 1e-5;
const EPS_SQ: f32 = 1e-10;
/// Adaptive ratio threshold: use the ratio estimate when it is this close
/// to 1, Hamilton-Adams otherwise.
const AR_THRESH: f32 = 0.75;
/// The level at which the least-gained channel saturates. Samples are
/// green-normalized, so that is 1.0; the reference's `1 / initialGain`.
const CLIP_PT: f32 = 1.0;
const CLIP_PT8: f32 = 0.8 * CLIP_PT;
/// Gaussian on a 5x5 quincunx, sigma 1.2.
const GAUSS_ODD: [f32; 4] = [0.146_597_28, 0.103_592_71, 0.073_203_61, 0.036_554_35];
const NYQ_THRESH: f32 = 0.5;
/// Gaussian on 5x5 times the Nyquist threshold.
const GAUSS_GRAD: [f32; 6] = [
    NYQ_THRESH * 0.073_844_12,
    NYQ_THRESH * 0.062_075_12,
    NYQ_THRESH * 0.052_181_82,
    NYQ_THRESH * 0.036_874_19,
    NYQ_THRESH * 0.030_997_32,
    NYQ_THRESH * 0.018_413_19,
];
/// Gaussian on the alternate 5x5 quincunx, sigma 1.5.
const GAUSS_EVEN: [f32; 2] = [0.137_194_94, 0.056_402_53];
/// Gaussian on the quincunx grid.
const G_QUINC: [f32; 4] = [0.169_917, 0.108_947, 0.069_855, 0.028_718_2];

/// Demosaic a Bayer mosaic with AMaZE. `samples` are one per pixel, levels
/// normalized and white balance applied with green at 1. Values are
/// clamped to `0..=max(1, largest sample)`.
///
/// Images too small for a tile interior fall back to bilinear.
pub fn demosaic_amaze(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) -> Result<Vec<f32>> {
    if !is_bayer(pattern) {
        return Err(Error::Unsupported(format!(
            "AMaZE needs a 2x2 Bayer pattern, got {pattern}"
        )));
    }
    let mut out = demosaic_bilinear(samples, width, height, pattern);
    if width < 2 * MARGIN + 2 || height < 2 * MARGIN + 2 {
        return Ok(out);
    }
    let ceiling = samples.iter().copied().fold(1.0f32, f32::max);

    // Tile origins, in image coordinates, stepping by the tile interior.
    let step = TS - 2 * MARGIN;
    let tops: Vec<isize> = (0..)
        .map(|k| -(MARGIN as isize) + (k * step) as isize)
        .take_while(|&t| t < height as isize)
        .collect();
    let lefts: Vec<isize> = (0..)
        .map(|k| -(MARGIN as isize) + (k * step) as isize)
        .take_while(|&l| l < width as isize)
        .collect();
    // A tile whose origin lies within a margin of the far edge has no
    // interior; the tile before it already covers those rows.
    let has_interior = |o: isize, n: usize| (o + MARGIN as isize) < n as isize;
    let origins: Vec<(isize, isize)> = tops
        .iter()
        .filter(|&&t| has_interior(t, height))
        .flat_map(|&t| {
            lefts
                .iter()
                .filter(|&&l| has_interior(l, width))
                .map(move |&l| (t, l))
        })
        .collect();

    let image = Image {
        samples,
        width,
        height,
        pattern,
        ceiling,
    };
    // Each tile is copied into `out` as it finishes, under a lock held
    // for the copy alone, so what is live is a tile per thread rather
    // than a second frame. (Nothing here parks a worker on another job:
    // a develop may itself be running inside the pool.)
    let sink = std::sync::Mutex::new(out.as_mut_slice());
    origins
        .par_iter()
        .map_init(Scratch::new, |scratch, &(top, left)| {
            scratch.run(&image, top, left)
        })
        .for_each(|tile| {
            let mut out = sink.lock().expect("copier does not panic");
            for r in 0..tile.rows {
                let y = tile.row0 + r;
                let src = &tile.data[r * tile.cols * 3..(r + 1) * tile.cols * 3];
                let dst =
                    &mut out[(y * width + tile.col0) * 3..(y * width + tile.col0 + tile.cols) * 3];
                dst.copy_from_slice(src);
            }
        });
    // The outer ring of the image saw reflected data; use bilinear there
    // as the reference uses its border routine.
    let mut ring = |y: usize, x: usize| {
        let i = (y * width + x) * 3;
        out[i..i + 3].copy_from_slice(&bilinear_pixel(samples, width, height, pattern, y, x));
    };
    for y in (0..BORDER).chain(height - BORDER..height) {
        for x in 0..width {
            ring(y, x);
        }
    }
    for y in BORDER..height - BORDER {
        for x in (0..BORDER).chain(width - BORDER..width) {
            ring(y, x);
        }
    }
    Ok(out)
}

struct Image<'a> {
    samples: &'a [f32],
    width: usize,
    height: usize,
    pattern: &'a CfaPattern,
    ceiling: f32,
}

impl Image<'_> {
    /// A sample at possibly out-of-range coordinates, reflected about the
    /// edge pixel so the CFA phase is preserved.
    fn at(&self, row: isize, col: isize) -> f32 {
        let r = reflect(row, self.height);
        let c = reflect(col, self.width);
        self.samples[r * self.width + c]
    }
}

fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    let r = if i < 0 {
        -i
    } else if i >= n {
        2 * n - 2 - i
    } else {
        i
    };
    r.clamp(0, n - 1) as usize
}

struct TileOut {
    row0: usize,
    col0: usize,
    rows: usize,
    cols: usize,
    data: Vec<f32>,
}

/// One thread's working buffers, all at tile resolution. The reference
/// packs several of these into half-size arrays indexed by site pair;
/// here every value lives at its own site.
struct Scratch {
    cfa: Vec<f32>,
    green: Vec<f32>,
    delhvsqsum: Vec<f32>,
    dirwts0: Vec<f32>,
    dirwts1: Vec<f32>,
    vcd: Vec<f32>,
    hcd: Vec<f32>,
    vcdalt: Vec<f32>,
    hcdalt: Vec<f32>,
    cddiffsq: Vec<f32>,
    hvwt: Vec<f32>,
    dgrb0: Vec<f32>,
    dgrb1: Vec<f32>,
    dgintv: Vec<f32>,
    dginth: Vec<f32>,
    dgrb2h: Vec<f32>,
    dgrb2v: Vec<f32>,
    delp: Vec<f32>,
    delm: Vec<f32>,
    dgrbsq1p: Vec<f32>,
    dgrbsq1m: Vec<f32>,
    rbm: Vec<f32>,
    rbp: Vec<f32>,
    pmwt: Vec<f32>,
    rbint: Vec<f32>,
    nyqutest: Vec<f32>,
    nyquist: Vec<u8>,
    nyquist2: Vec<u8>,
}

impl Scratch {
    fn new() -> Self {
        let f = || vec![0.0f32; TS * TS];
        Self {
            cfa: f(),
            green: f(),
            delhvsqsum: f(),
            dirwts0: f(),
            dirwts1: f(),
            vcd: f(),
            hcd: f(),
            vcdalt: f(),
            hcdalt: f(),
            cddiffsq: f(),
            hvwt: f(),
            dgrb0: f(),
            dgrb1: f(),
            dgintv: f(),
            dginth: f(),
            dgrb2h: f(),
            dgrb2v: f(),
            delp: f(),
            delm: f(),
            dgrbsq1p: f(),
            dgrbsq1m: f(),
            rbm: f(),
            rbp: f(),
            pmwt: f(),
            rbint: f(),
            nyqutest: f(),
            nyquist: vec![0; TS * TS],
            nyquist2: vec![0; TS * TS],
        }
    }

    fn reset(&mut self) {
        for b in [
            &mut self.hvwt,
            &mut self.dgrb0,
            &mut self.dgrb1,
            &mut self.dgrb2h,
            &mut self.dgrb2v,
            &mut self.pmwt,
        ] {
            b.fill(0.0);
        }
        self.nyquist.fill(0);
        self.nyquist2.fill(0);
    }

    #[allow(clippy::too_many_lines)]
    fn run(&mut self, image: &Image<'_>, top: isize, left: isize) -> TileOut {
        self.reset();
        let ts = TS;
        let tsi = TS as isize;
        let (v1, v2, v3) = (tsi, 2 * tsi, 3 * tsi);
        // p: towards north-east; m: towards south-east.
        let (p1, p2, p3) = (-tsi + 1, -2 * tsi + 2, -3 * tsi + 3);
        let (m1, m2, m3) = (tsi + 1, 2 * tsi + 2, 3 * tsi + 3);

        let bottom = (top + tsi).min(image.height as isize + MARGIN as isize);
        let right = (left + tsi).min(image.width as isize + MARGIN as isize);
        let rr1 = (bottom - top) as usize;
        let cc1 = (right - left) as usize;

        // Tile origins are even, so the CFA phase in tile coordinates is
        // the image's own.
        let color = |rr: usize, cc: usize| image.pattern.color_at(rr & 1, cc & 1);
        let is_green = |rr: usize, cc: usize| color(rr, cc) == CfaColor::Green;

        // Tile initialization from the image with reflected margins.
        for rr in 0..rr1 {
            for cc in 0..cc1 {
                let v = image.at(top + rr as isize, left + cc as isize);
                self.cfa[rr * ts + cc] = v;
                self.green[rr * ts + cc] = v;
            }
        }

        let cfa = &self.cfa;
        let at = |buf: &[f32], i: usize, off: isize| buf[(i as isize + off) as usize];

        // Pass 1: horizontal and vertical gradients.
        for rr in 2..rr1 - 2 {
            for cc in 2..cc1 - 2 {
                let i = rr * ts + cc;
                let s = |o| at(cfa, i, o);
                let delh = (s(1) - s(-1)).abs();
                let delv = (s(v1) - s(-v1)).abs();
                self.dirwts0[i] = EPS + (s(v2) - s(0)).abs() + (s(0) - s(-v2)).abs() + delv;
                self.dirwts1[i] = EPS + (s(2) - s(0)).abs() + (s(0) - s(-2)).abs() + delh;
                self.delhvsqsum[i] = delh * delh + delv * delv;
            }
        }

        // Pass 2: vertical and horizontal color differences.
        for rr in 4..rr1 - 4 {
            for cc in 4..cc1 - 4 {
                let i = rr * ts + cc;
                let s = |o| at(cfa, i, o);
                let d0 = |o| at(&self.dirwts0, i, o);
                let d1 = |o| at(&self.dirwts1, i, o);

                // Color ratios in each cardinal direction.
                let cru =
                    s(-v1) * (d0(-v2) + d0(0)) / (d0(-v2) * (EPS + s(0)) + d0(0) * (EPS + s(-v2)));
                let crd =
                    s(v1) * (d0(v2) + d0(0)) / (d0(v2) * (EPS + s(0)) + d0(0) * (EPS + s(v2)));
                let crl =
                    s(-1) * (d1(-2) + d1(0)) / (d1(-2) * (EPS + s(0)) + d1(0) * (EPS + s(-2)));
                let crr = s(1) * (d1(2) + d1(0)) / (d1(2) * (EPS + s(0)) + d1(0) * (EPS + s(2)));

                // Hamilton-Adams in each direction.
                let guha = s(-v1) + 0.5 * (s(0) - s(-v2));
                let gdha = s(v1) + 0.5 * (s(0) - s(v2));
                let glha = s(-1) + 0.5 * (s(0) - s(-2));
                let grha = s(1) + 0.5 * (s(0) - s(2));

                // Adaptive ratios where the ratio is near 1.
                let ar = |cr: f32, ha: f32| {
                    if (1.0 - cr).abs() < AR_THRESH {
                        s(0) * cr
                    } else {
                        ha
                    }
                };
                let mut guar = ar(cru, guha);
                let mut gdar = ar(crd, gdha);
                let mut glar = ar(crl, glha);
                let mut grar = ar(crr, grha);

                // Adaptive weights for the two directions.
                let hwt = d1(-1) / (d1(-1) + d1(1));
                let vwt = d0(-v1) / (d0(v1) + d0(-v1));

                let gintvha = vwt * gdha + (1.0 - vwt) * guha;
                let ginthha = hwt * grha + (1.0 - hwt) * glha;

                if is_green(rr, cc) {
                    self.vcd[i] = s(0) - (vwt * gdar + (1.0 - vwt) * guar);
                    self.hcd[i] = s(0) - (hwt * grar + (1.0 - hwt) * glar);
                    self.vcdalt[i] = s(0) - gintvha;
                    self.hcdalt[i] = s(0) - ginthha;
                } else {
                    self.vcd[i] = (vwt * gdar + (1.0 - vwt) * guar) - s(0);
                    self.hcd[i] = (hwt * grar + (1.0 - hwt) * glar) - s(0);
                    self.vcdalt[i] = gintvha - s(0);
                    self.hcdalt[i] = ginthha - s(0);
                }

                if s(0) > CLIP_PT8 || gintvha > CLIP_PT8 || ginthha > CLIP_PT8 {
                    // Hamilton-Adams where highlights are nearly clipped.
                    guar = guha;
                    gdar = gdha;
                    glar = glha;
                    grar = grha;
                    self.vcd[i] = self.vcdalt[i];
                    self.hcd[i] = self.hcdalt[i];
                }

                // Disagreement of the opposite-direction estimates.
                self.dgintv[i] = sq(guha - gdha).min(sq(guar - gdar));
                self.dginth[i] = sq(glha - grha).min(sq(glar - grar));
            }
        }

        // Pass 3: choose the smoother pair, bound in saturated regions.
        for rr in 4..rr1 - 4 {
            for cc in 4..cc1 - 4 {
                let i = rr * ts + cc;
                let s = |o| at(cfa, i, o);
                let var3 = |a: f32, b: f32, c: f32| 3.0 * (sq(a) + sq(b) + sq(c)) - sq(a + b + c);
                let hcdvar = var3(at(&self.hcd, i, -2), self.hcd[i], at(&self.hcd, i, 2));
                let hcdaltvar = var3(
                    at(&self.hcdalt, i, -2),
                    self.hcdalt[i],
                    at(&self.hcdalt, i, 2),
                );
                let vcdvar = var3(at(&self.vcd, i, -v2), self.vcd[i], at(&self.vcd, i, v2));
                let vcdaltvar = var3(
                    at(&self.vcdalt, i, -v2),
                    self.vcdalt[i],
                    at(&self.vcdalt, i, v2),
                );
                if hcdaltvar < hcdvar {
                    self.hcd[i] = self.hcdalt[i];
                }
                if vcdaltvar < vcdvar {
                    self.vcd[i] = self.vcdalt[i];
                }

                let (mut hcd, mut vcd) = (self.hcd[i], self.vcd[i]);
                if is_green(rr, cc) {
                    // Interpolated red or blue along each axis.
                    let ginth = -hcd + s(0);
                    let gintv = -vcd + s(0);
                    if hcd > 0.0 {
                        if 3.0 * hcd > ginth + s(0) {
                            hcd = -median(ginth, s(-1), s(1)) + s(0);
                        } else {
                            let hwt = 1.0 - 3.0 * hcd / (EPS + ginth + s(0));
                            hcd = hwt * hcd + (1.0 - hwt) * (-median(ginth, s(-1), s(1)) + s(0));
                        }
                    }
                    if vcd > 0.0 {
                        if 3.0 * vcd > gintv + s(0) {
                            vcd = -median(gintv, s(-v1), s(v1)) + s(0);
                        } else {
                            let vwt = 1.0 - 3.0 * vcd / (EPS + gintv + s(0));
                            vcd = vwt * vcd + (1.0 - vwt) * (-median(gintv, s(-v1), s(v1)) + s(0));
                        }
                    }
                    if ginth > CLIP_PT {
                        hcd = -median(ginth, s(-1), s(1)) + s(0);
                    }
                    if gintv > CLIP_PT {
                        vcd = -median(gintv, s(-v1), s(v1)) + s(0);
                    }
                } else {
                    // Interpolated green along each axis.
                    let ginth = hcd + s(0);
                    let gintv = vcd + s(0);
                    if hcd < 0.0 {
                        if 3.0 * hcd < -(ginth + s(0)) {
                            hcd = median(ginth, s(-1), s(1)) - s(0);
                        } else {
                            let hwt = 1.0 + 3.0 * hcd / (EPS + ginth + s(0));
                            hcd = hwt * hcd + (1.0 - hwt) * (median(ginth, s(-1), s(1)) - s(0));
                        }
                    }
                    if vcd < 0.0 {
                        if 3.0 * vcd < -(gintv + s(0)) {
                            vcd = median(gintv, s(-v1), s(v1)) - s(0);
                        } else {
                            let vwt = 1.0 + 3.0 * vcd / (EPS + gintv + s(0));
                            vcd = vwt * vcd + (1.0 - vwt) * (median(gintv, s(-v1), s(v1)) - s(0));
                        }
                    }
                    if ginth > CLIP_PT {
                        hcd = median(ginth, s(-1), s(1)) - s(0);
                    }
                    if gintv > CLIP_PT {
                        vcd = median(gintv, s(-v1), s(v1)) - s(0);
                    }
                    self.cddiffsq[i] = sq(vcd - hcd);
                }
                self.hcd[i] = hcd;
                self.vcd[i] = vcd;
            }
        }

        // Pass 4: vertical/horizontal weight at red and blue sites.
        for rr in 6..rr1 - 6 {
            for cc in 6..cc1 - 6 {
                if is_green(rr, cc) {
                    continue;
                }
                let i = rr * ts + cc;
                let vc = |o| at(&self.vcd, i, o);
                let hc = |o| at(&self.hcd, i, o);
                let uave = vc(0) + vc(-v1) + vc(-v2) + vc(-v3);
                let dave = vc(0) + vc(v1) + vc(v2) + vc(v3);
                let lave = hc(0) + hc(-1) + hc(-2) + hc(-3);
                let rave = hc(0) + hc(1) + hc(2) + hc(3);

                // Color-difference variance in each direction.
                let varu =
                    sq(vc(0) - uave) + sq(vc(-v1) - uave) + sq(vc(-v2) - uave) + sq(vc(-v3) - uave);
                let vard =
                    sq(vc(0) - dave) + sq(vc(v1) - dave) + sq(vc(v2) - dave) + sq(vc(v3) - dave);
                let varl =
                    sq(hc(0) - lave) + sq(hc(-1) - lave) + sq(hc(-2) - lave) + sq(hc(-3) - lave);
                let varr =
                    sq(hc(0) - rave) + sq(hc(1) - rave) + sq(hc(2) - rave) + sq(hc(3) - rave);

                let hwt =
                    at(&self.dirwts1, i, -1) / (at(&self.dirwts1, i, -1) + at(&self.dirwts1, i, 1));
                let vwt = at(&self.dirwts0, i, -v1)
                    / (at(&self.dirwts0, i, v1) + at(&self.dirwts0, i, -v1));

                let vcdvar = EPS_SQ + vwt * vard + (1.0 - vwt) * varu;
                let hcdvar = EPS_SQ + hwt * varr + (1.0 - hwt) * varl;

                let dv = |o| at(&self.dgintv, i, o);
                let dh = |o| at(&self.dginth, i, o);
                let varu1 = dv(0) + dv(-v1) + dv(-v2);
                let vard1 = dv(0) + dv(v1) + dv(v2);
                let varl1 = dh(0) + dh(-1) + dh(-2);
                let varr1 = dh(0) + dh(1) + dh(2);
                let vcdvar1 = EPS_SQ + vwt * vard1 + (1.0 - vwt) * varu1;
                let hcdvar1 = EPS_SQ + hwt * varr1 + (1.0 - hwt) * varl1;

                let varwt = hcdvar / (vcdvar + hcdvar);
                let diffwt = hcdvar1 / (vcdvar1 + hcdvar1);

                // If both agree on the direction take the more decisive,
                // otherwise the fluctuation weight.
                self.hvwt[i] = if (0.5 - varwt) * (0.5 - diffwt) > 0.0
                    && (0.5 - diffwt).abs() < (0.5 - varwt).abs()
                {
                    varwt
                } else {
                    diffwt
                };
            }
        }

        // Pass 5: Nyquist texture test at red and blue sites.
        let mut ny_rows = (0usize, 0usize);
        let mut ny_cols = (ts + 1, 0usize);
        let mut any_nyquist = false;
        for rr in 6..rr1 - 6 {
            for cc in 6..cc1 - 6 {
                if is_green(rr, cc) {
                    continue;
                }
                let i = rr * ts + cc;
                let cd = |o| at(&self.cddiffsq, i, o);
                let dq = |o| at(&self.delhvsqsum, i, o);
                let test = GAUSS_ODD[0] * cd(0)
                    + GAUSS_ODD[1] * (cd(-m1) + cd(p1) + cd(-p1) + cd(m1))
                    + GAUSS_ODD[2] * (cd(-v2) + cd(-2) + cd(2) + cd(v2))
                    + GAUSS_ODD[3] * (cd(-m2) + cd(p2) + cd(-p2) + cd(m2))
                    - (GAUSS_GRAD[0] * dq(0)
                        + GAUSS_GRAD[1] * (dq(-v1) + dq(1) + dq(-1) + dq(v1))
                        + GAUSS_GRAD[2] * (dq(-m1) + dq(p1) + dq(-p1) + dq(m1))
                        + GAUSS_GRAD[3] * (dq(-v2) + dq(-2) + dq(2) + dq(v2))
                        + GAUSS_GRAD[4]
                            * (dq(-v2 - 1)
                                + dq(-v2 + 1)
                                + dq(-v1 - 2)
                                + dq(-v1 + 2)
                                + dq(v1 - 2)
                                + dq(v1 + 2)
                                + dq(v2 - 1)
                                + dq(v2 + 1))
                        + GAUSS_GRAD[5] * (dq(-m2) + dq(p2) + dq(-p2) + dq(m2)));
                self.nyqutest[i] = test;
                if test > 0.0 {
                    self.nyquist[i] = 1;
                    if !any_nyquist {
                        ny_rows.0 = rr;
                        any_nyquist = true;
                    }
                    ny_rows.1 = rr;
                    ny_cols.0 = ny_cols.0.min(cc);
                    ny_cols.1 = ny_cols.1.max(cc);
                }
            }
        }
        let do_nyquist = any_nyquist && ny_rows.0 != ny_rows.1 && ny_cols.0 != ny_cols.1;
        let (ny_r0, ny_r1, ny_c0, ny_c1) = if do_nyquist {
            (
                ny_rows.0.max(8),
                (ny_rows.1 + 1).min(rr1 - 8),
                (ny_cols.0 - (ny_cols.0 & 1)).max(8),
                (ny_cols.1 + 1).min(cc1 - 8),
            )
        } else {
            (0, 0, 0, 0)
        };

        if do_nyquist {
            // Majority filter over the eight same-color-class neighbors.
            for rr in ny_r0..ny_r1 {
                for cc in ny_c0..ny_c1 {
                    if is_green(rr, cc) {
                        continue;
                    }
                    let i = rr * ts + cc;
                    let n = |o: isize| self.nyquist[(i as isize + o) as usize] as u32;
                    let count = n(-v2) + n(-m1) + n(p1) + n(-2) + n(2) + n(-p1) + n(m1) + n(v2);
                    self.nyquist2[i] = if count > 4 {
                        1
                    } else if count < 4 {
                        0
                    } else {
                        self.nyquist[i]
                    };
                }
            }
            // Area interpolation in Nyquist regions.
            for rr in ny_r0..ny_r1 {
                for cc in ny_c0..ny_c1 {
                    if is_green(rr, cc) || self.nyquist2[rr * ts + cc] == 0 {
                        continue;
                    }
                    let i = rr * ts + cc;
                    let (mut sumcfa, mut sumh, mut sumv, mut sumsqh, mut sumsqv, mut areawt) =
                        (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
                    for di in (-6..7).step_by(2) {
                        for dj in (-6..7).step_by(2) {
                            let j = (i as isize + di * tsi + dj) as usize;
                            if self.nyquist2[j] == 0 {
                                continue;
                            }
                            let c = cfa[j];
                            let s = |o| at(cfa, j, o);
                            sumcfa += c;
                            sumh += s(-1) + s(1);
                            sumv += s(-v1) + s(v1);
                            sumsqh += sq(c - s(-1)) + sq(c - s(1));
                            sumsqv += sq(c - s(-v1)) + sq(c - s(v1));
                            areawt += 1.0;
                        }
                    }
                    sumh = sumcfa - 0.5 * sumh;
                    sumv = sumcfa - 0.5 * sumv;
                    areawt *= 0.5;
                    let hcdvar = EPS_SQ + (areawt * sumsqh - sumh * sumh).abs();
                    let vcdvar = EPS_SQ + (areawt * sumsqv - sumv * sumv).abs();
                    self.hvwt[i] = hcdvar / (vcdvar + hcdvar);
                }
            }
        }

        // Pass 6: green at red and blue sites.
        for rr in 8..rr1 - 8 {
            for cc in 8..cc1 - 8 {
                if is_green(rr, cc) {
                    continue;
                }
                let i = rr * ts + cc;
                let hv = |o| at(&self.hvwt, i, o);
                let alt = 0.25 * (hv(-m1) + hv(p1) + hv(-p1) + hv(m1));
                let hvwt = if (0.5 - hv(0)).abs() < (0.5 - alt).abs() {
                    alt
                } else {
                    hv(0)
                };
                self.hvwt[i] = hvwt;
                self.dgrb0[i] = hvwt * self.vcd[i] + (1.0 - hvwt) * self.hcd[i];
                self.green[i] = cfa[i] + self.dgrb0[i];
            }
        }
        // Green curvature inside Nyquist regions, for the refinement.
        for rr in 8..rr1 - 8 {
            for cc in 8..cc1 - 8 {
                let i = rr * ts + cc;
                if is_green(rr, cc) || self.nyquist2[i] == 0 {
                    continue;
                }
                let g = |o| at(&self.green, i, o);
                self.dgrb2h[i] = sq(g(0) - 0.5 * (g(-1) + g(1)));
                self.dgrb2v[i] = sq(g(0) - 0.5 * (g(-v1) + g(v1)));
            }
        }
        if do_nyquist {
            for rr in ny_r0..ny_r1 {
                for cc in ny_c0..ny_c1 {
                    let i = rr * ts + cc;
                    if is_green(rr, cc) || self.nyquist2[i] == 0 {
                        continue;
                    }
                    let quinc = |buf: &[f32]| {
                        let b = |o| at(buf, i, o);
                        EPS_SQ
                            + G_QUINC[0] * b(0)
                            + G_QUINC[1] * (b(-m1) + b(p1) + b(-p1) + b(m1))
                            + G_QUINC[2] * (b(-v2) + b(-2) + b(2) + b(v2))
                            + G_QUINC[3] * (b(-m2) + b(p2) + b(-p2) + b(m2))
                    };
                    let gvarh = quinc(&self.dgrb2h);
                    let gvarv = quinc(&self.dgrb2v);
                    self.dgrb0[i] = (self.hcd[i] * gvarv + self.vcd[i] * gvarh) / (gvarv + gvarh);
                    self.green[i] = cfa[i] + self.dgrb0[i];
                }
            }
        }

        // Pass 7: diagonal gradients at red/blue sites, squared diagonal
        // differences at green sites.
        for rr in 6..rr1 - 6 {
            for cc in 6..cc1 - 6 {
                let i = rr * ts + cc;
                let s = |o| at(cfa, i, o);
                if is_green(rr, cc) {
                    self.dgrbsq1p[i] = sq(s(0) - s(-p1)) + sq(s(0) - s(p1));
                    self.dgrbsq1m[i] = sq(s(0) - s(-m1)) + sq(s(0) - s(m1));
                } else {
                    self.delp[i] = (s(p1) - s(-p1)).abs();
                    self.delm[i] = (s(m1) - s(-m1)).abs();
                }
            }
        }

        // Pass 8: the other chroma at red and blue sites along diagonals.
        for rr in 8..rr1 - 8 {
            for cc in 8..cc1 - 8 {
                if is_green(rr, cc) {
                    continue;
                }
                let i = rr * ts + cc;
                let s = |o| at(cfa, i, o);
                let crse = 2.0 * s(m1) / (EPS + s(0) + s(m2));
                let crnw = 2.0 * s(-m1) / (EPS + s(0) + s(-m2));
                let crne = 2.0 * s(p1) / (EPS + s(0) + s(p2));
                let crsw = 2.0 * s(-p1) / (EPS + s(0) + s(-p2));
                let est = |cr: f32, near: f32, far: f32| {
                    if (1.0 - cr).abs() < AR_THRESH {
                        s(0) * cr
                    } else {
                        near + 0.5 * (s(0) - far)
                    }
                };
                let rbse = est(crse, s(m1), s(m2));
                let rbnw = est(crnw, s(-m1), s(-m2));
                let rbne = est(crne, s(p1), s(p2));
                let rbsw = est(crsw, s(-p1), s(-p2));

                let dm = |o| at(&self.delm, i, o);
                let dp = |o| at(&self.delp, i, o);
                let wtse = EPS + dm(0) + dm(m1) + dm(m2);
                let wtnw = EPS + dm(0) + dm(-m1) + dm(-m2);
                let wtne = EPS + dp(0) + dp(p1) + dp(p2);
                let wtsw = EPS + dp(0) + dp(-p1) + dp(-p2);

                let mut rbm = (wtse * rbnw + wtnw * rbse) / (wtse + wtnw);
                let mut rbp = (wtne * rbsw + wtsw * rbne) / (wtne + wtsw);

                let even = |buf: &[f32]| {
                    let b = |o| at(buf, i, o);
                    EPS_SQ
                        + GAUSS_EVEN[0] * (b(-v1) + b(-1) + b(1) + b(v1))
                        + GAUSS_EVEN[1]
                            * (b(-v2 - 1)
                                + b(-v2 + 1)
                                + b(-2 - v1)
                                + b(2 - v1)
                                + b(-2 + v1)
                                + b(2 + v1)
                                + b(v2 - 1)
                                + b(v2 + 1))
                };
                let rbvarm = even(&self.dgrbsq1m);
                let rbvarp = even(&self.dgrbsq1p);
                self.pmwt[i] = rbvarm / (rbvarp + rbvarm);

                // Bound in regions of high saturation.
                if rbp < s(0) {
                    if 2.0 * rbp < s(0) {
                        rbp = median(rbp, s(-p1), s(p1));
                    } else {
                        let pwt = 2.0 * (s(0) - rbp) / (EPS + rbp + s(0));
                        rbp = pwt * rbp + (1.0 - pwt) * median(rbp, s(-p1), s(p1));
                    }
                }
                if rbm < s(0) {
                    if 2.0 * rbm < s(0) {
                        rbm = median(rbm, s(-m1), s(m1));
                    } else {
                        let mwt = 2.0 * (s(0) - rbm) / (EPS + rbm + s(0));
                        rbm = mwt * rbm + (1.0 - mwt) * median(rbm, s(-m1), s(m1));
                    }
                }
                if rbp > CLIP_PT {
                    rbp = median(rbp, s(-p1), s(p1));
                }
                if rbm > CLIP_PT {
                    rbm = median(rbm, s(-m1), s(m1));
                }
                self.rbp[i] = rbp;
                self.rbm[i] = rbm;
            }
        }

        // Pass 9: refine the diagonal weight from neighbors; R+B estimate.
        for rr in 10..rr1 - 10 {
            for cc in 10..cc1 - 10 {
                if is_green(rr, cc) {
                    continue;
                }
                let i = rr * ts + cc;
                let pm = |o| at(&self.pmwt, i, o);
                let alt = 0.25 * (pm(-m1) + pm(p1) + pm(-p1) + pm(m1));
                if (0.5 - pm(0)).abs() < (0.5 - alt).abs() {
                    self.pmwt[i] = alt;
                }
                self.rbint[i] = 0.5
                    * (cfa[i] + self.rbm[i] * (1.0 - self.pmwt[i]) + self.rbp[i] * self.pmwt[i]);
            }
        }

        // Pass 10: where the diagonal weight is more decisive than the axis
        // weight, re-derive green from the R+B estimate.
        for rr in 12..rr1 - 12 {
            for cc in 12..cc1 - 12 {
                if is_green(rr, cc) {
                    continue;
                }
                let i = rr * ts + cc;
                if (0.5 - self.pmwt[i]).abs() < (0.5 - self.hvwt[i]).abs() {
                    continue;
                }
                let s = |o| at(cfa, i, o);
                let rb = |o| at(&self.rbint, i, o);
                let cru = s(-v1) * 2.0 / (EPS + rb(0) + rb(-v2));
                let crd = s(v1) * 2.0 / (EPS + rb(0) + rb(v2));
                let crl = s(-1) * 2.0 / (EPS + rb(0) + rb(-2));
                let crr = s(1) * 2.0 / (EPS + rb(0) + rb(2));
                let est = |cr: f32, near: f32, far: f32| {
                    if (1.0 - cr).abs() < AR_THRESH {
                        rb(0) * cr
                    } else {
                        near + 0.5 * (rb(0) - far)
                    }
                };
                let gu = est(cru, s(-v1), rb(-v2));
                let gd = est(crd, s(v1), rb(v2));
                let gl = est(crl, s(-1), rb(-2));
                let gr = est(crr, s(1), rb(2));

                let d0 = |o| at(&self.dirwts0, i, o);
                let d1 = |o| at(&self.dirwts1, i, o);
                let mut gintv = (d0(-v1) * gd + d0(v1) * gu) / (d0(v1) + d0(-v1));
                let mut ginth = (d1(-1) * gr + d1(1) * gl) / (d1(-1) + d1(1));

                if gintv < rb(0) {
                    if 2.0 * gintv < rb(0) {
                        gintv = median(gintv, s(-v1), s(v1));
                    } else {
                        let vwt = 2.0 * (rb(0) - gintv) / (EPS + gintv + rb(0));
                        gintv = vwt * gintv + (1.0 - vwt) * median(gintv, s(-v1), s(v1));
                    }
                }
                if ginth < rb(0) {
                    if 2.0 * ginth < rb(0) {
                        ginth = median(ginth, s(-1), s(1));
                    } else {
                        let hwt = 2.0 * (rb(0) - ginth) / (EPS + ginth + rb(0));
                        ginth = hwt * ginth + (1.0 - hwt) * median(ginth, s(-1), s(1));
                    }
                }
                if ginth > CLIP_PT {
                    ginth = median(ginth, s(-1), s(1));
                }
                if gintv > CLIP_PT {
                    gintv = median(gintv, s(-v1), s(v1));
                }
                self.green[i] = ginth * (1.0 - self.hvwt[i]) + gintv * self.hvwt[i];
                self.dgrb0[i] = self.green[i] - cfa[i];
            }
        }

        // Pass 11: split G-B out of G-R at blue sites.
        for rr in 8..rr1 - 8 {
            for cc in 8..cc1 - 8 {
                if color(rr, cc) == CfaColor::Blue {
                    let i = rr * ts + cc;
                    self.dgrb1[i] = self.dgrb0[i];
                    self.dgrb0[i] = 0.0;
                }
            }
        }

        // Pass 12: the missing chroma difference at red and blue sites
        // from the four diagonal neighbors of the other color.
        for rr in 14..rr1 - 14 {
            for cc in 14..cc1 - 14 {
                let c = match color(rr, cc) {
                    CfaColor::Red => 1,
                    CfaColor::Blue => 0,
                    _ => continue,
                };
                let i = rr * ts + cc;
                let src: &[f32] = if c == 0 { &self.dgrb0 } else { &self.dgrb1 };
                let d = |o| at(src, i, o);
                let wtnw = 1.0
                    / (EPS
                        + (d(-m1) - d(m1)).abs()
                        + (d(-m1) - d(-m3)).abs()
                        + (d(m1) - d(-m3)).abs());
                let wtne = 1.0
                    / (EPS
                        + (d(p1) - d(-p1)).abs()
                        + (d(p1) - d(p3)).abs()
                        + (d(-p1) - d(p3)).abs());
                let wtsw = 1.0
                    / (EPS
                        + (d(-p1) - d(p1)).abs()
                        + (d(-p1) - d(m3)).abs()
                        + (d(p1) - d(-p3)).abs());
                let wtse = 1.0
                    / (EPS
                        + (d(m1) - d(-m1)).abs()
                        + (d(m1) - d(-p3)).abs()
                        + (d(-m1) - d(m3)).abs());
                let value = (wtnw
                    * (1.325 * d(-m1) - 0.175 * d(-m3) - 0.075 * d(-m1 - 2) - 0.075 * d(-m1 - v2))
                    + wtne
                        * (1.325 * d(p1) - 0.175 * d(p3) - 0.075 * d(p1 + 2) - 0.075 * d(p1 + v2))
                    + wtsw
                        * (1.325 * d(-p1)
                            - 0.175 * d(-p3)
                            - 0.075 * d(-p1 - 2)
                            - 0.075 * d(-p1 - v2))
                    + wtse
                        * (1.325 * d(m1) - 0.175 * d(m3) - 0.075 * d(m1 + 2) - 0.075 * d(m1 + v2)))
                    / (wtnw + wtne + wtsw + wtse);
                if c == 0 {
                    self.dgrb0[i] = value;
                } else {
                    self.dgrb1[i] = value;
                }
            }
        }

        // Output: the tile interior, red and blue from green minus the
        // chrominance, weighted over the cardinal neighbors at green sites.
        let row0 = (top + MARGIN as isize) as usize;
        let col0 = (left + MARGIN as isize) as usize;
        let rows = rr1 - 2 * MARGIN;
        let cols = cc1 - 2 * MARGIN;
        let mut data = vec![0.0f32; rows * cols * 3];
        let ceiling = image.ceiling;
        for rr in MARGIN..rr1 - MARGIN {
            for cc in MARGIN..cc1 - MARGIN {
                let i = rr * ts + cc;
                let g = self.green[i];
                let (r, b) = if is_green(rr, cc) {
                    let hv = |o| at(&self.hvwt, i, o);
                    let temp = 1.0 / (hv(-v1) + 2.0 - hv(1) - hv(-1) + hv(v1));
                    let chroma = |buf: &[f32]| {
                        let d = |o| at(buf, i, o);
                        g - (hv(-v1) * d(-v1)
                            + (1.0 - hv(1)) * d(1)
                            + (1.0 - hv(-1)) * d(-1)
                            + hv(v1) * d(v1))
                            * temp
                    };
                    (chroma(&self.dgrb0), chroma(&self.dgrb1))
                } else {
                    (g - self.dgrb0[i], g - self.dgrb1[i])
                };
                let o = ((rr - MARGIN) * cols + (cc - MARGIN)) * 3;
                data[o] = r.clamp(0.0, ceiling);
                data[o + 1] = g.clamp(0.0, ceiling);
                data[o + 2] = b.clamp(0.0, ceiling);
            }
        }
        TileOut {
            row0,
            col0,
            rows,
            cols,
            data,
        }
    }
}

#[inline]
fn sq(x: f32) -> f32 {
    x * x
}

#[inline]
fn median(a: f32, b: f32, c: f32) -> f32 {
    a.max(b).min(c).max(a.min(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bayer(name: &str) -> CfaPattern {
        let colors = name
            .chars()
            .map(|c| match c {
                'R' => CfaColor::Red,
                'G' => CfaColor::Green,
                _ => CfaColor::Blue,
            })
            .collect();
        CfaPattern::new(2, 2, colors).unwrap()
    }

    fn flat(width: usize, height: usize, pattern: &CfaPattern, rgb: [f32; 3]) -> Vec<f32> {
        (0..width * height)
            .map(|i| rgb[pattern.color_at(i / width, i % width).rgb_index().unwrap()])
            .collect()
    }

    #[test]
    fn reflection_keeps_the_cfa_phase() {
        assert_eq!(reflect(-1, 10), 1);
        assert_eq!(reflect(-16, 10), 9);
        assert_eq!(reflect(10, 10), 8);
        assert_eq!(reflect(11, 10), 7);
        assert_eq!(reflect(3, 10), 3);
        assert_eq!(median(3.0, 1.0, 2.0), 2.0);
        assert_eq!(median(1.0, 2.0, 3.0), 2.0);
        assert_eq!(median(2.0, 2.0, 1.0), 2.0);
    }

    #[test]
    fn flat_field_is_reproduced_for_every_layout_across_tile_seams() {
        // Larger than one tile interior in both directions so seams and the
        // partial last tile are exercised.
        let (w, h) = (300, 150);
        for name in ["RGGB", "BGGR", "GRBG", "GBRG"] {
            let pattern = bayer(name);
            let want = [0.5, 1.0, 0.25];
            let cfa = flat(w, h, &pattern, want);
            let out = demosaic_amaze(&cfa, w, h, &pattern).unwrap();
            for (i, px) in out.as_chunks::<3>().0.iter().enumerate() {
                for c in 0..3 {
                    assert!((px[c] - want[c]).abs() < 1e-4, "{name} pixel {i}: {px:?}");
                }
            }
        }
    }

    #[test]
    fn grey_ramp_stays_grey() {
        let (w, h) = (200, 60);
        let pattern = CfaPattern::rggb();
        let cfa: Vec<f32> = (0..w * h)
            .map(|i| (i % w) as f32 / (w - 1) as f32)
            .collect();
        let out = demosaic_amaze(&cfa, w, h, &pattern).unwrap();
        for y in BORDER..h - BORDER {
            for x in BORDER..w - BORDER {
                let want = x as f32 / (w - 1) as f32;
                let px = &out[(y * w + x) * 3..(y * w + x) * 3 + 3];
                for v in px {
                    assert!((v - want).abs() < 3e-3, "({x},{y}) {px:?} want {want}");
                }
            }
        }
    }

    #[test]
    fn highlights_above_one_survive() {
        let pattern = CfaPattern::rggb();
        let cfa = flat(64, 48, &pattern, [2.0, 1.0, 1.5]);
        let out = demosaic_amaze(&cfa, 64, 48, &pattern).unwrap();
        let px = &out[(24 * 64 + 30) * 3..(24 * 64 + 30) * 3 + 3];
        assert!(
            (px[0] - 2.0).abs() < 1e-3 && (px[2] - 1.5).abs() < 1e-3,
            "{px:?}"
        );
    }

    #[test]
    fn rejects_non_bayer_and_falls_back_when_tiny() {
        let xtrans = CfaPattern::new(6, 6, vec![CfaColor::Green; 36]).unwrap();
        assert!(demosaic_amaze(&[0.0; 36], 6, 6, &xtrans).is_err());
        let pattern = CfaPattern::rggb();
        let cfa = flat(20, 20, &pattern, [0.5, 1.0, 0.25]);
        assert_eq!(
            demosaic_amaze(&cfa, 20, 20, &pattern).unwrap(),
            demosaic_bilinear(&cfa, 20, 20, &pattern)
        );
    }
}
