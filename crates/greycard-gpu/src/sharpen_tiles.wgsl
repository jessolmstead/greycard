// The capture sharpening's Richardson–Lucy iterations, in the same
// tiles the CPU reference works in: each tile with its border of
// context lives in an atlas, padded and clamped as the reference pads
// and clamps it, and every 32-pixel block stops when the reference's
// would. A batch of tiles is worked at once; the picture's tiles are
// taken in as many batches as the atlas holds.

struct Params {
    size: vec2<u32>,
    tile: u32,
    border: u32,
    full: u32,
    cols: u32,
    rows: u32,
    c0: u32,
    b0: u32,
    iterations: u32,
    stop_early: u32,
    half: u32,
    taps: u32,
    threshold: f32,
    clip_level: f32,
    origin: vec2<u32>,
    skip: u32,
    tsize: u32,
    count: vec2<u32>,
    offset: u32,
    pad1: u32,
    kernel: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var lum: texture_2d<f32>;
@group(0) @binding(2) var blend: texture_2d<f32>;
@group(0) @binding(3) var sharpened: texture_storage_2d<r32float, read_write>;
@group(0) @binding(4) var estimate: texture_storage_2d<r32float, read_write>;
@group(0) @binding(5) var ratio: texture_storage_2d<r32float, read_write>;
// Per block of the batch: whether its iterations are over.
@group(0) @binding(6) var<storage, read_write> settled: array<u32>;
// Per tile of the batch: how many blocks are still going.
@group(0) @binding(7) var<storage, read_write> left: array<atomic<u32>>;

// The widest kernel reaches six each side; a workgroup's 16x16 patch
// of a tile is blurred from a region that much wider all round.
const REACH: u32 = 6u;
const REGION: u32 = 28u;
const BLOCK: u32 = 32u;
// A block whose blend never reaches this is left as it is.
const SETTLED_BLEND: f32 = 0.01;

var<workgroup> region: array<f32, 784>;
var<workgroup> rowblur: array<f32, 448>;
var<workgroup> scratch: array<f32, 256>;
var<workgroup> wg_flag: u32;

fn tap(i: u32) -> f32 {
    return p.kernel[i / 4u][i % 4u];
}

// A tile of the batch: its origin on the picture and in the atlas.
struct Tile {
    x0: u32,
    y0: u32,
    ax: u32,
    ay: u32,
    cols_here: u32,
    rows_here: u32,
}

fn tile_of(t: u32) -> Tile {
    let tc = t % p.cols;
    let tr = t / p.cols;
    var tile: Tile;
    tile.x0 = (p.c0 + tc) * p.tile;
    tile.y0 = (p.b0 + tr) * p.tile;
    tile.ax = tc * p.full;
    tile.ay = tr * p.full;
    tile.cols_here = min(p.tile, p.size.x - tile.x0);
    tile.rows_here = min(p.tile, p.size.y - tile.y0);
    return tile;
}

// The picture's luminance under a padded tile's pixel, the read
// clamped at the picture's edge as the reference fills its tile.
fn original(tile: Tile, tx: u32, ty: u32) -> f32 {
    let sx = clamp(i32(tile.x0 + tx) - i32(p.border), 0, i32(p.size.x) - 1);
    let sy = clamp(i32(tile.y0 + ty) - i32(p.border), 0, i32(p.size.y) - 1);
    return textureLoad(lum, vec2<i32>(sx, sy), 0).r;
}

fn blocks_per_side() -> u32 {
    return p.tile / BLOCK;
}

// Whether this tile still has blocks going, the same answer for the
// whole workgroup so the barriers after it are in uniform control.
fn tile_active(t: u32, li: u32) -> bool {
    if li == 0u {
        wg_flag = atomicLoad(&left[t]);
    }
    return workgroupUniformLoad(&wg_flag) != 0u;
}

// The separable blur of `estimate` (which 0) or `ratio` (which 1) at
// this thread's pixel of the tile: the rows first, then the columns,
// reads clamped at the padded tile's edge, the sums in the reference's
// order. Every thread of the workgroup takes part, whether or not its
// pixel is on the tile.
fn blurred(which: u32, tile: Tile, gx0: u32, gy0: u32, lid: vec2<u32>) -> f32 {
    let li = lid.y * 16u + lid.x;
    let last = i32(p.full) - 1;
    for (var i = li; i < REGION * REGION; i += 256u) {
        let r = i / REGION;
        let c = i % REGION;
        let sx = clamp(i32(gx0) - i32(REACH) + i32(c), 0, last);
        let sy = clamp(i32(gy0) - i32(REACH) + i32(r), 0, last);
        let at = vec2<i32>(i32(tile.ax) + sx, i32(tile.ay) + sy);
        var v: f32;
        if which == 0u {
            v = textureLoad(estimate, at).r;
        } else {
            v = textureLoad(ratio, at).r;
        }
        region[i] = v;
    }
    workgroupBarrier();
    for (var i = li; i < REGION * 16u; i += 256u) {
        let r = i / 16u;
        let c = i % 16u;
        var acc = 0.0;
        for (var k = 0u; k < p.taps; k++) {
            acc += tap(k) * region[r * REGION + c + REACH + k - p.half];
        }
        rowblur[i] = acc;
    }
    workgroupBarrier();
    var acc = 0.0;
    for (var k = 0u; k < p.taps; k++) {
        acc += tap(k) * rowblur[(lid.y + REACH + k - p.half) * 16u + lid.x];
    }
    return acc;
}

// Whether each block of the batch has anything to do, and how many
// blocks each tile has going; a workgroup a block.
@compute @workgroup_size(32, 8, 1)
fn setup_blocks(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let t = wid.z;
    let tile = tile_of(t);
    let bps = blocks_per_side();
    let b = t * bps * bps + wid.y * bps + wid.x;
    let li = lid.y * 32u + lid.x;
    if wid.x * BLOCK >= tile.cols_here || wid.y * BLOCK >= tile.rows_here {
        // No such block on this tile.
        if li == 0u {
            settled[b] = 1u;
        }
        return;
    }
    var most = 0.0;
    let tx = wid.x * BLOCK + lid.x;
    for (var r = 0u; r < 4u; r++) {
        let ty = wid.y * BLOCK + lid.y + 8u * r;
        if tx < tile.cols_here && ty < tile.rows_here {
            let v = textureLoad(blend, vec2<i32>(i32(tile.x0 + tx), i32(tile.y0 + ty)), 0).r;
            most = max(most, v);
        }
    }
    scratch[li] = most;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if li < s {
            scratch[li] = max(scratch[li], scratch[li + s]);
        }
        workgroupBarrier();
    }
    if li == 0u {
        if scratch[0] < SETTLED_BLEND {
            settled[b] = 1u;
        } else {
            settled[b] = 0u;
            atomicAdd(&left[t], 1u);
        }
    }
}

// The estimate starts as the luminance under the padded tile.
@compute @workgroup_size(16, 16, 1)
fn fill_estimate(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
    let tile = tile_of(wid.z);
    if gid.x >= p.full || gid.y >= p.full {
        return;
    }
    let v = original(tile, gid.x, gid.y);
    textureStore(estimate, vec2<i32>(i32(tile.ax + gid.x), i32(tile.ay + gid.y)), vec4<f32>(v, 0.0, 0.0, 0.0));
}

// ratio = original / blur(estimate).
@compute @workgroup_size(16, 16, 1)
fn blur_ratio(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let t = wid.z;
    let li = lid.y * 16u + lid.x;
    if !tile_active(t, li) {
        return;
    }
    let tile = tile_of(t);
    let gx0 = wid.x * 16u;
    let gy0 = wid.y * 16u;
    let bl = blurred(0u, tile, gx0, gy0, lid.xy);
    let tx = gx0 + lid.x;
    let ty = gy0 + lid.y;
    if tx < p.full && ty < p.full {
        let r = original(tile, tx, ty) / max(bl, 1.0e-6);
        textureStore(ratio, vec2<i32>(i32(tile.ax + tx), i32(tile.ay + ty)), vec4<f32>(r, 0.0, 0.0, 0.0));
    }
}

// estimate *= blur(ratio).
@compute @workgroup_size(16, 16, 1)
fn blur_multiply(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let t = wid.z;
    let li = lid.y * 16u + lid.x;
    if !tile_active(t, li) {
        return;
    }
    let tile = tile_of(t);
    let gx0 = wid.x * 16u;
    let gy0 = wid.y * 16u;
    let bl = blurred(1u, tile, gx0, gy0, lid.xy);
    let tx = gx0 + lid.x;
    let ty = gy0 + lid.y;
    if tx < p.full && ty < p.full {
        let at = vec2<i32>(i32(tile.ax + tx), i32(tile.ay + ty));
        let e = textureLoad(estimate, at).r * bl;
        textureStore(estimate, at, vec4<f32>(e, 0.0, 0.0, 0.0));
    }
}

// A block's estimate blended into the sharpened luminance by the mask.
fn commit(tile: Tile, tx: u32, ty: u32) {
    let at = vec2<i32>(i32(tile.x0 + tx), i32(tile.y0 + ty));
    let l = textureLoad(lum, at, 0).r;
    let b = textureLoad(blend, at, 0).r;
    let e = textureLoad(estimate, vec2<i32>(i32(tile.ax + tx + p.border), i32(tile.ay + ty + p.border))).r;
    textureStore(sharpened, at, vec4<f32>(l + b * (e - l), 0.0, 0.0, 0.0));
}

// After an iteration: a block whose estimate has dropped under half its
// blended start anywhere stops here, its estimate committed as it is;
// a workgroup a block.
@compute @workgroup_size(32, 8, 1)
fn check(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let t = wid.z;
    let bps = blocks_per_side();
    let b = t * bps * bps + wid.y * bps + wid.x;
    let li = lid.y * 32u + lid.x;
    if li == 0u {
        wg_flag = settled[b];
    }
    if workgroupUniformLoad(&wg_flag) != 0u {
        return;
    }
    let tile = tile_of(t);
    let tx = wid.x * BLOCK + lid.x;
    var worst = bitcast<f32>(0x7f800000u);
    for (var r = 0u; r < 4u; r++) {
        let ty = wid.y * BLOCK + lid.y + 8u * r;
        if tx < tile.cols_here && ty < tile.rows_here {
            let at = vec2<i32>(i32(tile.x0 + tx), i32(tile.y0 + ty));
            let e = textureLoad(estimate, vec2<i32>(i32(tile.ax + tx + p.border), i32(tile.ay + ty + p.border))).r;
            let f = textureLoad(lum, at, 0).r * textureLoad(blend, at, 0).r * 0.5;
            worst = min(worst, e - f);
        }
    }
    scratch[li] = worst;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if li < s {
            scratch[li] = min(scratch[li], scratch[li + s]);
        }
        workgroupBarrier();
    }
    if li == 0u {
        wg_flag = select(0u, 1u, scratch[0] < 0.0);
    }
    if workgroupUniformLoad(&wg_flag) == 0u {
        return;
    }
    for (var r = 0u; r < 4u; r++) {
        let ty = wid.y * BLOCK + lid.y + 8u * r;
        if tx < tile.cols_here && ty < tile.rows_here {
            commit(tile, tx, ty);
        }
    }
    if li == 0u {
        settled[b] = 1u;
        atomicSub(&left[t], 1u);
    }
}

// After the last iteration: every block still going is committed.
@compute @workgroup_size(16, 16, 1)
fn commit_rest(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
    let t = wid.z;
    let tile = tile_of(t);
    if gid.x >= tile.cols_here || gid.y >= tile.rows_here {
        return;
    }
    let bps = blocks_per_side();
    let b = t * bps * bps + (gid.y / BLOCK) * bps + gid.x / BLOCK;
    if settled[b] != 0u {
        return;
    }
    commit(tile, gid.x, gid.y);
}
