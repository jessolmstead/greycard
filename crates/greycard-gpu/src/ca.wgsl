// The lateral chromatic aberration correction's data-parallel stages,
// after RawTherapee's `rtengine/CA_correct_RT.cc` (copyright 2008-2010
// Emil Martinec; 2018 Ingo Weyrich for the iterated correction and the
// color-shift avoidance; GPL-3.0-or-later), as greycard-core's port of
// it has them (`greycard_core::develop::ca`, the reference here).
//
// On the mosaic: green interpolated at every red and blue site, the
// tiles' quadratic-fit sums, the resample by the fitted shifts, and
// the color-shift guard's factors, box blurs and multiply. The
// constants are `greycard_core::develop::ca`'s, and every expression
// takes the reference's operations in the reference's order. What the
// reference decides between these stages (the median, the gate, the
// polynomial fit, the solve) is read back and decided on the CPU with
// the reference's own functions.
//
// The reference works in tiles of 128 with a border of 8 it computes
// and throws away, reflecting reads past the picture's edge. Every
// value a tile's interior reads is one the picture alone determines
// (the interpolated green within four pixels of the interior, which
// the border always covers), so the green lives in one plane padded
// by the border, the interior of every tile is a disjoint 112-pixel
// block of the picture, and no atlas is needed: the vote is a
// workgroup a tile over its block, and the resample a thread a pixel
// with its tile's parameters.

struct Params {
    // The mosaic's size.
    size: vec2<u32>,
    // The factor planes' size: the mosaic's halved, rounded up.
    half: vec2<u32>,
    // Tiles across and down.
    tiles: vec2<u32>,
    // The box blur's radius, and 1 / (2 radius + 1).
    radius: u32,
    norm: f32,
    // The color at each parity (row & 1, col & 1), in the order
    // (0,0) (0,1) (1,0) (1,1): 0 red, 1 green, 2 blue.
    colors: vec4<u32>,
    // A blur pass: the plane (0 red, 1 blue), whether it reads the
    // scratch and writes the plane (else the other way), and its
    // axis (0 along the rows, 1 down the columns).
    plane: u32,
    from_tmp: u32,
    axis: u32,
    pad0: u32,
}

// One tile's resample parameters, per color (red, blue): the
// reference's `Resample`.
struct TileResample {
    vfloor: vec2<i32>,
    vceil: vec2<i32>,
    hfloor: vec2<i32>,
    hceil: vec2<i32>,
    vfrac: vec2<f32>,
    hfrac: vec2<f32>,
    dir_v: vec2<i32>,
    dir_h: vec2<i32>,
}

@group(0) @binding(0) var<uniform> p: Params;
// The mosaic as uploaded, for the color-shift guard.
@group(0) @binding(1) var original: texture_2d<f32>;
// The mosaic a pass reads: the upload, or the last pass's result.
@group(0) @binding(2) var cur: texture_2d<f32>;
// The pass's result; in the guard, the mosaic it multiplies.
@group(0) @binding(3) var next: texture_storage_2d<r32float, read_write>;
// Green at every site, padded by the border on every side.
@group(0) @binding(4) var green: texture_storage_2d<r32float, read_write>;
// The color-shift factors, red and blue, and the blur's scratch.
@group(0) @binding(5) var factor_r: texture_storage_2d<r32float, read_write>;
@group(0) @binding(6) var factor_b: texture_storage_2d<r32float, read_write>;
@group(0) @binding(7) var tmp: texture_storage_2d<r32float, read_write>;
// Per tile, the twelve sums: [direction][term][color].
@group(0) @binding(8) var<storage, read_write> sums: array<f32>;
// Per tile, its resample parameters.
@group(0) @binding(9) var<storage, read> resample: array<TileResample>;

const TS: i32 = 128;
const BORDER: i32 = 8;
const STEP: i32 = 112;
const EPS: f32 = 1.0e-5;
// Rows of a tile's interior a vote chunk covers, and the red or blue
// sites in them: 56 a row.
const CHUNK_ROWS: i32 = 4;
const CHUNK_SITES: u32 = 224u;

fn width() -> i32 {
    return i32(p.size.x);
}

fn height() -> i32 {
    return i32(p.size.y);
}

// The reference's `reflect`: a read past an edge mirrors about the
// edge pixel.
fn reflect(i: i32, n: i32) -> i32 {
    var r = i;
    if i < 0 {
        r = -i;
    } else if i >= n {
        r = 2 * n - 2 - i;
    }
    return clamp(r, 0, n - 1);
}

// The current mosaic at a position, reflected.
fn cfa(y: i32, x: i32) -> f32 {
    return textureLoad(cur, vec2<i32>(reflect(x, width()), reflect(y, height())), 0).r;
}

// The padded green plane at a picture position.
fn g(y: i32, x: i32) -> f32 {
    return textureLoad(green, vec2<i32>(x + BORDER, y + BORDER)).r;
}

// 0 red, 1 green, 2 blue at a picture position; the tiles start on
// even rows and columns, so a tile's parity is the picture's, and a
// negative position's parity is its two's complement bit.
fn color_at(y: i32, x: i32) -> u32 {
    return p.colors[u32(y & 1) * 2u + u32(x & 1)];
}

fn is_green(y: i32, x: i32) -> bool {
    return color_at(y, x) == 1u;
}

// The reference's color index: 0 red, 1 blue.
fn color_index(y: i32, x: i32) -> u32 {
    return select(1u, 0u, color_at(y, x) == 0u);
}

fn sq(v: f32) -> f32 {
    return v * v;
}

// `a * b + (1 - a) * c`, the reference's `intp`.
fn intp(a: f32, b: f32, c: f32) -> f32 {
    return a * (b - c) + c;
}

// Green at every position of the padded plane: the mosaic where it is
// green, else the reference's directionally weighted interpolation of
// the four green neighbors.
@compute @workgroup_size(16, 16, 1)
fn interpolate_green(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = width();
    let h = height();
    if i32(gid.x) >= w + 2 * BORDER || i32(gid.y) >= h + 2 * BORDER {
        return;
    }
    let x = i32(gid.x) - BORDER;
    let y = i32(gid.y) - BORDER;
    var v: f32;
    if is_green(y, x) {
        v = cfa(y, x);
    } else {
        // Green neighbors are one step away, same-color two.
        let wtu = 1.0
            / sq(EPS
                + abs(cfa(y + 1, x) - cfa(y - 1, x))
                + abs(cfa(y, x) - cfa(y - 2, x))
                + abs(cfa(y - 1, x) - cfa(y - 3, x)));
        let wtd = 1.0
            / sq(EPS
                + abs(cfa(y - 1, x) - cfa(y + 1, x))
                + abs(cfa(y, x) - cfa(y + 2, x))
                + abs(cfa(y + 1, x) - cfa(y + 3, x)));
        let wtl = 1.0
            / sq(EPS
                + abs(cfa(y, x + 1) - cfa(y, x - 1))
                + abs(cfa(y, x) - cfa(y, x - 2))
                + abs(cfa(y, x - 1) - cfa(y, x - 3)));
        let wtr = 1.0
            / sq(EPS
                + abs(cfa(y, x - 1) - cfa(y, x + 1))
                + abs(cfa(y, x) - cfa(y, x + 2))
                + abs(cfa(y, x + 1) - cfa(y, x + 3)));
        v = (wtu * cfa(y - 1, x) + wtd * cfa(y + 1, x) + wtl * cfa(y, x - 1) + wtr * cfa(y, x + 1))
            / (wtu + wtd + wtl + wtr);
    }
    textureStore(green, vec2<i32>(gid.xy), vec4<f32>(v, 0.0, 0.0, 0.0));
}

// The color difference, green less the mosaic, at a red or blue site.
fn d(y: i32, x: i32) -> f32 {
    return g(y, x) - cfa(y, x);
}

// The reference's high-pass filters of the color difference along
// each axis at a red or blue site.
fn rbhpfv(y: i32, x: i32) -> f32 {
    return abs(abs(d(y, x) - d(y + 4, x)) + abs(d(y - 4, x) - d(y, x)) - abs(d(y - 4, x) - d(y + 4, x)));
}

fn rbhpfh(y: i32, x: i32) -> f32 {
    return abs(abs(d(y, x) - d(y, x + 4)) + abs(d(y, x - 4) - d(y, x)) - abs(d(y, x - 4) - d(y, x + 4)));
}

// The reference's low-pass filters: the difference and the sum of the
// green and the mosaic low-passed along each axis.
fn glpfv(y: i32, x: i32) -> f32 {
    return 2.0 * g(y, x) + g(y + 2, x) + g(y - 2, x);
}

fn glpfh(y: i32, x: i32) -> f32 {
    return 2.0 * g(y, x) + g(y, x + 2) + g(y, x - 2);
}

fn clpfv(y: i32, x: i32) -> f32 {
    return 2.0 * cfa(y, x) + cfa(y + 2, x) + cfa(y - 2, x);
}

fn clpfh(y: i32, x: i32) -> f32 {
    return 2.0 * cfa(y, x) + cfa(y, x + 2) + cfa(y, x - 2);
}

fn rblpfv(y: i32, x: i32) -> f32 {
    return 0.25 * abs(glpfv(y, x) - clpfv(y, x));
}

fn rblpfh(y: i32, x: i32) -> f32 {
    return 0.25 * abs(glpfh(y, x) - clpfh(y, x));
}

fn grblpfv(y: i32, x: i32) -> f32 {
    return 0.25 * (glpfv(y, x) + clpfv(y, x));
}

fn grblpfh(y: i32, x: i32) -> f32 {
    return 0.25 * (glpfh(y, x) + clpfh(y, x));
}

// A vote chunk's per-site terms, six a site: the vertical
// direction's three, then the horizontal's.
var<workgroup> contrib: array<f32, 1344>;

// A tile's twelve sums, a workgroup a tile: the reference's
// quadratic-fit coefficients of color-difference variance against
// interpolation position, accumulated in single precision in the
// reference's row-major order over the tile's interior, the greens
// skipped. Each chunk of four interior rows has its sites' terms made
// by the workgroup, then added by one thread in order, so the sum's
// rounding is the reference's: the terms are rounded to single before
// the add, as they are there, and the adds run in sequence.
@compute @workgroup_size(256, 1, 1)
fn vote(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let tile = wid.y * p.tiles.x + wid.x;
    let top = i32(wid.y) * STEP - BORDER;
    let left = i32(wid.x) * STEP - BORDER;
    let rr1 = min(TS, height() + BORDER - top);
    let cc1 = min(TS, width() + BORDER - left);
    let rows_end = rr1 - BORDER;
    let cols_end = cc1 - BORDER;
    // [direction][term][color].
    var acc: array<f32, 12>;
    for (var i = 0u; i < 12u; i++) {
        acc[i] = 0.0;
    }
    let t = lid.x;
    for (var r0 = BORDER; r0 < rows_end; r0 += CHUNK_ROWS) {
        if t < CHUNK_SITES {
            let rr = r0 + i32(t / 56u);
            let y = top + rr;
            // The row's red or blue sites: the parity that is not
            // green, from the interior's first column.
            let q = select(0, 1, is_green(y, left + BORDER));
            let cc = BORDER + q + 2 * i32(t % 56u);
            let x = left + cc;
            var terms: array<f32, 6>;
            if rr < rows_end && cc < cols_end {
                let deltgrb = cfa(y, x) - g(y, x);
                let gdiff_v = (g(y + 1, x) - g(y - 1, x))
                    + 0.3 * (g(y + 1, x + 1) - g(y - 1, x + 1) + g(y + 1, x - 1) - g(y - 1, x - 1));
                let gradwt_v = (rbhpfv(y, x) + 0.5 * (rbhpfv(y, x + 2) + rbhpfv(y, x - 2)))
                    * (grblpfv(y - 2, x) + grblpfv(y + 2, x))
                    / (EPS + 0.1 * (grblpfv(y - 2, x) + grblpfv(y + 2, x)) + rblpfv(y - 2, x) + rblpfv(y + 2, x));
                terms[0] = gradwt_v * deltgrb * deltgrb;
                terms[1] = gradwt_v * gdiff_v * deltgrb;
                terms[2] = gradwt_v * gdiff_v * gdiff_v;
                let gdiff_h = (g(y, x + 1) - g(y, x - 1))
                    + 0.3 * (g(y + 1, x + 1) - g(y + 1, x - 1) + g(y - 1, x + 1) - g(y - 1, x - 1));
                let gradwt_h = (rbhpfh(y, x) + 0.5 * (rbhpfh(y + 2, x) + rbhpfh(y - 2, x)))
                    * (grblpfh(y, x - 2) + grblpfh(y, x + 2))
                    / (EPS + 0.1 * (grblpfh(y, x - 2) + grblpfh(y, x + 2)) + rblpfh(y, x - 2) + rblpfh(y, x + 2));
                terms[3] = gradwt_h * deltgrb * deltgrb;
                terms[4] = gradwt_h * gdiff_h * deltgrb;
                terms[5] = gradwt_h * gdiff_h * gdiff_h;
            } else {
                for (var i = 0u; i < 6u; i++) {
                    terms[i] = 0.0;
                }
            }
            for (var i = 0u; i < 6u; i++) {
                contrib[t * 6u + i] = terms[i];
            }
        }
        workgroupBarrier();
        if t == 0u {
            for (var s = 0u; s < CHUNK_SITES; s++) {
                let rr = r0 + i32(s / 56u);
                let y = top + rr;
                let q = select(0, 1, is_green(y, left + BORDER));
                let cc = BORDER + q + 2 * i32(s % 56u);
                if rr < rows_end && cc < cols_end {
                    let k = color_index(y, left + cc);
                    acc[0u + k] += contrib[s * 6u + 0u];
                    acc[2u + k] += contrib[s * 6u + 1u];
                    acc[4u + k] += contrib[s * 6u + 2u];
                    acc[6u + k] += contrib[s * 6u + 3u];
                    acc[8u + k] += contrib[s * 6u + 4u];
                    acc[10u + k] += contrib[s * 6u + 5u];
                }
            }
        }
        workgroupBarrier();
    }
    if t == 0u {
        for (var i = 0u; i < 12u; i++) {
            sums[tile * 12u + i] = acc[i];
        }
    }
}

// Green at a red or blue site's optical position, by the tile's
// bilinear parameters for its color.
fn gint(y: i32, x: i32, r: TileResample, c: u32) -> f32 {
    let vf = r.vfloor[c];
    let vc = r.vceil[c];
    let hf = r.hfloor[c];
    let hc = r.hceil[c];
    let hfrac = r.hfrac[c];
    let gint_floor = intp(hfrac, g(y + vf, x + hc), g(y + vf, x + hf));
    let gint_ceil = intp(hfrac, g(y + vc, x + hc), g(y + vc, x + hf));
    return intp(r.vfrac[c], gint_ceil, gint_floor);
}

// The reference's `correct` at every pixel: red and blue resampled
// by their tile's fitted shift with the reference's guards, green
// carried, everything kept at or above zero.
@compute @workgroup_size(16, 16, 1)
fn correct(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let at = vec2<i32>(x, y);
    var v = cfa(y, x);
    if !is_green(y, x) {
        let c = color_index(y, x);
        let tile = (gid.y / u32(STEP)) * p.tiles.x + gid.x / u32(STEP);
        let r = resample[tile];
        let dv = r.dir_v[c];
        let dh = r.dir_h[c];
        // The color difference at the shifted green, here and at the
        // three same-color neighbors toward the shift.
        let gs00 = gint(y, x, r, c);
        let gs01 = gint(y, x - dh, r, c);
        let gs10 = gint(y - dv, x, r, c);
        let gs11 = gint(y - dv, x - dh, r, c);
        let gd00 = gs00 - cfa(y, x);
        let gd01 = gs01 - cfa(y, x - dh);
        let gd10 = gs10 - cfa(y - dv, x);
        let gd11 = gs11 - cfa(y - dv, x - dh);
        let hfrac = r.hfrac[c] * 0.5;
        let vfrac = r.vfrac[c] * 0.5;
        let g0 = g(y, x);
        let old = g0 - v;
        let h_floor = intp(hfrac, gd01, gd00);
        let h_ceil = intp(hfrac, gd11, gd10);
        var intv = intp(vfrac, h_ceil, h_floor);
        let rbint = g0 - intv;
        if abs(rbint - v) < 0.25 * (rbint + v) {
            if abs(old) > abs(intv) {
                v = rbint;
            }
        } else {
            // Weight by how far green at the shifted points is from
            // green here.
            let p0 = 1.0 / (EPS + abs(g0 - gs00));
            let p1 = 1.0 / (EPS + abs(g0 - gs01));
            let p2 = 1.0 / (EPS + abs(g0 - gs10));
            let p3 = 1.0 / (EPS + abs(g0 - gs11));
            intv = (p0 * gd00 + p1 * gd01 + p2 * gd10 + p3 * gd11) / (p0 + p1 + p2 + p3);
            if abs(old) > abs(intv) {
                v = g0 - intv;
            }
        }
        // Overshot the correction: desaturate instead.
        if old * intv < 0.0 {
            v = g0 - 0.5 * (old + intv);
        }
    }
    textureStore(next, at, vec4<f32>(max(v, 0.0), 0.0, 0.0, 0.0));
}

// The reference's `shift_factor`: the old value over the new, within
// a stop either way, or one where either is too small to say.
fn shift_factor(old: f32, now: f32) -> f32 {
    let tiny = 1.0 / 65535.0;
    if now <= tiny || old <= tiny {
        return 1.0;
    }
    return clamp(old / now, 0.5, 2.0);
}

// The color-shift factors at half resolution: each 2x2 of the mosaic
// has one red and one blue site, and a site off the picture (an odd
// width or height) leaves the reference's initial one.
@compute @workgroup_size(16, 16, 1)
fn factors(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.half.x || gid.y >= p.half.y {
        return;
    }
    var fr = 1.0;
    var fb = 1.0;
    for (var dy = 0; dy < 2; dy++) {
        for (var dx = 0; dx < 2; dx++) {
            let y = i32(gid.y) * 2 + dy;
            let x = i32(gid.x) * 2 + dx;
            if y >= height() || x >= width() {
                continue;
            }
            let color = color_at(y, x);
            if color == 1u {
                continue;
            }
            let at = vec2<i32>(x, y);
            let f = shift_factor(textureLoad(original, at, 0).r, textureLoad(next, at).r);
            if color == 0u {
                fr = f;
            } else {
                fb = f;
            }
        }
    }
    let at = vec2<i32>(gid.xy);
    textureStore(factor_r, at, vec4<f32>(fr, 0.0, 0.0, 0.0));
    textureStore(factor_b, at, vec4<f32>(fb, 0.0, 0.0, 0.0));
}

// A blur pass's source at the `i`th sample of line `line`, clamped to
// the line.
fn blur_src(line: i32, i: i32, n: i32) -> f32 {
    let k = clamp(i, 0, n - 1);
    var at: vec2<i32>;
    if p.axis == 0u {
        at = vec2<i32>(k, line);
    } else {
        at = vec2<i32>(line, k);
    }
    if p.from_tmp != 0u {
        return textureLoad(tmp, at).r;
    }
    if p.plane == 0u {
        return textureLoad(factor_r, at).r;
    }
    return textureLoad(factor_b, at).r;
}

fn blur_store(line: i32, i: i32, v: f32) {
    var at: vec2<i32>;
    if p.axis == 0u {
        at = vec2<i32>(i, line);
    } else {
        at = vec2<i32>(line, i);
    }
    let value = vec4<f32>(v, 0.0, 0.0, 0.0);
    if p.from_tmp != 0u {
        if p.plane == 0u {
            textureStore(factor_r, at, value);
        } else {
            textureStore(factor_b, at, value);
        }
    } else {
        textureStore(tmp, at, value);
    }
}

// One box blur pass over every line of a factor plane, a thread a
// line, by the reference's running sum: the window's sum once, then a
// sample in and a sample out per step. The same adds in the same
// order as the reference's `box_blur_line`, so the rounding is its.
@compute @workgroup_size(64, 1, 1)
fn blur_lines(@builtin(global_invocation_id) gid: vec3<u32>) {
    var lines: i32;
    var n: i32;
    if p.axis == 0u {
        lines = i32(p.half.y);
        n = i32(p.half.x);
    } else {
        lines = i32(p.half.x);
        n = i32(p.half.y);
    }
    let line = i32(gid.x);
    if line >= lines {
        return;
    }
    let radius = i32(p.radius);
    var sum = 0.0;
    for (var k = -radius; k <= radius; k++) {
        sum += blur_src(line, k, n);
    }
    for (var i = 0; i < n; i++) {
        blur_store(line, i, sum * p.norm);
        sum += blur_src(line, i + radius + 1, n) - blur_src(line, i - radius, n);
    }
}

// The corrected mosaic's red and blue sites multiplied by their
// blurred factors.
@compute @workgroup_size(16, 16, 1)
fn apply_factors(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let color = color_at(y, x);
    if color == 1u {
        return;
    }
    let at = vec2<i32>(x, y);
    let half = vec2<i32>(x / 2, y / 2);
    var f: f32;
    if color == 0u {
        f = textureLoad(factor_r, half).r;
    } else {
        f = textureLoad(factor_b, half).r;
    }
    let v = textureLoad(next, at).r * f;
    textureStore(next, at, vec4<f32>(v, 0.0, 0.0, 0.0));
}
