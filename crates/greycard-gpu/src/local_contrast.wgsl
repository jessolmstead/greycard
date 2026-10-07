// The local contrast (Texture and Clarity) on the GPU: the passes of
// `greycard_core::develop::local_contrast`, one kernel each, on
// planes of R32Float. The constants come in through the uniform from
// the reference's own, so the two cannot drift apart.
//
// Every box pass is separable, one kernel along the rows and one
// down the columns, each reading its window whole from workgroup
// memory and dividing by what is inside the picture, as the
// reference's running sums do; never a summed-area table, whose
// totals over a frame lose the precision a variance against an
// epsilon of 0.25 needs. The kernels take their source as a sampled
// texture and their destination as a storage one, so one pipeline
// serves every pair of planes, full size or grid, through a bind
// group a pair.

struct Params {
    // The plane the pass works on: the picture's size, or the grid's.
    size: vec2<u32>,
    // A box pass: its radius, and whether to sum the squares.
    radius: u32,
    square: u32,
    // A guided filter's epsilon and the band's gain times the slider.
    epsilon: f32,
    k: f32,
    // The develop's clip level, and whether there is one.
    clip_level: f32,
    has_clip: u32,
    // The coarse grid: its size and its block's side in pixels.
    grid: vec2<u32>,
    step: u32,
    // The apply: TEXTURE, CLARITY and COARSE below.
    mode: u32,
    // The luminance weights, and mid grey in w.
    luma: vec4<f32>,
    // Clarity's fades in stops: shadow from and to, highlight from and to.
    fade: vec4<f32>,
    // The luminance floor, and where the clip fade starts as a fraction
    // of the clip level.
    floor: f32,
    clip_fade: f32,
    pad: vec2<u32>,
}

const TEXTURE: u32 = 1u;
const CLARITY: u32 = 2u;
const COARSE: u32 = 4u;

@group(0) @binding(0) var<uniform> p: Params;

fn smoothstep_(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// How far the gain is faded toward the clip on a pixel's brightest
// channel: one with no clip level.
fn clip_weight(px: vec3<f32>) -> f32 {
    if p.has_clip == 0u {
        return 1.0;
    }
    let brightest = max(px.x, max(px.y, px.z));
    return 1.0 - smoothstep_(p.clip_level * p.clip_fade, p.clip_level, brightest);
}

// Clarity's weight at a base luminance in stops about mid grey.
fn midtone_weight(stops: f32) -> f32 {
    return smoothstep_(p.fade.x, p.fade.y, stops) * (1.0 - smoothstep_(p.fade.z, p.fade.w, stops));
}

// The bindings, numbered once for the module; each kernel's layout
// is the subset it uses.
//
// A pass's one sampled source and its storage destinations.
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<r32float, write>;
@group(0) @binding(3) var dst2: texture_storage_2d<r32float, write>;
@group(0) @binding(4) var dst3: texture_storage_2d<r32float, write>;
// The coefficient passes' two planes in place, and the grid's
// weights beside them.
@group(0) @binding(5) var mean: texture_storage_2d<r32float, read_write>;
@group(0) @binding(6) var sq: texture_storage_2d<r32float, read_write>;
@group(0) @binding(7) var held: texture_2d<f32>;
@group(0) @binding(8) var share: texture_2d<f32>;
// What the gain and the apply read: the picture, the log luminance
// (Clarity's band's top, low-passed, by the apply), the exact
// filter's averaged coefficients, Texture's gain, the grid's averaged
// coefficients.
@group(0) @binding(9) var pic: texture_2d<f32>;
@group(0) @binding(10) var log_lum_in: texture_2d<f32>;
@group(0) @binding(11) var intercept: texture_2d<f32>;
@group(0) @binding(12) var slope: texture_2d<f32>;
@group(0) @binding(13) var gain: texture_2d<f32>;
@group(0) @binding(14) var grid_intercept: texture_2d<f32>;
@group(0) @binding(15) var grid_slope: texture_2d<f32>;
// The apply's outputs: the viewport's half floats, or full floats.
@group(0) @binding(16) var out_half: texture_storage_2d<rgba16float, write>;
@group(0) @binding(17) var out_full: texture_storage_2d<rgba32float, write>;

// ---- The log luminance: the picture (`src`) into a plane (`dst`).

@compute @workgroup_size(16, 16, 1)
fn log_lum(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let px = textureLoad(src, at, 0).rgb;
    let y = p.luma.x * px.x + p.luma.y * px.y + p.luma.z * px.z;
    let l = log2(max(y, p.floor) / p.luma.w);
    textureStore(dst, at, vec4<f32>(l, 0.0, 0.0, 0.0));
}

// ---- The box mean, along the rows and down the columns, `src` to
// `dst`.

// A row pass works a run of ROW_TILE pixels a workgroup, with the
// window's reach each side of it in workgroup memory; a column pass a
// tile of COL_W by COL_H. The reach is bounded by MAX_RADIUS, which
// the host checks: the grid's radius on a 16384-pixel frame is 102.
const MAX_RADIUS: u32 = 128u;
const ROW_TILE: u32 = 256u;
const COL_W: u32 = 8u;
const COL_H: u32 = 32u;

var<workgroup> line: array<f32, 512>;
var<workgroup> column: array<f32, 2304>;

fn source(x: i32, y: i32) -> f32 {
    var v = textureLoad(src, vec2<i32>(x, y), 0).r;
    if p.square != 0u {
        v = v * v;
    }
    return v;
}

@compute @workgroup_size(256, 1, 1)
fn box_rows(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let w = i32(p.size.x);
    let y = i32(wid.y);
    let x0 = i32(wid.x * ROW_TILE);
    let r = i32(p.radius);
    let reach = ROW_TILE + 2u * p.radius;
    for (var i = lid.x; i < reach; i += ROW_TILE) {
        let x = x0 - r + i32(i);
        var v = 0.0;
        if x >= 0 && x < w {
            v = source(x, y);
        }
        line[i] = v;
    }
    workgroupBarrier();
    let x = x0 + i32(lid.x);
    if x >= w {
        return;
    }
    let lo = max(x - r, 0);
    let hi = min(x + r, w - 1);
    var sum = 0.0;
    for (var xx = lo; xx <= hi; xx++) {
        sum += line[xx - x0 + r];
    }
    textureStore(dst, vec2<i32>(x, y), vec4<f32>(sum / f32(hi - lo + 1), 0.0, 0.0, 0.0));
}

@compute @workgroup_size(8, 32, 1)
fn box_cols(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let w = i32(p.size.x);
    let h = i32(p.size.y);
    let x0 = i32(wid.x * COL_W);
    let y0 = i32(wid.y * COL_H);
    let r = i32(p.radius);
    let rows = COL_H + 2u * p.radius;
    let tid = lid.y * COL_W + lid.x;
    for (var i = tid; i < rows * COL_W; i += COL_W * COL_H) {
        let x = x0 + i32(i % COL_W);
        let y = y0 - r + i32(i / COL_W);
        var v = 0.0;
        if x < w && y >= 0 && y < h {
            v = source(x, y);
        }
        column[i] = v;
    }
    workgroupBarrier();
    let x = x0 + i32(lid.x);
    let y = y0 + i32(lid.y);
    if x >= w || y >= h {
        return;
    }
    let lo = max(y - r, 0);
    let hi = min(y + r, h - 1);
    var sum = 0.0;
    for (var yy = lo; yy <= hi; yy++) {
        sum += column[u32(yy - y0 + r) * COL_W + lid.x];
    }
    // As the reference scales its columns: by the reciprocal.
    let scale = 1.0 / f32(hi - lo + 1);
    textureStore(dst, vec2<i32>(x, y), vec4<f32>(sum * scale, 0.0, 0.0, 0.0));
}

// ---- The guided filter's line in every window, from the means.

// `mean` the window mean, `sq` the mean of the squares, in place:
// `sq` becomes the slope and `mean` the intercept.

@compute @workgroup_size(16, 16, 1)
fn coefficients(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let m = textureLoad(mean, at).r;
    let q = textureLoad(sq, at).r;
    let variance = max(q - m * m, 0.0);
    let slope = variance / (variance + p.epsilon);
    textureStore(sq, at, vec4<f32>(slope, 0.0, 0.0, 0.0));
    textureStore(mean, at, vec4<f32>(m - slope * m, 0.0, 0.0, 0.0));
}

// ---- Texture's gain, from its averaged coefficients, into `dst`.

@compute @workgroup_size(16, 16, 1)
fn texture_gain(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let u = textureLoad(log_lum_in, at, 0).r;
    let base = textureLoad(intercept, at, 0).r + textureLoad(slope, at, 0).r * u;
    let px = textureLoad(pic, at, 0).rgb;
    let g = exp2(clip_weight(px) * p.k * (u - base));
    textureStore(dst, at, vec4<f32>(g, 0.0, 0.0, 0.0));
}

// ---- The coarse grid: block sums, and the filter's stages on them.

// Each block's sums of the plane and its square over a full block's
// count, and its share of a full block's pixels: a block cut short by
// the picture's edge counts for what it holds. `src` (the picture's
// size) into `dst`, `dst2` and `dst3` (the grid's).

@compute @workgroup_size(16, 16, 1)
fn block_sums(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.grid.x || gid.y >= p.grid.y {
        return;
    }
    let step = p.step;
    let x0 = gid.x * step;
    let y0 = gid.y * step;
    let x1 = min(x0 + step, p.size.x);
    let y1 = min(y0 + step, p.size.y);
    var m = 0.0;
    var q = 0.0;
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            let v = textureLoad(src, vec2<i32>(i32(x), i32(y)), 0).r;
            m += v;
            q += v * v;
        }
    }
    let area = f32(step * step);
    let at = vec2<i32>(gid.xy);
    textureStore(dst, at, vec4<f32>(m / area, 0.0, 0.0, 0.0));
    textureStore(dst2, at, vec4<f32>(q / area, 0.0, 0.0, 0.0));
    let share = f32((y1 - y0) * (x1 - x0)) / area;
    textureStore(dst3, at, vec4<f32>(share, 0.0, 0.0, 0.0));
}

// The line in every window of the grid: `mean` and `sq` the box
// means over the grid, `held` a window's pixels as a share of its
// blocks (what divides those into means over pixels), `share` the
// block's own. In place: `sq` the slope and `mean` the intercept,
// each weighted by the block's share for the average over the
// windows to come.

@compute @workgroup_size(16, 16, 1)
fn grid_coefficients(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let h = textureLoad(held, at, 0).r;
    let s = textureLoad(share, at, 0).r;
    let m = textureLoad(mean, at).r / h;
    let q = textureLoad(sq, at).r / h;
    let variance = max(q - m * m, 0.0);
    let slope = variance / (variance + p.epsilon);
    textureStore(sq, at, vec4<f32>(slope * s, 0.0, 0.0, 0.0));
    textureStore(mean, at, vec4<f32>((m - slope * m) * s, 0.0, 0.0, 0.0));
}

// The averaged slope and intercept over the pixels their windows hold.
@compute @workgroup_size(16, 16, 1)
fn grid_divide(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let h = textureLoad(held, at, 0).r;
    textureStore(sq, at, vec4<f32>(textureLoad(sq, at).r / h, 0.0, 0.0, 0.0));
    textureStore(mean, at, vec4<f32>(textureLoad(mean, at).r / h, 0.0, 0.0, 0.0));
}

// ---- The apply: both gains onto the picture.

// A block's center along an axis: the middle of what is inside.
fn center(b: i32, len: i32, step: i32) -> f32 {
    return f32(b * step) + f32(min(step, len - b * step) - 1) / 2.0;
}

struct Tap {
    i0: i32,
    i1: i32,
    t: f32,
}

// The two blocks whose centers a pixel lies between and how far it
// is from the first to the second, clamped to the first and the last
// block's centers, as the reference's `bilinear_taps`.
fn tap_at(x: i32, len: i32, step: i32) -> Tap {
    let blocks = (len + step - 1) / step;
    let xf = f32(x);
    var b = x / step;
    if xf < center(b, len, step) {
        b -= 1;
    }
    if b < 0 {
        return Tap(0, 0, 0.0);
    }
    if b + 1 >= blocks || xf <= center(b, len, step) {
        return Tap(b, b, 0.0);
    }
    let c0 = center(b, len, step);
    let c1 = center(b + 1, len, step);
    return Tap(b, b + 1, (xf - c0) / (c1 - c0));
}

// A grid plane at a pixel: its two rows blended down the column,
// then the two blocks along the row, in the reference's order.
fn from_grid(g: texture_2d<f32>, tx: Tap, ty: Tap) -> f32 {
    let a0 = textureLoad(g, vec2<i32>(tx.i0, ty.i0), 0).r;
    let a1 = textureLoad(g, vec2<i32>(tx.i0, ty.i1), 0).r;
    let b0 = textureLoad(g, vec2<i32>(tx.i1, ty.i0), 0).r;
    let b1 = textureLoad(g, vec2<i32>(tx.i1, ty.i1), 0).r;
    let a = a0 + ty.t * (a1 - a0);
    let b = b0 + ty.t * (b1 - b0);
    return a + tx.t * (b - a);
}

fn applied(at: vec2<i32>) -> vec3<f32> {
    var px = textureLoad(pic, at, 0).rgb;
    if (p.mode & TEXTURE) != 0u {
        px *= textureLoad(gain, at, 0).r;
    }
    if (p.mode & CLARITY) != 0u {
        let u = textureLoad(log_lum_in, at, 0).r;
        var base: f32;
        if (p.mode & COARSE) != 0u {
            let step = i32(p.step);
            let tx = tap_at(at.x, i32(p.size.x), step);
            let ty = tap_at(at.y, i32(p.size.y), step);
            base = from_grid(grid_slope, tx, ty) * u + from_grid(grid_intercept, tx, ty);
        } else {
            base = textureLoad(intercept, at, 0).r + textureLoad(slope, at, 0).r * u;
        }
        px *= exp2(clip_weight(px) * (p.k * midtone_weight(base)) * (u - base));
    }
    return px;
}

// Into the viewport's texture, the alpha zero: no mask to paint.
@compute @workgroup_size(16, 16, 1)
fn apply_half(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    textureStore(out_half, at, vec4<f32>(applied(at), 0.0));
}

// Into full floats for the sharpen or a read back, the alpha one as
// the upload has it.
@compute @workgroup_size(16, 16, 1)
fn apply_full(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    textureStore(out_full, at, vec4<f32>(applied(at), 1.0));
}
