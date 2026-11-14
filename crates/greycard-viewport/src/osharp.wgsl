// The output sharpen's blur across, between the viewport's two passes
// (`viewport.wgsl`'s `osharp`): the first pass's encoded luminance,
// drawn with a margin past the target, blurred along each row by the
// Gaussian `output_sharpen::blur` takes, its taps held to the picture's
// own edges. The last pass blurs it down.
//
// The first pass writes `OSHARP_PAST` wherever it draws canvas, so the
// picture's edge is where the shader itself put it: a tap is held at
// the last texel of picture toward it, as the CPU clamps to its edge
// pixel, and no second reckoning of the edge can disagree with the
// shader's at a tie. `viewport.wgsl`'s `osharp_down` is this, down.

struct P {
    // The blur's standard deviation in target pixels, the taps'
    // spacing, taps a side, and their reach in whole texels.
    k: vec4<f32>,
}

@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var lum: texture_2d<f32>;

// What the first pass writes past the picture's edge: the luminance is
// never below zero, and a texel that is not a number is not this.
const OSHARP_PAST: f32 = -1.0;

fn past(v: f32) -> bool {
    return v < -0.5;
}

fn load(i: vec2<i32>) -> f32 {
    let limit = vec2<i32>(textureDimensions(lum)) - vec2<i32>(1, 1);
    return textureLoad(lum, clamp(i, vec2<i32>(0), limit), 0).r;
}

// The texture at a position in pixels, bilinear between the texel
// centers about it, its edges clamped: full floats are read by load,
// with no filtering sampler. `viewport.wgsl`'s `osharp_tap` is this.
fn tap(at: vec2<f32>) -> f32 {
    let u = at - vec2<f32>(0.5);
    let base = floor(u);
    let f = u - base;
    let i = vec2<i32>(base);
    // A weight of nothing on a texel is not read: a texel that is not
    // a number, or past the edge, beside a tap on a texel center stays
    // out of it.
    var acc = 0.0;
    if (f.x < 1.0 && f.y < 1.0) { acc = acc + (1.0 - f.x) * (1.0 - f.y) * load(i); }
    if (f.x > 0.0 && f.y < 1.0) { acc = acc + f.x * (1.0 - f.y) * load(i + vec2<i32>(1, 0)); }
    if (f.x < 1.0 && f.y > 0.0) { acc = acc + (1.0 - f.x) * f.y * load(i + vec2<i32>(0, 1)); }
    if (f.x > 0.0 && f.y > 0.0) { acc = acc + f.x * f.y * load(i + vec2<i32>(1, 1)); }
    return acc;
}

// Texels of picture from `c` toward `dir` before the first past the
// edge, `reach` at most. The picture's run along a row is one piece,
// so a texel of picture at the reach is picture all the way.
fn edge(c: vec2<i32>, dir: vec2<i32>, reach: i32) -> f32 {
    if (!past(load(c + dir * reach))) {
        return f32(reach);
    }
    var k = 0;
    while (k < reach && !past(load(c + dir * (k + 1)))) {
        k = k + 1;
    }
    return f32(k);
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = vec2<i32>(floor(pos.xy));
    // Canvas stays canvas: the last pass reads no column of it.
    if (past(load(c))) {
        return vec4<f32>(OSHARP_PAST, OSHARP_PAST, OSHARP_PAST, 1.0);
    }
    let s = max(p.k.x, 1e-3);
    let step = p.k.y;
    let n = i32(p.k.z + 0.5);
    let reach = i32(p.k.w + 0.5);
    let lo = pos.x - edge(c, vec2<i32>(-1, 0), reach);
    let hi = pos.x + edge(c, vec2<i32>(1, 0), reach);
    var acc = 0.0;
    var sum = 0.0;
    for (var i = -n; i <= n; i = i + 1) {
        let d = f32(i) * step;
        let v = tap(vec2<f32>(clamp(pos.x + d, lo, hi), pos.y));
        // A tap that is not a number gives nothing, as
        // `output_sharpen::blur` leaves it out.
        if (abs(v) <= 1e30) {
            let w = exp(-(d * d) / (2.0 * s * s));
            acc = acc + w * v;
            sum = sum + w;
        }
    }
    let v = acc / sum;
    return vec4<f32>(v, v, v, 1.0);
}
