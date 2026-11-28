// The capture sharpening's last step: every channel scaled about the
// pedestal's negative by the sharpened luminance over the old, both on
// the pedestal, the blend mask in the alpha, into the viewport's
// half-float texture or into full floats for a read back; or the
// picture as it was, for a develop that makes the sharpen's delta and
// leaves the picture unsharpened. And the delta itself, in stops, as
// `sharpen::delta` makes it, for a mask's Sharpen.

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
    pedestal: f32,
    origin: vec2<u32>,
    skip: u32,
    tsize: u32,
    count: vec2<u32>,
    offset: u32,
    floor_pedestal: f32,
    // What the levels are read at, `SharpenOptions::level_scale`.
    level_scale: f32,
    // The first row a whole-picture pass works: a band's, or zero.
    row0: u32,
    kernel: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var rgb: texture_2d_array<f32>;
@group(0) @binding(2) var lum: texture_2d<f32>;
@group(0) @binding(3) var sharpened: texture_2d<f32>;
@group(0) @binding(4) var blend: texture_2d<f32>;
@group(0) @binding(5) var out_half: texture_storage_2d<rgba16float, write>;
@group(0) @binding(6) var out_full: texture_storage_2d_array<r32float, write>;
@group(0) @binding(7) var out_mask: texture_storage_2d<r32float, write>;
@group(0) @binding(8) var out_delta: texture_storage_2d<r32float, write>;

// The picture's three planes, a layer each (`crate::Image`).
fn pixel(at: vec2<i32>) -> vec3<f32> {
    return vec3<f32>(
        textureLoad(rgb, at, 0, 0).x,
        textureLoad(rgb, at, 1, 0).x,
        textureLoad(rgb, at, 2, 0).x,
    );
}

// A picture into its three planes.
fn store_full(at: vec2<i32>, px: vec3<f32>) {
    textureStore(out_full, at, 0, vec4<f32>(px.x, 0.0, 0.0, 0.0));
    textureStore(out_full, at, 1, vec4<f32>(px.y, 0.0, 0.0, 0.0));
    textureStore(out_full, at, 2, vec4<f32>(px.z, 0.0, 0.0, 0.0));
}

fn sharpened_pixel(at: vec2<i32>) -> vec4<f32> {
    let px = pixel(at);
    let old = textureLoad(lum, at, 0).r;
    let now = textureLoad(sharpened, at, 0).r;
    let factor = now / old;
    let mask = textureLoad(blend, at, 0).r;
    return vec4<f32>((px + p.pedestal) * factor - p.pedestal, mask);
}

@compute @workgroup_size(16, 16, 1)
fn apply_half(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.y + p.row0;
    if gid.x >= p.size.x || row >= p.size.y {
        return;
    }
    let at = vec2<i32>(i32(gid.x), i32(row));
    textureStore(out_half, at, halves_nearest(sharpened_pixel(at)));
}

@compute @workgroup_size(16, 16, 1)
fn apply_full(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.y + p.row0;
    if gid.x >= p.size.x || row >= p.size.y {
        return;
    }
    let at = vec2<i32>(i32(gid.x), i32(row));
    let v = sharpened_pixel(at);
    store_full(at, v.rgb);
    textureStore(out_mask, at, vec4<f32>(v.a, 0.0, 0.0, 0.0));
}

// The picture as it was, the alpha zero as the Detail section's leaves
// it: no sharpen to paint.
@compute @workgroup_size(16, 16, 1)
fn keep_half(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.y + p.row0;
    if gid.x >= p.size.x || row >= p.size.y {
        return;
    }
    let at = vec2<i32>(i32(gid.x), i32(row));
    textureStore(out_half, at, halves_nearest(vec4<f32>(pixel(at), 0.0)));
}

@compute @workgroup_size(16, 16, 1)
fn keep_full(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.y + p.row0;
    if gid.x >= p.size.x || row >= p.size.y {
        return;
    }
    let at = vec2<i32>(i32(gid.x), i32(row));
    store_full(at, pixel(at));
    textureStore(out_mask, at, vec4<f32>(textureLoad(blend, at, 0).r, 0.0, 0.0, 0.0));
}

// The sharpen's change to the pixel in stops: the sharpened luminance
// over the old, both on the pedestal.
@compute @workgroup_size(16, 16, 1)
fn delta(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.y + p.row0;
    if gid.x >= p.size.x || row >= p.size.y {
        return;
    }
    let at = vec2<i32>(i32(gid.x), i32(row));
    let old = textureLoad(lum, at, 0).r;
    let now = textureLoad(sharpened, at, 0).r;
    textureStore(out_delta, at, vec4<f32>(log2(now / old), 0.0, 0.0, 0.0));
}
