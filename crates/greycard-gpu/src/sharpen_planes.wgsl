// The capture sharpening's whole-picture passes: luminance, L* and the
// clip mask from the developed picture; the tile statistics behind the
// automatic contrast threshold; the blend mask and its blur; and the
// row sums the stats are made from. The constants are
// `greycard_core::develop::sharpen`'s. The clip mask, the dilation,
// the contrast, the sigmoid and the blur take the reference's
// operations in its order; `l_star` uses a power and a Newton step
// for the reference's `cbrt`, and `tile_index` and `row_sum` reduce
// a strided partial per thread and then a tree where the reference
// sums in sequence (see the module doc in sharpen.rs for what those
// last bits could move).

struct Params {
    // The picture's size.
    size: vec2<u32>,
    tile: u32,
    border: u32,
    // A batch of tiles: its padded tile side, tiles across and down,
    // and the first tile column and band it starts at.
    full: u32,
    cols: u32,
    rows: u32,
    c0: u32,
    b0: u32,
    iterations: u32,
    stop_early: u32,
    // The blur's kernel: taps each side of the center, and taps in all.
    half: u32,
    taps: u32,
    threshold: f32,
    clip_level: f32,
    // A grid of tiles for the threshold search: where it starts, its
    // step, its tile's side, and how many along each axis.
    origin: vec2<u32>,
    skip: u32,
    tsize: u32,
    count: vec2<u32>,
    offset: u32,
    pad1: u32,
    // The kernel's taps, up to sixteen: a deconvolution kernel whole,
    // or the blend blur's one side, the center first.
    kernel: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var rgb: texture_2d<f32>;
@group(0) @binding(2) var lum: texture_storage_2d<r32float, read_write>;
@group(0) @binding(3) var lstar: texture_storage_2d<r32float, read_write>;
@group(0) @binding(4) var blend: texture_storage_2d<r32float, read_write>;
@group(0) @binding(5) var tmp: texture_storage_2d<r32float, read_write>;
// Row sums, or the tile indices of a search.
@group(0) @binding(6) var<storage, read_write> partials: array<f32>;

const LUMA = vec3<f32>(0.2627, 0.6780, 0.0593);
// RawTherapee's tile limits, in L*.
const MIN_TILE_L: f32 = 2000.0 / 327.68;
const MAX_TILE_L: f32 = 20000.0 / 327.68;
const MIN_TILE_VARIANCE: f32 = 0.5 / 327.68;

fn tap(i: u32) -> f32 {
    return p.kernel[i / 4u][i % 4u];
}

fn cbrt(y: f32) -> f32 {
    // WGSL has no cube root; a power and one Newton step lands
    // within an ulp of one.
    var r = pow(y, 1.0 / 3.0);
    r = r - (r * r * r - y) / (3.0 * r * r);
    return r;
}

// CIE L* of a luminance relative to white at one.
fn l_star(y: f32) -> f32 {
    var f: f32;
    if y > 0.008856 {
        f = cbrt(max(y, 0.0));
    } else {
        f = 7.787 * y + 16.0 / 116.0;
    }
    return 116.0 * f - 16.0;
}

fn lstar_at(x: i32, y: i32) -> f32 {
    return textureLoad(lstar, vec2<i32>(x, y)).r;
}

// Local contrast as RawTherapee measures it, at a point at least two
// pixels inside the picture.
fn contrast(x: i32, y: i32) -> f32 {
    let dx1 = lstar_at(x + 1, y) - lstar_at(x - 1, y);
    let dy1 = lstar_at(x, y + 1) - lstar_at(x, y - 1);
    let dx2 = lstar_at(x + 2, y) - lstar_at(x - 2, y);
    let dy2 = lstar_at(x, y + 2) - lstar_at(x, y - 2);
    return sqrt(dx1 * dx1 + dy1 * dy1 + dx2 * dx2 + dy2 * dy2) * 0.0625;
}

// RawTherapee's sigmoid: half at the threshold, near one at twice it.
fn blend_factor(c: f32, threshold: f32) -> f32 {
    let x = -16.0 + (16.0 / threshold) * c;
    return 0.5 * (1.0 + x / sqrt(1.0 + x * x));
}

// Luminance and L* of every pixel, and the clip mask into `blend`.
@compute @workgroup_size(16, 16, 1)
fn prepare(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let px = textureLoad(rgb, at, 0).rgb;
    let y = LUMA.x * px.x + LUMA.y * px.y + LUMA.z * px.z;
    textureStore(lum, at, vec4<f32>(y, 0.0, 0.0, 0.0));
    textureStore(lstar, at, vec4<f32>(l_star(y), 0.0, 0.0, 0.0));
    var clip = 1.0;
    if px.x >= p.clip_level || px.y >= p.clip_level || px.z >= p.clip_level {
        clip = 0.0;
    }
    textureStore(blend, at, vec4<f32>(clip, 0.0, 0.0, 0.0));
}

var<workgroup> scratch: array<f32, 256>;

// The sum of one row of `blend`, a workgroup a row.
@compute @workgroup_size(256, 1, 1)
fn row_sum(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let y = i32(wid.x);
    var acc = 0.0;
    for (var x = lid.x; x < p.size.x; x += 256u) {
        acc += textureLoad(blend, vec2<i32>(i32(x), y)).r;
    }
    scratch[lid.x] = acc;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if lid.x < s {
            scratch[lid.x] += scratch[lid.x + s];
        }
        workgroupBarrier();
    }
    if lid.x == 0u {
        partials[p.offset + wid.x] = scratch[0];
    }
}

// The variance-over-mean of one tile of L*, a workgroup a tile, or
// infinity when the tile is too dark, too bright, suspiciously flat,
// or off the picture.
@compute @workgroup_size(256, 1, 1)
fn tile_index(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let x0 = p.origin.x + wid.x * p.skip;
    let y0 = p.origin.y + wid.y * p.skip;
    let size = p.tsize;
    let n = size * size;
    let inside = x0 + size <= p.size.x && y0 + size <= p.size.y;
    var acc = 0.0;
    if inside {
        for (var i = lid.x; i < n; i += 256u) {
            acc += lstar_at(i32(x0 + i % size), i32(y0 + i / size));
        }
    }
    scratch[lid.x] = acc;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if lid.x < s {
            scratch[lid.x] += scratch[lid.x + s];
        }
        workgroupBarrier();
    }
    let avg = scratch[0] / f32(n);
    workgroupBarrier();
    var var_acc = 0.0;
    if inside {
        for (var i = lid.x; i < n; i += 256u) {
            let d = lstar_at(i32(x0 + i % size), i32(y0 + i / size)) - avg;
            var_acc += d * d;
        }
    }
    scratch[lid.x] = var_acc;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if lid.x < s {
            scratch[lid.x] += scratch[lid.x + s];
        }
        workgroupBarrier();
    }
    if lid.x == 0u {
        var index = scratch[0] / (f32(n) * avg);
        if !inside || avg < MIN_TILE_L || avg > MAX_TILE_L || index < MIN_TILE_VARIANCE {
            index = bitcast<f32>(0x7f800000u);
        }
        partials[p.offset + wid.y * p.count.x + wid.x] = index;
    }
}

// The clip mask's zeros widened by two pixels each way: `blend` (the
// clip mask) into `tmp`.
@compute @workgroup_size(16, 16, 1)
fn dilate(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    var m = textureLoad(blend, at).r;
    if m != 0.0 {
        let x0 = max(at.x - 2, 0);
        let x1 = min(at.x + 2, i32(p.size.x) - 1);
        let y0 = max(at.y - 2, 0);
        let y1 = min(at.y + 2, i32(p.size.y) - 1);
        for (var yy = y0; yy <= y1; yy++) {
            for (var xx = x0; xx <= x1; xx++) {
                if textureLoad(blend, vec2<i32>(xx, yy)).r == 0.0 {
                    m = 0.0;
                }
            }
        }
    }
    textureStore(tmp, at, vec4<f32>(m, 0.0, 0.0, 0.0));
}

// The blend mask before its blur: the dilated clip mask in `tmp`
// times the sigmoid of the local contrast, into `blend`.
@compute @workgroup_size(16, 16, 1)
fn contrast_blend(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let xx = clamp(at.x, 2, i32(p.size.x) - 3);
    let yy = clamp(at.y, 2, i32(p.size.y) - 3);
    let b = textureLoad(tmp, at).r * blend_factor(contrast(xx, yy), p.threshold);
    textureStore(blend, at, vec4<f32>(b, 0.0, 0.0, 0.0));
}

// The blend blur along the rows, `blend` into `tmp`, reads clamped at
// the picture's edge; the kernel is one side, center first.
@compute @workgroup_size(16, 16, 1)
fn blur_rows(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let last = i32(p.size.x) - 1;
    var acc = tap(0u) * textureLoad(blend, at).r;
    for (var k = 1u; k <= p.half; k++) {
        let d = i32(k);
        let a = textureLoad(blend, vec2<i32>(clamp(at.x - d, 0, last), at.y)).r;
        let b = textureLoad(blend, vec2<i32>(clamp(at.x + d, 0, last), at.y)).r;
        acc += tap(k) * (a + b);
    }
    textureStore(tmp, at, vec4<f32>(acc, 0.0, 0.0, 0.0));
}

// The blend blur down the columns, `tmp` into `blend`.
@compute @workgroup_size(16, 16, 1)
fn blur_cols(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let last = i32(p.size.y) - 1;
    var acc = tap(0u) * textureLoad(tmp, at).r;
    for (var k = 1u; k <= p.half; k++) {
        let d = i32(k);
        let a = textureLoad(tmp, vec2<i32>(at.x, clamp(at.y - d, 0, last))).r;
        let b = textureLoad(tmp, vec2<i32>(at.x, clamp(at.y + d, 0, last))).r;
        acc += tap(k) * (a + b);
    }
    textureStore(blend, at, vec4<f32>(acc, 0.0, 0.0, 0.0));
}

// `tmp` becomes the sharpened luminance, starting as the luminance:
// what a block the sharpen never commits keeps.
@compute @workgroup_size(16, 16, 1)
fn init_sharpened(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    textureStore(tmp, at, textureLoad(lum, at));
}
