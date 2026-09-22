// The capture sharpening's last step: every channel scaled by the
// sharpened luminance over the old, the blend mask in the alpha, into
// the viewport's half-float texture or into full floats for a read
// back.

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
@group(0) @binding(1) var rgb: texture_2d<f32>;
@group(0) @binding(2) var lum: texture_2d<f32>;
@group(0) @binding(3) var sharpened: texture_2d<f32>;
@group(0) @binding(4) var blend: texture_2d<f32>;
@group(0) @binding(5) var out_half: texture_storage_2d<rgba16float, write>;
@group(0) @binding(6) var out_full: texture_storage_2d<rgba32float, write>;
@group(0) @binding(7) var out_mask: texture_storage_2d<r32float, write>;

fn sharpened_pixel(at: vec2<i32>) -> vec4<f32> {
    let px = textureLoad(rgb, at, 0).rgb;
    let old = textureLoad(lum, at, 0).r;
    let now = textureLoad(sharpened, at, 0).r;
    let factor = now / max(old, 1.0e-5);
    let mask = textureLoad(blend, at, 0).r;
    return vec4<f32>(px * factor, mask);
}

@compute @workgroup_size(16, 16, 1)
fn apply_half(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    textureStore(out_half, at, sharpened_pixel(at));
}

@compute @workgroup_size(16, 16, 1)
fn apply_full(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.size.x || gid.y >= p.size.y {
        return;
    }
    let at = vec2<i32>(gid.xy);
    let v = sharpened_pixel(at);
    textureStore(out_full, at, vec4<f32>(v.rgb, 1.0));
    textureStore(out_mask, at, vec4<f32>(v.a, 0.0, 0.0, 0.0));
}
