// The dehaze's two halves that scale with the picture, on the GPU:
// the block sums the fit's reduced copy is made from, and the apply
// that puts the fitted model back on every pixel. The fit itself, the
// airlight's quantiles and the guided filter on the reduced grid, is
// the reference's own on the CPU, from the sums read back; the model
// it makes comes back here as two planes on the grid and the
// bilinear taps of every column and row, the reference's to the bit.
//
// The block sums are added as the reference adds them, from zero,
// along each row of a block and the rows in order, one invocation a
// block, so that on the same pixels they are the reference's to the
// bit: an add is correctly rounded on every Vulkan device, and the
// order is the only thing left to differ. The division by the count
// is the reference's, on the CPU, since a division here is not
// correctly rounded.

struct Params {
    // The picture's size, and the reduced grid's.
    size: vec2<u32>,
    grid: vec2<u32>,
    // The reduction's factor on each edge.
    factor: u32,
    // The amount's share of the dark channel, and the floor of the
    // transmission when haze is taken out.
    strength: f32,
    min_transmission: f32,
    pad: u32,
    // The airlight, working-space RGB, in xyz.
    airlight: vec4<f32>,
    // The luminance weights in xyz.
    luma: vec4<f32>,
}

// A full-size coordinate's two reduced indices and the weight of the
// second, as the reference's `taps` gives them.
struct Tap {
    i0: u32,
    i1: u32,
    t: f32,
    pad: u32,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var pic: texture_2d<f32>;
// The block sums, interleaved RGB on the grid.
@group(0) @binding(2) var<storage, read_write> sums: array<f32>;
// The model: the averaged slope and intercept on the grid, and the
// taps of the picture's columns and rows.
@group(0) @binding(3) var<storage, read> slope: array<f32>;
@group(0) @binding(4) var<storage, read> intercept: array<f32>;
@group(0) @binding(5) var<storage, read> columns: array<Tap>;
@group(0) @binding(6) var<storage, read> rows: array<Tap>;
// Each workgroup's sum and least of the transmissions it applied.
@group(0) @binding(7) var<storage, read_write> partial: array<vec2<f32>>;
// The apply's outputs: the viewport's half floats, or full floats.
@group(0) @binding(8) var out_half: texture_storage_2d<rgba16float, write>;
@group(0) @binding(9) var out_full: texture_storage_2d<rgba32float, write>;

// ---- The block sums: one invocation a block of the grid.

@compute @workgroup_size(16, 16, 1)
fn block_sums(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.grid.x || gid.y >= p.grid.y {
        return;
    }
    let x0 = gid.x * p.factor;
    let y0 = gid.y * p.factor;
    let x1 = min(x0 + p.factor, p.size.x);
    let y1 = min(y0 + p.factor, p.size.y);
    var s = vec3<f32>(0.0);
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            s = s + textureLoad(pic, vec2<u32>(x, y), 0).rgb;
        }
    }
    let i = (gid.y * p.grid.x + gid.x) * 3u;
    sums[i] = s.x;
    sums[i + 1u] = s.y;
    sums[i + 2u] = s.z;
}

// ---- The apply: the reference's `Model::transmission` and the
// recovery, a pixel an invocation.

// The model's slope and intercept at a pixel's taps, in x and y: each
// along the row on the two rows, then down between them, in the
// reference's order.
fn line_at(tx: Tap, ty: Tap) -> vec2<f32> {
    let w = p.grid.x;
    let i00 = ty.i0 * w + tx.i0;
    let i01 = ty.i0 * w + tx.i1;
    let i10 = ty.i1 * w + tx.i0;
    let i11 = ty.i1 * w + tx.i1;
    let a = vec2<f32>(slope[i00], intercept[i00]);
    let b = vec2<f32>(slope[i01], intercept[i01]);
    let c = vec2<f32>(slope[i10], intercept[i10]);
    let d = vec2<f32>(slope[i11], intercept[i11]);
    let top = a + (b - a) * tx.t;
    let bottom = c + (d - c) * tx.t;
    return top + (bottom - top) * ty.t;
}

// The dark channel of one pixel over the airlight, held to 0..1.
fn dark_ratio(px: vec3<f32>) -> f32 {
    let r = px / p.airlight.xyz;
    return clamp(min(r.x, min(r.y, r.z)), 0.0, 1.0);
}

// The transmission at a pixel: the model's line in its luminance, no
// less than its own dark channel reads, floored, and held under one
// when haze is taken out; over one, to one less the strength, when
// it is put in.
fn transmission(px: vec3<f32>, tx: Tap, ty: Tap) -> f32 {
    let lum = p.luma.x * px.x + p.luma.y * px.y + p.luma.z * px.z;
    let line = line_at(tx, ty);
    let t = line.x * lum + line.y;
    if p.strength > 0.0 {
        let own = 1.0 - p.strength * dark_ratio(px);
        return clamp(max(t, own), p.min_transmission, 1.0);
    }
    return clamp(t, 1.0, 1.0 - p.strength);
}

var<workgroup> wg_sum: array<f32, 256>;
var<workgroup> wg_min: array<f32, 256>;

// Larger than any transmission: a pixel outside the picture's.
const NONE: f32 = 1e30;

// The recovered pixel at `at`, and this workgroup's sum and least of
// the transmissions into `partial`. Every invocation reaches the
// barriers; one outside the picture adds nothing.
fn cleared(at: vec2<u32>, lid: u32, wid: vec2<u32>, groups: u32) -> vec3<f32> {
    let inside = at.x < p.size.x && at.y < p.size.y;
    var out = vec3<f32>(0.0);
    var t = 0.0;
    var least = NONE;
    if inside {
        let px = textureLoad(pic, at, 0).rgb;
        t = transmission(px, columns[at.x], rows[at.y]);
        least = t;
        let a = p.airlight.xyz;
        out = (px - a) / t + a;
    }
    wg_sum[lid] = t;
    wg_min[lid] = least;
    workgroupBarrier();
    for (var step = 128u; step > 0u; step = step / 2u) {
        if lid < step {
            wg_sum[lid] = wg_sum[lid] + wg_sum[lid + step];
            wg_min[lid] = min(wg_min[lid], wg_min[lid + step]);
        }
        workgroupBarrier();
    }
    if lid == 0u {
        partial[wid.y * groups + wid.x] = vec2<f32>(wg_sum[0], wg_min[0]);
    }
    return out;
}

// Into the viewport's texture, the alpha zero: no mask to paint.
@compute @workgroup_size(16, 16, 1)
fn apply_half(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(num_workgroups) n: vec3<u32>,
) {
    let px = cleared(gid.xy, lid, wid.xy, n.x);
    if gid.x < p.size.x && gid.y < p.size.y {
        textureStore(out_half, gid.xy, vec4<f32>(px, 0.0));
    }
}

// Into full floats for the sharpen or a read back, the alpha one as
// the upload has it.
@compute @workgroup_size(16, 16, 1)
fn apply_full(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(num_workgroups) n: vec3<u32>,
) {
    let px = cleared(gid.xy, lid, wid.xy, n.x);
    if gid.x < p.size.x && gid.y < p.size.y {
        textureStore(out_full, gid.xy, vec4<f32>(px, 1.0));
    }
}
