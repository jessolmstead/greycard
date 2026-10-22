// The lens correction on the GPU: `greycard_core::develop::lens`'s
// `resample_row` and `vignette_row`, a thread a pixel. Each output
// pixel reads each channel at the source position the correction's
// models give (`source_of`), Catmull-Rom over four by four texels with
// the edges clamped, nothing where the position is off the picture,
// and the vignetting's gain taken where it read. A correction that
// moves nothing multiplies the gain in at the pixel itself.
//
// Every expression is the reference's, in the reference's order, so
// the two differ only by the device's freedom to fuse a multiply and
// an add and by its division and square root, which Vulkan holds to a
// few units in the last place rather than correctly rounded.

struct Params {
    // The picture's size, the first row of this band, and the
    // distortion model: 0 none, 1 poly3, 2 poly5, 3 ptlens, 4 acm.
    size: vec2<u32>,
    y0: u32,
    distortion: u32,
    // The size again in floats, as the reference converts it.
    dims: vec2<f32>,
    scale: f32,
    radius_scale: f32,
    vignetting_scale: f32,
    // Whether there is a CA model, a vignetting model, and whether
    // anything moves.
    ca: u32,
    vignetting: u32,
    moves: u32,
    // The distortion's coefficients: k1, k2 for poly5; a, b, c for
    // ptlens; acm's five k then its focal scale.
    dist: vec4<f32>,
    dist2: vec4<f32>,
    // CA's [v, c, b] for red and blue.
    red: vec4<f32>,
    blue: vec4<f32>,
    // The vignetting's k1, k2, k3.
    vig: vec4<f32>,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
// One band of rows of the output, from `p.y0`.
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

// A quotient as the CPU rounds it. Vulkan holds `/` to 2.5 units in
// the last place, and NVIDIA's is that loose: a position's few units
// at 8000 pixels are a thousandth of a pixel, a visible share of a
// step between neighbors. One correction from the residual brings it
// to the correctly rounded quotient in all but rare ties, where `fma`
// is fused, as it is on the devices we have run; where a backend
// evaluates it unfused, the correction is smaller but never a step
// away from the plain quotient.
fn div(a: f32, b: f32) -> f32 {
    let q = a / b;
    return q + fma(-q, b, a) / b;
}

// A square root as the CPU rounds it, by the same correction.
fn root(x: f32) -> f32 {
    let s = sqrt(x);
    if s == 0.0 {
        return 0.0;
    }
    return s + fma(-s, s, x) / (2.0 * s);
}

fn distorted(x: f32, y: f32) -> vec2<f32> {
    let r2 = x * x + y * y;
    switch p.distortion {
        case 1u: {
            let k1 = p.dist.x;
            let f = 1.0 - k1 + k1 * r2;
            return vec2(x * f, y * f);
        }
        case 2u: {
            let f = 1.0 + p.dist.x * r2 + p.dist.y * r2 * r2;
            return vec2(x * f, y * f);
        }
        case 3u: {
            let a = p.dist.x;
            let b = p.dist.y;
            let c = p.dist.z;
            let r = root(r2);
            let f = a * r2 * r + b * r2 + c * r + 1.0 - a - b - c;
            return vec2(x * f, y * f);
        }
        case 4u: {
            let k = array(p.dist.x, p.dist.y, p.dist.z, p.dist.w, p.dist2.x);
            let focal = p.dist2.y;
            let xf = x * focal;
            let yf = y * focal;
            let s2 = xf * xf + yf * yf;
            let radial = 1.0 + k[0] * s2 + k[1] * s2 * s2 + k[2] * s2 * s2 * s2;
            let tangent = 2.0 * (k[3] * yf + k[4] * xf);
            let xd = xf * radial + xf * tangent + k[4] * s2;
            let yd = yf * radial + yf * tangent + k[3] * s2;
            return vec2(div(xd, focal), div(yd, focal));
        }
        default: {
            return vec2(x, y);
        }
    }
}

fn shifted(channel: u32, x: f32, y: f32) -> vec2<f32> {
    var m: vec4<f32>;
    if channel == 0u {
        m = p.red;
    } else if channel == 2u {
        m = p.blue;
    } else {
        return vec2(x, y);
    }
    let r = root(x * x + y * y);
    let f = m.x + m.y * r + m.z * r * r;
    return vec2(x * f, y * f);
}

// `LensCorrection::source_of`.
fn source_of(channel: u32, x: f32, y: f32) -> vec2<f32> {
    let w = p.dims.x;
    let h = p.dims.y;
    let half = min(w, h) / 2.0;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let k = p.radius_scale;
    var uv = vec2(div(div(x - cx, half), p.scale) * k, div(div(y - cy, half), p.scale) * k);
    if p.distortion != 0u {
        uv = distorted(uv.x, uv.y);
    }
    if p.ca != 0u {
        uv = shifted(channel, uv.x, uv.y);
    }
    return vec2(cx + div(uv.x, k) * half, cy + div(uv.y, k) * half);
}

// `LensCorrection::gain_at`, with `Vignetting::falloff` and the floor.
fn gain_at(x: f32, y: f32) -> f32 {
    let w = p.dims.x;
    let h = p.dims.y;
    let dx = x - w / 2.0;
    let dy = y - h / 2.0;
    let r = div(root(dx * dx + dy * dy), root(w * w + h * h)) * 2.0 * p.vignetting_scale;
    let rc = min(r, 1.0);
    let r2 = rc * rc;
    let falloff = 1.0 + p.vig.x * r2 + p.vig.y * r2 * r2 + p.vig.z * r2 * r2 * r2;
    return div(1.0, max(falloff, 0.01));
}

// Catmull-Rom's weights, as the reference's `weights`.
fn weights(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4(
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    );
}

// `sample_cubic` for all three channels at one position: pixel
// centers at halves, the taps clamped to the picture, the rows'
// sums in the reference's order.
fn sample_cubic(x: f32, y: f32) -> vec3<f32> {
    let w = i32(p.size.x);
    let h = i32(p.size.y);
    let fx = floor(x - 0.5);
    let fy = floor(y - 0.5);
    let wx = weights(x - 0.5 - fx);
    let wy = weights(y - 0.5 - fy);
    let ix = i32(fx);
    let iy = i32(fy);
    var out = vec3(0.0);
    for (var j = 0; j < 4; j++) {
        let sy = clamp(iy + j - 1, 0, h - 1);
        for (var i = 0; i < 4; i++) {
            let sx = clamp(ix + i - 1, 0, w - 1);
            out += wx[i] * wy[j] * textureLoad(src, vec2(sx, sy), 0).rgb;
        }
    }
    return out;
}

fn inside(s: vec2<f32>) -> bool {
    return !(s.x < 0.0 || s.y < 0.0 || s.x >= p.dims.x || s.y >= p.dims.y);
}

@compute @workgroup_size(16, 16)
fn correct(@builtin(global_invocation_id) id: vec3<u32>) {
    let rows = textureDimensions(dst).y;
    let y = id.y + p.y0;
    if id.x >= p.size.x || id.y >= rows || y >= p.size.y {
        return;
    }
    let ox = f32(id.x) + 0.5;
    let oy = f32(y) + 0.5;
    var px: vec3<f32>;
    if p.moves == 0u {
        px = textureLoad(src, vec2(id.x, y), 0).rgb;
        if p.vignetting != 0u {
            px *= gain_at(ox, oy);
        }
    } else if p.ca == 0u {
        // Every channel reads at the green's place.
        let s = source_of(1u, ox, oy);
        if inside(s) {
            px = sample_cubic(s.x, s.y);
            if p.vignetting != 0u {
                px *= gain_at(s.x, s.y);
            }
        }
    } else {
        for (var c = 0u; c < 3u; c++) {
            let s = source_of(c, ox, oy);
            if inside(s) {
                var v = sample_cubic(s.x, s.y)[c];
                if p.vignetting != 0u {
                    v *= gain_at(s.x, s.y);
                }
                px[c] = v;
            }
        }
    }
    textureStore(dst, vec2(id.x, id.y), vec4(px, 1.0));
}
