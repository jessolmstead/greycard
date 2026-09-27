//! The scopes: the RGB histogram, the waveform, the RGB parade and the
//! vectorscope.
//!
//! All four are binnings of the same picture, the whole frame drawn
//! small through the viewport's shader, so they show what the screen
//! shows: encoded output, not linear light. [`bins`] is the reference
//! for `scope.wgsl`, which bins the same way on the GPU; the tests
//! check the drawing against bins the reference made.
//!
//! One buffer holds them all. The histogram is always there, in the
//! first [`HIST`] bins, because the curve editor draws it behind the
//! curve whatever the panel shows; the chosen scope's own bins follow
//! it.

/// Levels a channel is binned into.
pub const LEVELS: usize = 256;
/// The histogram: 256 red, 256 green, 256 blue.
pub const HIST: usize = 3 * LEVELS;
/// Columns of the waveform. The parade reads the same bins.
pub const COLUMNS: usize = 256;
/// The waveform: a level histogram per column per channel.
pub const WAVE: usize = 3 * COLUMNS * LEVELS;
/// Cells across the vectorscope's square grid of Cb and Cr. One cell
/// is one pixel of the picture: `2 * RADIUS == WHEEL`.
pub const WHEEL: usize = 208;
/// The vectorscope.
pub const VECTOR: usize = WHEEL * WHEEL;
/// The most bins any scope asks for, which is what the buffer holds.
pub const MAX: usize = HIST + WAVE;

/// Which scope the panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// The RGB histogram.
    #[default]
    Rgb,
    /// Levels against image column, the channels over each other.
    Waveform,
    /// The waveform's three channels side by side.
    Parade,
    /// Chroma on a Cb/Cr wheel, with the skin-tone line.
    Vector,
}

impl Scope {
    /// In the order the panel offers them.
    pub const ALL: [Scope; 4] = [Scope::Rgb, Scope::Waveform, Scope::Parade, Scope::Vector];

    pub fn name(self) -> &'static str {
        match self {
            Scope::Rgb => "RGB",
            Scope::Waveform => "Wave",
            Scope::Parade => "Parade",
            Scope::Vector => "Vector",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    /// What the shader binds beyond the histogram: 0 nothing, 1 the
    /// waveform, 2 the vectorscope.
    pub fn kind(self) -> u32 {
        match self {
            Scope::Rgb => 0,
            Scope::Waveform | Scope::Parade => 1,
            Scope::Vector => 2,
        }
    }

    /// Bins this scope reads, the histogram's included.
    pub fn bin_count(self) -> usize {
        match self.kind() {
            1 => HIST + WAVE,
            2 => HIST + VECTOR,
            _ => HIST,
        }
    }

    /// The picture's size in pixels. The histogram is low; a waveform
    /// wants height to hold a trace apart; the vectorscope is square
    /// so its circle is round however wide the panel is.
    pub fn size(self) -> (u32, u32) {
        match self {
            Scope::Rgb => (256, 80),
            Scope::Waveform | Scope::Parade => (256, 140),
            Scope::Vector => (WHEEL as u32 + 12, WHEEL as u32 + 12),
        }
    }
}

// The binning below is the reference: `scope.wgsl` is written from
// it and the tests measure the drawing against it. The running
// program bins on the GPU, so nothing else calls these.

/// The level a channel value falls in, as the shader rounds it.
#[allow(dead_code)]
fn level(v: f32) -> usize {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as usize
}

/// The cell of the vectorscope's grid a color falls in, or `None`
/// when its chroma is off the grid. Rec.709 luma, and Cb and Cr each
/// from -0.5 to 0.5; the grid runs left to right in Cb and top to
/// bottom in falling Cr, as the picture is drawn.
#[allow(dead_code)]
pub fn wheel_cell(c: [f32; 3]) -> Option<usize> {
    let (cb, cr) = chroma(c);
    let u = ((cb + 0.5) * WHEEL as f32).floor();
    let v = ((0.5 - cr) * WHEEL as f32).floor();
    if u < 0.0 || v < 0.0 || u >= WHEEL as f32 || v >= WHEEL as f32 {
        return None;
    }
    Some(v as usize * WHEEL + u as usize)
}

/// Rec.709 Cb and Cr of an encoded color.
#[allow(dead_code)]
pub fn chroma(c: [f32; 3]) -> (f32, f32) {
    let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    ((c[2] - y) / 1.8556, (c[0] - y) / 1.5748)
}

/// The color of a chroma, at mid luma: the inverse of [`chroma`] with
/// Y a half, so the middle of the wheel is grey and its edge is
/// saturated.
fn color_of(cb: f32, cr: f32) -> [f32; 3] {
    [
        (0.5 + 1.5748 * cr).clamp(0.0, 1.0),
        (0.5 - 0.1873 * cb - 0.4681 * cr).clamp(0.0, 1.0),
        (0.5 + 1.8556 * cb).clamp(0.0, 1.0),
    ]
}

/// The reference binning, over an encoded picture. `scope.wgsl` does
/// the same on the GPU over the analysis texture.
#[allow(dead_code)]
pub fn bins(scope: Scope, pixels: &[[f32; 3]], width: usize, height: usize) -> Vec<u32> {
    accumulate(scope, pixels, width, height, |_| 1)
}

/// The steps a pixel's coverage is held in: the analysis texture's
/// alpha, eight bits, where the viewport shader writes the weighing
/// mask's coverage.
pub const COVERAGE_STEPS: u32 = 255;

/// A coverage in [0, 1] as the analysis texture holds it, and as
/// `scope.wgsl` reads it back: a whole number of [`COVERAGE_STEPS`].
#[allow(dead_code)]
pub fn coverage_step(w: f32) -> u32 {
    (w.clamp(0.0, 1.0) * COVERAGE_STEPS as f32 + 0.5) as u32
}

/// The reference binning weighted by a mask: each pixel adds its
/// coverage, in [`COVERAGE_STEPS`], to every bin it lands in rather
/// than one, and the bins are then brought back to pixels by
/// [`normalize`]. `coverage` is a value a pixel, in [0, 1]. This is
/// `scope.wgsl` with its `weighted` switch on, followed by what
/// `Renderer::analyze` does with the read back.
#[allow(dead_code)]
pub fn weighted_bins(
    scope: Scope,
    pixels: &[[f32; 3]],
    coverage: &[f32],
    width: usize,
    height: usize,
) -> Vec<u32> {
    let mut out = accumulate(scope, pixels, width, height, |i| coverage_step(coverage[i]));
    normalize(&mut out, width * height);
    out
}

/// Weighted bins, which hold coverage steps, as pixels again, scaled
/// so the selection has the mass of the whole picture: a bin holding
/// `n` of a coverage sum `sum` becomes `n * pixels / sum`, rounded.
/// The histogram's end bins, which the clipping marks read
/// ([`Clipping::of`]), are never rounded to nothing where anything
/// landed, so a single pixel at the edge of a mask still lights a
/// mark; every other bin rounds, so the faint edge of a soft mask
/// does not lift the cells it touches into view. The coverage sum is
/// the histogram's red channel, into which every pixel adds its
/// coverage once.
///
/// So a small mask's scope is as bright as the whole frame's (the
/// drawing scales to its densest cells, and the mass is the same),
/// a mask of one everywhere gives the unweighted bins exactly, and a
/// mask of nothing gives no bins at all.
pub fn normalize(bins: &mut [u32], pixels: usize) {
    let sum: u64 = bins[..LEVELS.min(bins.len())]
        .iter()
        .map(|&n| u64::from(n))
        .sum();
    if sum == 0 {
        bins.fill(0);
        return;
    }
    let pixels = pixels as u64;
    let clip = |i: usize| i < HIST && matches!(i % LEVELS, 0 | 255);
    for (i, n) in bins.iter_mut().enumerate() {
        if *n != 0 {
            let scaled = (2 * u64::from(*n) * pixels + sum) / (2 * sum);
            let floor = if clip(i) { 1 } else { 0 };
            *n = scaled.clamp(floor, u64::from(u32::MAX)) as u32;
        }
    }
}

/// Bin every pixel, adding `weight(i)` for the pixel at index `i`.
#[allow(dead_code)]
fn accumulate(
    scope: Scope,
    pixels: &[[f32; 3]],
    width: usize,
    height: usize,
    weight: impl Fn(usize) -> u32,
) -> Vec<u32> {
    let mut out = vec![0u32; scope.bin_count()];
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let q = weight(i);
            if q == 0 {
                continue;
            }
            let c = pixels[i];
            for (ch, v) in c.iter().enumerate() {
                out[ch * LEVELS + level(*v)] += q;
            }
            match scope.kind() {
                1 => {
                    let column = (x * COLUMNS / width).min(COLUMNS - 1);
                    for (ch, v) in c.iter().enumerate() {
                        out[HIST + (ch * COLUMNS + column) * LEVELS + level(*v)] += q;
                    }
                }
                2 => {
                    if let Some(cell) = wheel_cell(c) {
                        out[HIST + cell] += q;
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Which channels the histogram's end bins hold: what the output
/// clips to black and to white. A channel clips when any pixel of
/// the analysis image lands in its first or last bin, as the level
/// [`level`] rounds it, so one bright pixel in a few hundred
/// thousand lights the mark, as it does on the histogram itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Clipping {
    pub shadows: [bool; 3],
    pub highlights: [bool; 3],
}

impl Clipping {
    /// From the histogram's bins.
    pub fn of(bins: &[u32]) -> Self {
        let at = |i: usize| bins.get(i).copied().unwrap_or(0) > 0;
        Self {
            shadows: [at(0), at(LEVELS), at(2 * LEVELS)],
            highlights: [at(LEVELS - 1), at(2 * LEVELS - 1), at(3 * LEVELS - 1)],
        }
    }

    /// The color of a mark for the channels in `chans`: red, green
    /// or blue for one of them; yellow, magenta or cyan for two;
    /// white for all three; `None` when nothing clips.
    pub fn mark(chans: [bool; 3]) -> Option<[u8; 3]> {
        if !chans.iter().any(|&c| c) {
            return None;
        }
        Some(chans.map(|c| if c { 0xff } else { 0x00 }))
    }
}

/// The panel's ground, which every scope is drawn on.
const GROUND: u8 = 0x14;
/// The vectorscope's radius, in pixels, at a chroma of a half.
const RADIUS: f32 = WHEEL as f32 / 2.0;

/// The scope as a picture for the panel. `bins` is what [`bins`] or
/// the compute pass made, the histogram first.
pub fn draw(scope: Scope, bins: &[u32]) -> slint::Image {
    let (w, h) = scope.size();
    let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(w, h);
    let px = buf.make_mut_slice();
    for p in px.iter_mut() {
        *p = slint::Rgb8Pixel {
            r: GROUND,
            g: GROUND,
            b: GROUND,
        };
    }
    let (w, h) = (w as usize, h as usize);
    match scope {
        Scope::Rgb => histogram(px, w, h, bins),
        Scope::Waveform => waveform(px, w, h, bins, false),
        Scope::Parade => waveform(px, w, h, bins, true),
        Scope::Vector => vector(px, w, h, bins),
    }
    if let Some(path) = std::env::var_os("GREYCARD_UI_SCOPE")
        && let Some(img) = image::RgbImage::from_raw(w as u32, h as u32, buf.as_bytes().to_vec())
        && let Err(e) = img.save(&path)
    {
        tracing::warn!("scope dump {}: {e}", path.to_string_lossy());
    }
    slint::Image::from_rgb8(buf)
}

/// Brightness of a waveform cell holding `n` of `scale`: a square
/// root, which lifts the thin parts of a trace without letting the
/// stray pixels of the highlights shout over its body.
fn lit(n: u32, scale: f32) -> f32 {
    (n as f32 / scale).min(1.0).sqrt()
}

/// What a waveform is drawn to. The flattest part of a picture piles
/// up orders of magnitude more in one cell than a textured part does,
/// and scaling to the very top leaves everything else invisible; so
/// the densest few cells in a thousand clip instead, as they do on a
/// real waveform, and the rest of the trace has the range.
fn scale(cells: &[u32]) -> f32 {
    let mut counts: Vec<u32> = cells.iter().copied().filter(|&n| n != 0).collect();
    if counts.is_empty() {
        return 1.0;
    }
    counts.sort_unstable();
    let at = ((counts.len() as f32 * 0.995) as usize).min(counts.len() - 1);
    counts[at].max(1) as f32
}

/// Brightness of a vectorscope cell. A photograph is mostly neutral,
/// so one cell in the middle holds orders of magnitude more than the
/// colors around it; a logarithm, lifted, shows both.
fn lit_log(n: u32, peak: f32) -> f32 {
    if n == 0 {
        0.0
    } else {
        ((n as f32 + 1.0).ln() / (peak + 1.0).ln()).sqrt()
    }
}

fn add_at(px: &mut [slint::Rgb8Pixel], w: usize, h: usize, x: isize, y: isize, rgb: [u8; 3]) {
    if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
        return;
    }
    let p = &mut px[y as usize * w + x as usize];
    p.r = p.r.saturating_add(rgb[0]);
    p.g = p.g.saturating_add(rgb[1]);
    p.b = p.b.saturating_add(rgb[2]);
}

/// Bars per channel, additive, as an editor's histogram.
fn histogram(px: &mut [slint::Rgb8Pixel], w: usize, h: usize, bins: &[u32]) {
    // The end bins hold everything clipped; they set no scale.
    let peak = (0..3)
        .flat_map(|c| {
            bins[c * LEVELS + 1..c * LEVELS + LEVELS - 1]
                .iter()
                .copied()
        })
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let height = |n: u32| ((n as f32 / peak).min(1.0).sqrt() * h as f32).ceil() as usize;
    for x in 0..w {
        let bin = x * LEVELS / w;
        let hs = [
            height(bins[bin]),
            height(bins[LEVELS + bin]),
            height(bins[2 * LEVELS + bin]),
        ];
        for y in 0..h {
            let from_bottom = h - 1 - y;
            let on = [
                hs[0] > from_bottom,
                hs[1] > from_bottom,
                hs[2] > from_bottom,
            ];
            if !on.iter().any(|&o| o) {
                continue;
            }
            let p = &mut px[y * w + x];
            p.r = if on[0] { 0xd0 } else { 0x30 };
            p.g = if on[1] { 0xd0 } else { 0x30 };
            p.b = if on[2] { 0xd0 } else { 0x30 };
        }
    }
}

/// Levels up the picture against the image's columns across it. As a
/// parade the three channels get a panel each, side by side;
/// otherwise they lie over one another and a neutral frame is grey.
fn waveform(px: &mut [slint::Rgb8Pixel], w: usize, h: usize, bins: &[u32], parade: bool) {
    let wave = &bins[HIST..];
    let panel = if parade { (w - 2) / 3 } else { w };
    // Bin the levels into the picture's rows first, so the scale is
    // over what is drawn and not over the finer bins behind it.
    let mut cells = vec![0u32; 3 * panel * h];
    for ch in 0..3 {
        for x in 0..panel {
            let from = x * COLUMNS / panel;
            let to = (((x + 1) * COLUMNS) / panel).max(from + 1).min(COLUMNS);
            for column in from..to {
                let base = (ch * COLUMNS + column) * LEVELS;
                for l in 0..LEVELS {
                    let n = wave[base + l];
                    if n != 0 {
                        let row = h - 1 - (l * h / LEVELS).min(h - 1);
                        cells[(ch * h + row) * panel + x] += n;
                    }
                }
            }
        }
    }
    let peak = scale(&cells);
    // Over one another the three channels add, so a neutral picture
    // would be white everywhere at a panel's gain; give each a third
    // of the range and let the overlaps make the white.
    let gain = if parade { 200.0 } else { 110.0 };
    for ch in 0..3 {
        let x0 = if parade { ch * (panel + 1) } else { 0 };
        for row in 0..h {
            for x in 0..panel {
                let i = lit(cells[(ch * h + row) * panel + x], peak);
                if i == 0.0 {
                    continue;
                }
                let mut rgb = [0u8; 3];
                rgb[ch] = (i * gain) as u8;
                add_at(px, w, h, (x0 + x) as isize, row as isize, rgb);
            }
        }
    }
}

/// Chroma on a Cb/Cr wheel, colored by where it lands, over a
/// graticule with the skin-tone line.
fn vector(px: &mut [slint::Rgb8Pixel], w: usize, h: usize, bins: &[u32]) {
    let (cx, cy) = ((w - 1) as f32 / 2.0, (h - 1) as f32 / 2.0);
    let mut mark = |x: f32, y: f32, rgb: [u8; 3]| {
        add_at(px, w, h, x.round() as isize, y.round() as isize, rgb);
    };
    // The graticule: rings at a quarter and a half of chroma, the
    // axes, and the skin-tone line at 123 degrees from +Cb.
    for &(r, tone) in &[(RADIUS / 2.0, 0x0au8), (RADIUS, 0x16u8)] {
        let steps = (r * 8.0) as usize + 8;
        for i in 0..steps {
            let a = i as f32 / steps as f32 * std::f32::consts::TAU;
            mark(cx + r * a.cos(), cy - r * a.sin(), [tone; 3]);
        }
    }
    let mut i = 0;
    while i < RADIUS as usize {
        let d = i as f32;
        for (x, y) in [(cx + d, cy), (cx - d, cy), (cx, cy + d), (cx, cy - d)] {
            mark(x, y, [0x0e; 3]);
        }
        i += 3;
    }
    let skin = 123.0f32.to_radians();
    for i in 0..RADIUS as usize {
        let d = i as f32;
        mark(cx + d * skin.cos(), cy - d * skin.sin(), [0x40, 0x2a, 0x1e]);
    }
    // The trace. One cell is one pixel, so nothing falls between.
    let wheel = &bins[HIST..];
    let peak = wheel.iter().copied().max().unwrap_or(1).max(1) as f32;
    for v in 0..WHEEL {
        for u in 0..WHEEL {
            let i = lit_log(wheel[v * WHEEL + u], peak);
            if i == 0.0 {
                continue;
            }
            let cb = (u as f32 + 0.5) / WHEEL as f32 - 0.5;
            let cr = 0.5 - (v as f32 + 0.5) / WHEEL as f32;
            // At its own brightness, so the intensity alone says how
            // much landed here and the color only says where.
            let c = color_of(cb, cr);
            let top = c[0].max(c[1]).max(c[2]).max(1e-3);
            let rgb = [
                (c[0] / top * i * 255.0) as u8,
                (c[1] / top * i * 255.0) as u8,
                (c[2] / top * i * 255.0) as u8,
            ];
            add_at(
                px,
                w,
                h,
                (cx + 2.0 * cb * RADIUS).round() as isize,
                (cy - 2.0 * cr * RADIUS).round() as isize,
                rgb,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture whose value rises with the column, grey.
    fn ramp(w: usize, h: usize) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let v = (i % w) as f32 / (w - 1) as f32;
                [v; 3]
            })
            .collect()
    }

    fn flat(w: usize, h: usize, c: [f32; 3]) -> Vec<[f32; 3]> {
        vec![c; w * h]
    }

    fn pixels(image: &slint::Image) -> (Vec<slint::Rgb8Pixel>, usize, usize) {
        let rgb = image.to_rgb8().unwrap();
        (
            rgb.as_slice().to_vec(),
            rgb.width() as usize,
            rgb.height() as usize,
        )
    }

    #[test]
    fn every_pixel_lands_in_a_bin() {
        let (w, h) = (64, 40);
        let image = ramp(w, h);
        for scope in Scope::ALL {
            let bins = bins(scope, &image, w, h);
            assert_eq!(bins.len(), scope.bin_count());
            let hist: u32 = bins[..HIST].iter().sum();
            assert_eq!(hist as usize, w * h * 3, "{scope:?} histogram");
            match scope.kind() {
                1 => assert_eq!(
                    bins[HIST..].iter().sum::<u32>() as usize,
                    w * h * 3,
                    "waveform"
                ),
                // A grey ramp is all on the wheel's center column.
                2 => assert_eq!(bins[HIST..].iter().sum::<u32>() as usize, w * h, "wheel"),
                _ => {}
            }
        }
    }

    #[test]
    fn the_histogram_picture_follows_the_bins() {
        // Red in the shadows, blue in the highlights, nothing else.
        let mut bins = vec![0u32; HIST];
        bins[20] = 1000;
        bins[512 + 200] = 500;
        let (px, w, _) = pixels(&draw(Scope::Rgb, &bins));
        let at = |x: usize, y: usize| px[y * w + x];
        // Column 20 is red to the top; column 200 blue to about
        // sqrt(1/2) of the height; column 100 is background.
        assert!(at(20, 1).r > 0xc0 && at(20, 1).b < 0x40);
        assert!(at(200, 79).b > 0xc0 && at(200, 79).r < 0x40);
        assert!(at(200, 10).b < 0x20, "blue does not reach the top");
        assert!(at(100, 79).r < 0x20 && at(100, 79).g < 0x20);
    }

    #[test]
    fn the_clipping_reads_the_end_bins() {
        let mut ends = vec![0u32; HIST];
        assert_eq!(Clipping::of(&ends), Clipping::default());
        assert_eq!(Clipping::mark([false; 3]), None);
        // One red pixel at the top, and green and blue at the bottom.
        ends[LEVELS - 1] = 1;
        ends[LEVELS] = 3;
        ends[2 * LEVELS] = 7;
        let c = Clipping::of(&ends);
        assert_eq!(c.highlights, [true, false, false]);
        assert_eq!(c.shadows, [false, true, true]);
        assert_eq!(Clipping::mark(c.highlights), Some([0xff, 0, 0]));
        assert_eq!(Clipping::mark(c.shadows), Some([0, 0xff, 0xff]));
        assert_eq!(Clipping::mark([true; 3]), Some([0xff; 3]));
        // The bins of a ramp reach both ends in every channel.
        let (w, h) = (256, 4);
        let ramp = bins(Scope::Rgb, &ramp(w, h), w, h);
        assert_eq!(
            Clipping::of(&ramp),
            Clipping {
                shadows: [true; 3],
                highlights: [true; 3]
            }
        );
    }

    #[test]
    fn the_waveform_draws_a_ramp_on_the_diagonal() {
        let (w, h) = (256, 64);
        let bins = bins(Scope::Waveform, &ramp(w, h), w, h);
        let (px, pw, ph) = pixels(&draw(Scope::Waveform, &bins));
        let lit = |x: usize, y: usize| px[y * pw + x].g > GROUND + 0x20;
        // Dark on the left, bright on the right, and the other way up.
        assert!(lit(0, ph - 1), "the darkest column sits at the bottom");
        assert!(!lit(0, 0));
        assert!(lit(pw - 1, 0), "the brightest column sits at the top");
        assert!(!lit(pw - 1, ph - 1));
        // A grey ramp lights all three channels together.
        let p = px[(ph - 1) * pw];
        assert!(p.r.abs_diff(p.g) < 8 && p.g.abs_diff(p.b) < 8, "{p:?}");
    }

    #[test]
    fn the_parade_gives_each_channel_a_panel() {
        let (w, h) = (128, 64);
        let bins = bins(Scope::Parade, &flat(w, h, [1.0, 0.0, 0.0]), w, h);
        let (px, pw, ph) = pixels(&draw(Scope::Parade, &bins));
        let panel = (pw - 2) / 3;
        // The panels do not divide the bins evenly, so read the
        // brightest pixel of a row rather than one column of it.
        let row = |ch: usize, y: usize| {
            let x0 = ch * (panel + 1);
            (x0..x0 + panel)
                .map(|x| match ch {
                    0 => px[y * pw + x].r,
                    1 => px[y * pw + x].g,
                    _ => px[y * pw + x].b,
                })
                .max()
                .unwrap()
        };
        // Red is clipped, so it lies along the top of the first panel;
        // green and blue are at nothing, along the bottom of theirs.
        assert!(row(0, 0) > 0xa0, "red at the top of its panel");
        assert!(row(0, ph - 1) < 0x40, "and nowhere near the bottom");
        assert!(row(1, ph - 1) > 0xa0, "green at the bottom of its panel");
        assert!(row(1, 0) < 0x40);
        assert!(row(2, ph - 1) > 0xa0, "blue at the bottom of its panel");
    }

    #[test]
    fn the_wheel_puts_grey_in_the_middle_and_red_above_it() {
        let (w, h) = (32, 32);
        let (pw, _) = Scope::Vector.size();
        let pw = pw as usize;
        let center = pw / 2;

        let grey = bins(Scope::Vector, &flat(w, h, [0.5; 3]), w, h);
        let (px, _, _) = pixels(&draw(Scope::Vector, &grey));
        let p = px[center * pw + center];
        assert!(
            p.r > 0x80 && p.g > 0x80 && p.b > 0x80,
            "grey is a bright middle: {p:?}"
        );

        let red = bins(Scope::Vector, &flat(w, h, [1.0, 0.0, 0.0]), w, h);
        let (px, _, _) = pixels(&draw(Scope::Vector, &red));
        let (x, y) = px
            .iter()
            .enumerate()
            .max_by_key(|(_, p)| p.r as u32 + p.g as u32 + p.b as u32)
            .map(|(i, _)| (i % pw, i / pw))
            .unwrap();
        let p = px[y * pw + x];
        assert!(p.r > p.g + 0x40 && p.r > p.b + 0x40, "red reads red: {p:?}");
        assert!(y < center - 80, "red is near the top: {y}");
        assert!(x < center && x > center - 40, "a little to the left: {x}");
    }

    /// A picture with something in every scope: hue across, value
    /// down.
    fn colors(w: usize, h: usize) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                let t = x * std::f32::consts::TAU;
                [
                    y * (0.5 + 0.5 * t.cos()),
                    y * (0.5 + 0.5 * (t + 2.1).cos()),
                    y * (0.5 + 0.5 * (t + 4.2).cos()),
                ]
            })
            .collect()
    }

    #[test]
    fn a_mask_of_one_everywhere_is_the_unweighted_scope() {
        let (w, h) = (97, 61);
        let image = colors(w, h);
        let ones = vec![1.0; w * h];
        for scope in Scope::ALL {
            assert_eq!(
                weighted_bins(scope, &image, &ones, w, h),
                bins(scope, &image, w, h),
                "{scope:?}"
            );
        }
    }

    #[test]
    fn a_mask_of_nothing_is_an_empty_scope() {
        let (w, h) = (64, 40);
        let image = colors(w, h);
        let none = vec![0.0; w * h];
        for scope in Scope::ALL {
            let b = weighted_bins(scope, &image, &none, w, h);
            assert_eq!(b.len(), scope.bin_count());
            assert!(b.iter().all(|&n| n == 0), "{scope:?}");
            assert_eq!(Clipping::of(&b), Clipping::default());
            // It draws, with no trace: the histogram is all ground.
            let (px, _, _) = pixels(&draw(scope, &b));
            if scope == Scope::Rgb {
                assert!(
                    px.iter()
                        .all(|p| p.r == GROUND && p.g == GROUND && p.b == GROUND)
                );
            }
        }
    }

    #[test]
    fn a_small_selection_has_the_whole_pictures_mass() {
        // The left eighth of a ramp, selected: its bins are the left
        // eighth's alone, scaled up to the whole picture's count, so
        // the scope is drawn as bright as the whole frame's.
        let (w, h) = (256, 16);
        let image = ramp(w, h);
        let coverage: Vec<f32> = (0..w * h)
            .map(|i| if i % w < w / 8 { 1.0 } else { 0.0 })
            .collect();
        for scope in Scope::ALL {
            let b = weighted_bins(scope, &image, &coverage, w, h);
            let hist: u32 = b[..HIST].iter().sum();
            assert_eq!(hist as usize, 3 * w * h, "{scope:?}");
            // Nothing from the right of the selection.
            assert!(b[level(0.5)..LEVELS].iter().all(|&n| n == 0));
            assert_eq!(b[0], 8 * h as u32, "the darkest column, eight times over");
        }
        // A fifth of a coverage counts a fifth: a picture half at 0.2
        // and half at 0.8, the dark half weighed 51 of the texture's
        // 255 steps and the bright one all of them, holds five times
        // the bright.
        let mut image = flat(w, h, [0.2; 3]);
        image[w * h / 2..].fill([0.8; 3]);
        let coverage: Vec<f32> = (0..w * h)
            .map(|i| if i < w * h / 2 { 51.0 / 255.0 } else { 1.0 })
            .collect();
        let b = weighted_bins(Scope::Rgb, &image, &coverage, w, h);
        let (dark, bright) = (b[level(0.2)], b[level(0.8)]);
        assert_eq!(dark + bright, (w * h) as u32);
        assert!(bright.abs_diff(5 * dark) <= 5, "{dark} {bright}");
    }

    #[test]
    fn a_faint_edge_does_not_lift_a_scope_cell() {
        // A grey field wholly selected, and one saturated mid-level
        // pixel at the lowest coverage step: it rounds to nothing in
        // the waveform and on the wheel, as the rounding says, rather
        // than being lifted to a count that draws.
        let (w, h) = (128, 128);
        let mut image = flat(w, h, [0.5; 3]);
        let odd = [0.7, 0.3, 0.4];
        image[w * h / 2 + w / 2] = odd;
        let mut coverage = vec![1.0; w * h];
        coverage[w * h / 2 + w / 2] = 1.0 / 255.0;
        let wave = weighted_bins(Scope::Waveform, &image, &coverage, w, h);
        let column = (w / 2) * COLUMNS / w;
        let at = HIST + column * LEVELS + level(odd[0]);
        assert_eq!(wave[at], 0, "the red level of the faint pixel");
        let wheel = weighted_bins(Scope::Vector, &image, &coverage, w, h);
        let cell = wheel_cell(odd).unwrap();
        assert_ne!(Some(cell), wheel_cell([0.5; 3]));
        assert_eq!(wheel[HIST + cell], 0, "the faint pixel's chroma");
        // And the mass is the whole picture's less the faint pixel's
        // three rounded-away levels, not lifted by it.
        let mass: u32 = wave[HIST..].iter().sum();
        assert_eq!(mass as usize, 3 * w * h - 3);
    }

    #[test]
    fn a_pixel_at_the_edge_of_a_mask_still_lands() {
        // One clipped pixel at the lowest coverage the texture holds,
        // beside a fully selected field: it is a count still, and the
        // clipping mark lights for it.
        let (w, h) = (128, 128);
        let mut image = flat(w, h, [0.5; 3]);
        image[0] = [1.0, 0.5, 0.5];
        let mut coverage = vec![1.0; w * h];
        coverage[0] = 1.0 / 255.0;
        let b = weighted_bins(Scope::Rgb, &image, &coverage, w, h);
        assert_eq!(b[LEVELS - 1], 1);
        assert!(Clipping::of(&b).highlights[0]);
        // And below the texture's first step it is not in the mask.
        coverage[0] = 0.4 / 255.0;
        let b = weighted_bins(Scope::Rgb, &image, &coverage, w, h);
        assert_eq!(b[LEVELS - 1], 0);
    }

    #[test]
    fn the_names_round_trip() {
        for scope in Scope::ALL {
            assert_eq!(Scope::from_name(scope.name()), Some(scope));
        }
        assert_eq!(Scope::from_name("Nope"), None);
    }
}
