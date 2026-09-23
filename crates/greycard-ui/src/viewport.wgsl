// The viewport: the engine's linear Rec.2020 image drawn at a zoom and
// center, exposure applied, a display tone curve with a shoulder (or a
// clip, when the curve is off) in the working space, then the matrix
// to linear sRGB and the sRGB encoding. The target is plain 8-bit:
// Slint's renderer treats every texture as encoded bytes and writes
// them to a non-sRGB surface unchanged, so an sRGB-format target would
// be decoded on the way in and shown a gamma too dark.

struct Params {
    view: vec2<f32>,
    image: vec2<f32>,
    center: vec2<f32>,
    // The rectangle of the leveled plane on show; the plane's size;
    // the rows of the matrix taking a point of the plane about its
    // center to the source about its (`Geometry::matrix`); the
    // perspective's row in persp.xy (`Geometry::perspective`); and
    // whether that lands between pixels (cubic.x).
    frame_origin: vec2<f32>,
    frame_size: vec2<f32>,
    plane: vec2<f32>,
    turn0: vec2<f32>,
    turn1: vec2<f32>,
    persp: vec4<f32>,
    cubic: vec4<f32>,
    // Its x: which clipping warnings to paint over the picture, 1 the
    // shadows, 2 the highlights, 3 both. (cubic.z: the source is
    // encoded sRGB already, the camera's JPEG, and takes the second
    // path below, to the display table and nowhere else.)
    clip: vec4<f32>,
    zoom: f32,
    // Stops.
    exposure: f32,
    curve: f32,
    // Slope at mid grey relative to the base curve's.
    contrast: f32,
    // Stops of shift by the guide plane's luminance, and the black
    // point as a fraction of mid grey; `finish.rs` says where each acts.
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    // The color mixer is on.
    mixer: f32,
    // The color curves shift something.
    shaded: f32,
    // How many local adjustments, and which one's mask to paint over
    // the picture (its index plus one; nothing at zero).
    locals: f32,
    show_mask: f32,
    // Global vibrance and saturation, in the same pass as the mixer.
    color: f32,
    saturation: f32,
    vibrance: f32,
    // Keeps m0 16-byte aligned.
    pad0: f32,
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
    // The white balance preview: rows of a working-space matrix that
    // takes the develop's white point to the panel's, until the exact
    // develop lands.
    w0: vec4<f32>,
    w1: vec4<f32>,
    w2: vec4<f32>,
    // The working space to Oklab's LMS and back, rows.
    ok_in0: vec4<f32>,
    ok_in1: vec4<f32>,
    ok_in2: vec4<f32>,
    ok_out0: vec4<f32>,
    ok_out1: vec4<f32>,
    ok_out2: vec4<f32>,
    // The mixer's hue shift (degrees), chroma change and lightness
    // change for each of eight bands, two vec4s each.
    mix_hue0: vec4<f32>,
    mix_hue1: vec4<f32>,
    mix_sat0: vec4<f32>,
    mix_sat1: vec4<f32>,
    mix_lum0: vec4<f32>,
    mix_lum1: vec4<f32>,
    // Amount in stops, midpoint, feather, roundness (`vignette.rs`).
    vignette: vec4<f32>,
    // Amount, the lattice's level and the way through its octave,
    // then the kind's character (`grain.rs`): radius; spread, floor,
    // tint, norm.
    grain0: vec4<f32>,
    grain1: vec4<f32>,
    // The canvas outside the frame: a UI color (`Theme`'s, the
    // panel's choice), already encoded, so it skips the display
    // table below rather than going through it.
    canvas: vec4<f32>,
    // The black and white conversion: its switch in x, its strength
    // in y, and the eight bands' weights in two vec4s (`bw.rs`).
    // Global only: a local adjustment carries none.
    bw: vec4<f32>,
    bw_w0: vec4<f32>,
    bw_w1: vec4<f32>,
    // The tint as a vector, its amount in its hue's direction, in xy
    // (`tint.rs`): what the global look's and each local's add as.
    tint: vec4<f32>,
    // The tone equalizer's guide plane: in xy the source pixels
    // its texture covers, which is its size times the pixels to a
    // texel, so a source position divided by it is the texture's uv and
    // the sampler's bilinear is `Guide::at` in `finish.rs`; in z
    // whether there is one to read.
    guide: vec4<f32>,
    // The target pixel this view's top left corner sits at: nonzero
    // for a view drawn into a part of the target, the compare view's
    // tiles.
    tile: vec4<f32>,
    // The look table (`greycard_core::lut`): its strength in x, its
    // nodes an axis in y, which transfer function its input is in in
    // z (0 sRGB, 1 Rec.709, 2 linear, 3 a plain gamma) and that
    // gamma in w. Nothing at a strength of zero, which is also what a
    // view with no look is given.
    look: vec4<f32>,
    // The input value its first and last nodes stand for, per
    // channel: a `.cube`'s DOMAIN_MIN and DOMAIN_MAX.
    look_min: vec4<f32>,
    look_max: vec4<f32>,
    // The working space to the table's primaries, linear, rows, and
    // back again (`lut::Look::to_lut` and `from_lut`).
    look_in0: vec4<f32>,
    look_in1: vec4<f32>,
    look_in2: vec4<f32>,
    look_out0: vec4<f32>,
    look_out1: vec4<f32>,
    look_out2: vec4<f32>,
};

// A local adjustment: its look's parameters as the global ones are
// held, and its mask as a run of shapes. Its tables are at
// `local_tables[index * 512 ..]`, laid out as `curves`.
struct Local {
    exposure: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    mixer: f32,
    shaded: f32,
    shapes_start: u32,
    shapes_count: u32,
    invert: u32,
    enabled: u32,
    color: f32,
    saturation: f32,
    vibrance: f32,
    // Keeps what follows 16-byte aligned.
    pad0: f32,
    // This adjustment's tint as a vector, in xy.
    tint: vec4<f32>,
    mix_hue0: vec4<f32>,
    mix_hue1: vec4<f32>,
    mix_sat0: vec4<f32>,
    mix_sat1: vec4<f32>,
    mix_lum0: vec4<f32>,
    mix_lum1: vec4<f32>,
};

// A mask's shape, as `mask.rs` has it: kind 0 is linear (a: from,
// to), kind 1 radial (a: center, radii; b: angle in radians,
// feather), kind 2 a brush (its layer of `brushes`, its raster's
// aspect in b.x). The low two bits of the flags are the mode (0 adds,
// 1 subtracts, 2 intersects); bit 2 inverts.
struct Shape {
    kind: u32,
    flags: u32,
    layer: u32,
    pad: u32,
    a: vec4<f32>,
    b: vec4<f32>,
};

// The look at one pixel: the global look with the locals blended in
// by their masks.
struct Look {
    exposure: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    mixer: bool,
    shaded: bool,
    color: bool,
    saturation: f32,
    vibrance: f32,
    bw: bool,
    // The tint as a vector: the global look's, each local's added by
    // its mask's weight.
    tint: vec2<f32>,
    bw_w: array<f32, 8>,
    hue: array<f32, 8>,
    sat: array<f32, 8>,
    lum: array<f32, 8>,
};

const MAX_LOCALS: u32 = 16u;

// The mixer's band centers in Oklab hue, degrees, as `mixer.rs` has them.
const BAND_CENTERS: array<f32, 8> = array<f32, 8>(25.0, 60.0, 100.0, 140.0, 195.0, 265.0, 300.0, 335.0);
// Ottosson's matrices, columns as WGSL wants them.
const LMS_TO_LAB: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(0.21045426, 1.9779985, 0.025904037),
    vec3<f32>(0.7936178, -2.4285922, 0.78277177),
    vec3<f32>(-0.004072047, 0.4505937, -0.80867577),
);
const LAB_TO_LMS: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(1.0, 1.0, 1.0),
    vec3<f32>(0.39633778, -0.105561346, -0.08948418),
    vec3<f32>(0.21580376, -0.06385417, -1.2914855),
);

const MID_GREY: f32 = 0.18;
// Luminance weights of the working space, Rec.2020.
const LUMA: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
// The display table: encoded sRGB in, the monitor's encoded RGB out.
@group(0) @binding(3) var lut: texture_3d<f32>;
@group(0) @binding(4) var lut_samp: sampler;
// The curves, baked: red, green and blue outputs for each of 256
// encoded inputs, the master curve composed in; then, from entry 256,
// the color curves' shift of Oklab's a and b for each of 256
// lightnesses, in x and y.
@group(0) @binding(5) var<uniform> curves: array<vec4<f32>, 512>;
@group(0) @binding(6) var<storage, read> locals: array<Local>;
@group(0) @binding(7) var<storage, read> local_tables: array<vec4<f32>>;
@group(0) @binding(8) var<storage, read> shapes: array<Shape>;
// The brushes' rasters, a layer each, over u 0 to 1 and v 0 to the
// picture's aspect.
@group(0) @binding(9) var brushes: texture_2d_array<f32>;
// The tone equalizer's guide plane: the smoothed log luminance of the
// source over mid grey, in stops, at a reduced scale (`guide_plane` in
// `finish.rs`). One texel of mid grey when there is none.
@group(0) @binding(10) var guide: texture_2d<f32>;
// The look table, in its own encoding, read by node rather than
// sampled: the interpolation is tetrahedral and the hardware's is
// not. Two nodes an axis of identity when there is no look.
@group(0) @binding(11) var look_lut: texture_3d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the target.
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // Frame pixel under this target pixel, then the source pixel under
    // that through the turn, as `to_source` in the edit crate.
    let q = (pos.xy - p.tile.xy - p.view * 0.5) / p.zoom + p.center;
    if (q.x < 0.0 || q.y < 0.0 || q.x >= p.frame_size.x || q.y >= p.frame_size.y) {
        return vec4<f32>(p.canvas.rgb, 1.0);
    }
    // The perspective first, its divisor held above the horizon as
    // `to_source` holds it, then the turn.
    let r0 = p.frame_origin + q - p.plane * 0.5;
    let r = r0 / max(1.0 + dot(p.persp.xy, r0), 1e-4);
    let at = p.image * 0.5 + vec2<f32>(dot(p.turn0, r), dot(p.turn1, r));
    if (at.x < 0.0 || at.y < 0.0 || at.x >= p.image.x || at.y >= p.image.y) {
        return vec4<f32>(0.03, 0.03, 0.03, 1.0);
    }
    // The picture, and in its alpha the sharpen's blend mask.
    var t4: vec4<f32>;
    if (p.cubic.x < 0.5) {
        t4 = textureSampleLevel(src, samp, at / p.image, 0.0);
    } else {
        t4 = sample_cubic(at);
    }
    // The second path: the camera's JPEG, display-referred sRGB
    // already, so none of the look, the tone, the curves or the
    // output matrix applies. It goes through the monitor's table as
    // the encoded output does below, and that is all.
    if (p.cubic.z > 0.5) {
        let n = f32(textureDimensions(lut).x);
        let e = clamp(t4.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        let shown = textureSampleLevel(lut, lut_samp, e * (n - 1.0) / n + 0.5 / n, 0.0);
        return vec4<f32>(shown.rgb, 1.0);
    }
    let t = t4.rgb;
    // The look here: the global look, then each local's blended in
    // by its mask's value at this source position, as `finish_pixel`
    // in `finish.rs` does it.
    var look: Look;
    look.exposure = p.exposure;
    if (p.vignette.x != 0.0) {
        look.exposure = look.exposure + p.vignette.x * vignette_at(q / p.frame_size, p.frame_size.x / p.frame_size.y);
    }
    look.contrast = p.contrast;
    look.highlights = p.highlights;
    look.shadows = p.shadows;
    look.whites = p.whites;
    look.blacks = p.blacks;
    look.mixer = p.mixer > 0.5;
    look.shaded = p.shaded > 0.5;
    look.color = p.color > 0.5;
    look.saturation = select(0.0, p.saturation, look.color);
    look.vibrance = select(0.0, p.vibrance, look.color);
    look.bw = p.bw.x > 0.5;
    look.tint = p.tint.xy;
    for (var b = 0u; b < 8u; b = b + 1u) {
        // The strength is one scalar on the summed gain
        // (`BlackWhite::stops`), and nothing but the picture's own
        // section writes these, so it rides on the weights here.
        look.bw_w[b] = select(0.0, band_value(p.bw_w0, p.bw_w1, b) * p.bw.y, look.bw);
        look.hue[b] = select(0.0, band_value(p.mix_hue0, p.mix_hue1, b), look.mixer);
        look.sat[b] = select(0.0, band_value(p.mix_sat0, p.mix_sat1, b), look.mixer);
        look.lum[b] = select(0.0, band_value(p.mix_lum0, p.mix_lum1, b), look.mixer);
    }
    let count = min(u32(p.locals), MAX_LOCALS);
    var weights: array<f32, 16>;
    let uv = at / p.image.x;
    for (var k = 0u; k < count; k = k + 1u) {
        let l = locals[k];
        let m = mask_at(l, uv);
        weights[k] = m;
        let w = m * f32(l.enabled);
        if (w <= 0.0) { continue; }
        look.exposure = look.exposure + w * l.exposure;
        look.contrast = look.contrast + w * (l.contrast - 1.0);
        look.highlights = look.highlights + w * l.highlights;
        look.shadows = look.shadows + w * l.shadows;
        look.whites = look.whites + w * l.whites;
        look.blacks = look.blacks + w * l.blacks;
        if (l.mixer > 0.5) {
            look.mixer = true;
            for (var b = 0u; b < 8u; b = b + 1u) {
                look.hue[b] = look.hue[b] + w * band_value(l.mix_hue0, l.mix_hue1, b);
                look.sat[b] = look.sat[b] + w * band_value(l.mix_sat0, l.mix_sat1, b);
                look.lum[b] = look.lum[b] + w * band_value(l.mix_lum0, l.mix_lum1, b);
            }
        }
        if (l.color > 0.5) {
            look.color = true;
            look.saturation = look.saturation + w * l.saturation;
            look.vibrance = look.vibrance + w * l.vibrance;
        }
        look.tint = look.tint + w * l.tint.xy;
        if (l.shaded > 0.5) { look.shaded = true; }
    }
    var c = vec3<f32>(dot(p.w0.xyz, t), dot(p.w1.xyz, t), dot(p.w2.xyz, t)) * exp2(look.exposure);
    if (look.mixer || look.color || look.bw || length(look.tint) > 0.0) {
        // The mixer and the black and white read their hue from the
        // source's mean a and b about this pixel, brought to this
        // exposure: Oklab's a and b scale with the cube root of a gain.
        var ab = vec2<f32>(0.0);
        let by_mean = look.mixer || look.bw;
        if (by_mean) {
            ab = local_ab(at) * exp2(look.exposure / 3.0);
        }
        c = mix_color(c, look, ab, by_mean);
    }
    if (p.curve > 0.5) {
        // The guide is the scene's, before any exposure, as `local_ab`
        // is: the exposure here is a shift of it in stops and the
        // contrast a scale. Nothing to read means the pixel's own
        // luminance, as `shape` in `finish.rs` falls back to.
        var g = 0.0;
        let has_guide = p.guide.z > 0.5;
        if (has_guide) {
            g = look.contrast * (textureSampleLevel(guide, samp, at / p.guide.xy, 0.0).r + look.exposure);
        }
        c = tone(shape(c, look, g, has_guide));
    } else {
        c = min(c, vec3<f32>(1.0));
    }
    // The point curves, on the encoded working-space value, before the
    // matrix, as `finish.rs` does; then the color curves.
    c = decode(curve(encode(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))), count, weights));
    if (look.shaded) {
        c = shade(c, count, weights);
    }
    // The look table, last before the output transform, as
    // `finish_pixel_with` has it.
    if (p.look.x > 0.0) {
        c = look_at(c);
    }
    // The proof's gamut mark is looked up at the working-space color,
    // before the output matrix clamps it into its space.
    let wide = encode(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
    let s = vec3<f32>(dot(p.m0.xyz, c), dot(p.m1.xyz, c), dot(p.m2.xyz, c));
    var e = encode(clamp(s, vec3<f32>(0.0), vec3<f32>(1.0)));
    if (p.grain0.x > 0.0) {
        e = grain_apply(grain_at(q / p.frame_size, p.frame_size.x / p.frame_size.y), e);
    }
    // Sample the table at its grid: the first and last points sit half
    // a texel in from the edges.
    let n = f32(textureDimensions(lut).x);
    let shown = textureSampleLevel(lut, lut_samp, e * (n - 1.0) / n + 0.5 / n, 0.0);
    var d = shown.rgb;
    // The soft proof's gamut warning, from the table's alpha at the
    // working-space color: grey where the proof's profile cannot
    // hold it.
    let mark = textureSampleLevel(lut, lut_samp, wide * (n - 1.0) / n + 0.5 / n, 0.0);
    if (mark.a > 0.5) {
        d = vec3<f32>(0.5, 0.5, 0.5);
    }
    // A mask on show: red over the picture where it acts.
    if (p.show_mask > 0.5) {
        let k = u32(p.show_mask) - 1u;
        if (k < count) {
            d = mix(d, vec3<f32>(1.0, 0.15, 0.15), 0.6 * weights[k]);
        }
    }
    // The sharpen's mask on show, the same way.
    if (p.cubic.y > 0.5) {
        d = mix(d, vec3<f32>(1.0, 0.15, 0.15), 0.6 * clamp(t4.a, 0.0, 1.0));
    }
    // The clipping warnings: where a channel of the output lands in
    // the histogram's first or last bin, as `scope.wgsl` rounds it,
    // the pixel is painted blue for the shadows or red for the
    // highlights, the highlights over the shadows.
    let clip = u32(p.clip.x);
    if (clip != 0u) {
        let level = floor(e * 255.0 + 0.5);
        let lo = min(min(level.r, level.g), level.b) <= 0.0;
        let hi = max(max(level.r, level.g), level.b) >= 255.0;
        if ((clip & 2u) != 0u && hi) {
            d = vec3<f32>(1.0, 0.15, 0.15);
        } else if ((clip & 1u) != 0u && lo) {
            d = vec3<f32>(0.2, 0.4, 1.0);
        }
    }
    return vec4<f32>(d, 1.0);
}

// The grain, as `grain.rs` has it: the hash, a cell's crystals from
// its bits, the soft bumps of the cell and its neighbors summed and
// tinted, and how it goes on.
fn grain_hash(x: u32, y: u32, seed: u32) -> u32 {
    var h = (x * 0x8da6b343u) ^ (y * 0xd8163841u) ^ (seed * 0xcb1ab31fu);
    h = h ^ (h >> 15u);
    h = h * 0x2c1b3c6du;
    h = h ^ (h >> 12u);
    h = h * 0x297a2d39u;
    h = h ^ (h >> 15u);
    return h;
}

// Which of a cell's four candidates stands for it one level up.
fn grain_pick(level: u32, cell: vec2<u32>) -> u32 {
    return grain_hash(cell.x, cell.y, 0x100u + level) & 3u;
}

fn grain_at(uv: vec2<f32>, aspect: f32) -> vec3<f32> {
    let level = u32(p.grain0.y);
    let f = p.grain0.z;
    let pitch = f32(1u << level);
    let base_cell = 0.1 / 1000.0;
    let x = uv.x / base_cell + 64.0;
    let y = uv.y / aspect / base_cell + 64.0;
    let ix = u32(max(floor(x / pitch), 1.0));
    let iy = u32(max(floor(y / pitch), 1.0));
    let fade = (4.0 / (f * f) - 1.0) / 3.0;
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < 3u; j = j + 1u) {
        for (var i = 0u; i < 3u; i = i + 1u) {
            let cell = vec2<u32>(ix + i - 1u, iy + j - 1u);
            let kept = grain_pick(level, cell);
            for (var d = 0u; d < 4u; d = d + 1u) {
                // The candidate's home, followed down the levels.
                var home = cell;
                var slot = d;
                if (level > 0u) {
                    home = vec2<u32>(2u * cell.x + (d & 1u), 2u * cell.y + (d >> 1u));
                    var l = level - 1u;
                    for (; l > 0u; l = l - 1u) {
                        let c = grain_pick(l, home);
                        home = vec2<u32>(2u * home.x + (c & 1u), 2u * home.y + (c >> 1u));
                    }
                    slot = grain_pick(0u, home);
                }
                let h = grain_hash(home.x, home.y, slot + 1u);
                let ox = f32(h & 0x3fu) / 63.0;
                let oy = f32((h >> 6u) & 0x3fu) / 63.0;
                let r = f32((h >> 12u) & 0x1fu) / 31.0;
                let amp = f32((h >> 17u) & 0x7fu) / 127.0 * 2.0 - 1.0;
                let t1 = f32((h >> 24u) & 0xfu) / 15.0 * 2.0 - 1.0;
                let t2 = f32((h >> 28u) & 0xfu) / 15.0 * 2.0 - 1.0;
                let radius = (p.grain0.w + p.grain1.x * r) * f * pitch;
                let dx = f32(home.x) + ox - x;
                let dy = f32(home.y) + oy - y;
                let q = (dx * dx + dy * dy) / (radius * radius);
                if (q >= 1.0) { continue; }
                let bump = (1.0 - q) * (1.0 - q);
                let strength = sign(amp) * (p.grain1.y + (1.0 - p.grain1.y) * abs(amp));
                let weight = select(fade, 1.0, d == kept);
                let g = bump * strength * weight;
                sum = sum + g * vec3<f32>(1.0 + p.grain1.z * t1, 1.0 + p.grain1.z * t2, 1.0 - p.grain1.z * t1);
            }
        }
    }
    return clamp(p.grain0.x, 0.0, 1.0) * p.grain1.w * sum;
}

// The tonal weighting: peaks at 0.6, in the midtones and upper
// midtones, easing off towards both ends without going under half
// its peak, as `Grain::weight` in `grain.rs` has it (see there for
// why: a scanned negative's grain is flattest in the midtones, not
// the shadows).
fn grain_apply(noise: vec3<f32>, e: vec3<f32>) -> vec3<f32> {
    let luma = clamp(0.2126 * e.x + 0.7152 * e.y + 0.0722 * e.z, 0.0, 1.0);
    let r = luma / 0.6;
    let bump = r * sqrt(r) * ((1.0 - luma) / 0.4);
    let w = 0.12 * 0.8 * (0.55 + 0.45 * bump);
    return clamp(e + noise * w, vec3<f32>(0.0), vec3<f32>(1.0));
}

// How much of the vignette acts at a fraction of the frame, as
// `Vignette::at` in `vignette.rs`.
fn vignette_at(uv: vec2<f32>, aspect: f32) -> f32 {
    let pq = 2.0 * uv - 1.0;
    let r = clamp(p.vignette.w, -1.0, 1.0);
    let stretch = pow(select(1.0 / aspect, aspect, aspect >= 1.0), max(r, 0.0));
    var xy: vec2<f32>;
    if (aspect >= 1.0) {
        xy = vec2<f32>(pq.x, pq.y / stretch);
    } else {
        xy = vec2<f32>(pq.x / stretch, pq.y);
    }
    var n = 2.0;
    if (r < 0.0) { n = 2.0 / (1.0 + 0.85 * r); }
    let d = pow(pow(abs(xy.x), n) + pow(abs(xy.y), n), 1.0 / n) / sqrt(2.0);
    let m = clamp(p.vignette.y, 0.0, 1.0);
    let e1 = m + clamp(p.vignette.z, 0.0, 1.0);
    if (e1 - m < 1e-6) {
        return select(0.0, 1.0, d >= e1);
    }
    return smoothstep(m, e1, d);
}

// A shape's value at a source position in units of the width, as
// `Shape::at` in `mask.rs`.
fn shape_at(sh: Shape, uv: vec2<f32>) -> f32 {
    var s = 0.0;
    if (sh.kind == 0u) {
        let d = sh.a.zw - sh.a.xy;
        let len2 = dot(d, d);
        if (len2 >= 1e-12) {
            s = 1.0 - clamp(dot(uv - sh.a.xy, d) / len2, 0.0, 1.0);
        }
    } else if (sh.kind == 2u) {
        s = textureSampleLevel(brushes, samp, vec2<f32>(uv.x, uv.y / sh.b.x), sh.layer, 0.0).r;
    } else if (sh.kind == 3u) {
        // A raster shape whose raster is not made yet: nothing, which
        // is what `Local::weight` reads a missing raster as. It takes
        // no layer, so it cannot sample the next shape's paint, and
        // its mode and its invert still act, as they do there.
        s = 0.0;
    } else {
        let sc = vec2<f32>(sin(sh.b.x), cos(sh.b.x));
        let dxy = uv - sh.a.xy;
        let xy = vec2<f32>(sc.y * dxy.x + sc.x * dxy.y, -sc.x * dxy.x + sc.y * dxy.y) / max(sh.a.zw, vec2<f32>(1e-6));
        let e = length(xy);
        let feather = clamp(sh.b.y, 0.0, 1.0);
        if (feather < 1e-6) {
            s = select(0.0, 1.0, e < 1.0);
        } else {
            s = 1.0 - smoothstep(1.0 - feather, 1.0, e);
        }
    }
    if ((sh.flags & 4u) != 0u) { s = 1.0 - s; }
    return s;
}

// A local's mask at a source position: its shapes joined or taken
// away in order, as `Mask::at`. Only the switched-on shapes are
// uploaded (`Mask::live`), so one switched off is absent from the
// join here as it is there, whatever its mode.
fn mask_at(l: Local, uv: vec2<f32>) -> f32 {
    // Nothing live is nothing everywhere, `invert` or not, as `Mask::at_with`.
    if (l.shapes_count == 0u) { return 0.0; }
    var acc = 0.0;
    for (var i = 0u; i < l.shapes_count; i = i + 1u) {
        let sh = shapes[l.shapes_start + i];
        let s = shape_at(sh, uv);
        let mode = sh.flags & 3u;
        if (mode == 1u) {
            acc = acc * (1.0 - s);
        } else if (mode == 2u) {
            acc = acc * s;
        } else {
            acc = acc + s - acc * s;
        }
    }
    if (l.invert != 0u) { acc = 1.0 - acc; }
    return acc;
}

// Where each shift acts, in stops over mid grey, as `finish.rs` has them.
const SHADOWS_RAMP: vec2<f32> = vec2<f32>(-3.5, 0.0);
const HIGHLIGHTS_RAMP: vec2<f32> = vec2<f32>(-1.0, 2.5);
// The highlights' share by stops over grey, as `highlights_weight` in
// `finish.rs`: rising to a peak over the ramp, easing back towards
// display white. `DISPLAY_WHITE_STOPS` is the shoulder's, below.
const HIGHLIGHTS_PEAK: f32 = 0.55;
const HIGHLIGHTS_EASE_FROM: f32 = 2.0;
const HIGHLIGHTS_EASE_DEPTH: f32 = 0.4;

fn highlights_weight(g: f32) -> f32 {
    return HIGHLIGHTS_PEAK * smoothstep(HIGHLIGHTS_RAMP.x, HIGHLIGHTS_RAMP.y, g)
        * (1.0 - HIGHLIGHTS_EASE_DEPTH * smoothstep(HIGHLIGHTS_EASE_FROM, DISPLAY_WHITE_STOPS, g));
}
// How far the blended whites may go either way, as `WHITES_RANGE` in
// `finish.rs`: the exponent has a pole at display white and the sliders
// of a picture and its masks add, so the value arriving here is not
// the slider's ±2.
const WHITES_RANGE: vec2<f32> = vec2<f32>(-3.5, 2.0);

// The whites control, as `white_point` in `finish.rs`: a power about
// mid grey on the luminance over it, its exponent eased in over the
// first stop, that brings a luminance `whites` stops under display white
// (the shoulder's `DISPLAY_WHITE_STOPS`, where the raw clips) to display
// white; nothing at or under mid grey. A gain on all three
// channels, so hue holds.
fn white_point(c: vec3<f32>, whites: f32) -> vec3<f32> {
    let y = dot(LUMA, c);
    if (whites == 0.0 || y <= MID_GREY) {
        return c;
    }
    let w = clamp(whites, WHITES_RANGE.x, WHITES_RANGE.y);
    let u = log2(y / MID_GREY);
    var p = DISPLAY_WHITE_STOPS / (DISPLAY_WHITE_STOPS - w);
    p = 1.0 + (p - 1.0) * smoothstep(0.0, 1.0, u);
    return c * exp2(u * (p - 1.0));
}

// The edit's shape of the scene before the curve, as `shape` in
// `finish.rs`: contrast as a power about mid grey per channel, before
// the shoulder, so the mid-tone slope changes and the top still rolls
// off; then the shifts, a gain from the region's luminance (`g`, the
// guide plane's, already at this pixel's exposure and contrast) so the
// texture inside a region keeps its contrast and its hue holds; then
// the white point; then the black point with scene white held.
fn shape(x: vec3<f32>, look: Look, g: f32, has_guide: bool) -> vec3<f32> {
    let c = MID_GREY * pow(max(x, vec3<f32>(0.0)) / MID_GREY, vec3<f32>(look.contrast));
    let l = select(log2(max(dot(LUMA, c), 1e-6) / MID_GREY), g, has_guide);
    let shift = look.shadows * (1.0 - smoothstep(SHADOWS_RAMP.x, SHADOWS_RAMP.y, l))
        + look.highlights * highlights_weight(l);
    return black_point(white_point(c * exp2(shift), look.whites), look.blacks);
}

// The blacks, as `black_point` in `finish.rs`: a crush is an offset
// and a scale that hold scene white; a lift is a gain on the luminance,
// `BLACKS_LIFT` stops at the slider's top, full in the deep shadows and
// fading out over `BLACKS_RAMP`, so it opens them without a veil.
const BLACKS_TOP: f32 = 0.3;
const BLACKS_LIFT: f32 = 1.2;
const BLACKS_RAMP: vec2<f32> = vec2<f32>(-5.0, 1.5);

fn black_point(c: vec3<f32>, blacks: f32) -> vec3<f32> {
    if (blacks <= 0.0) {
        let black = -blacks * MID_GREY;
        return max((c - black) / (1.0 - black), vec3<f32>(0.0));
    }
    let u = log2(max(dot(LUMA, c), 1e-9) / MID_GREY);
    let fade = 1.0 - smoothstep(BLACKS_RAMP.x, BLACKS_RAMP.y, u);
    return max(c, vec3<f32>(0.0)) * exp2(BLACKS_LIFT * blacks / BLACKS_TOP * fade);
}

fn band_value(v0: vec4<f32>, v1: vec4<f32>, i: u32) -> f32 {
    if (i < 4u) { return v0[i]; }
    return v1[i - 4u];
}

fn signed_cbrt(v: vec3<f32>) -> vec3<f32> {
    return sign(v) * pow(abs(v), vec3<f32>(1.0 / 3.0));
}

fn wrap360(v: f32) -> f32 {
    return v - 360.0 * floor(v / 360.0);
}

// The Oklab chroma the mixer trusts a hue fully at, and the mean's
// radius, as `mixer.rs` has them.
const CHROMA_FULL: f32 = 0.03;
const MEAN_RADIUS: i32 = 2;
// Stops of light at a black and white weight of one, as `bw.rs` has it.
const BW_RANGE: f32 = 1.0;

// The mean Oklab a and b of the source about a position, a 5x5 box
// with the edges clamped, as `local_ab` in `finish.rs`: each tap
// through the white balance and to Oklab, then the mean.
fn local_ab(s: vec2<f32>) -> vec2<f32> {
    let limit = vec2<i32>(p.image) - vec2<i32>(1, 1);
    let center = vec2<i32>(floor(s));
    var acc = vec2<f32>(0.0);
    for (var j = -MEAN_RADIUS; j <= MEAN_RADIUS; j = j + 1) {
        let sy = clamp(center.y + j, 0, limit.y);
        for (var i = -MEAN_RADIUS; i <= MEAN_RADIUS; i = i + 1) {
            let sx = clamp(center.x + i, 0, limit.x);
            let t = textureLoad(src, vec2<i32>(sx, sy), 0).rgb;
            let c = vec3<f32>(dot(p.w0.xyz, t), dot(p.w1.xyz, t), dot(p.w2.xyz, t));
            let lms = signed_cbrt(vec3<f32>(dot(p.ok_in0.xyz, c), dot(p.ok_in1.xyz, c), dot(p.ok_in2.xyz, c)));
            acc = acc + (LMS_TO_LAB * lms).yz;
        }
    }
    let n = f32(2 * MEAN_RADIUS + 1);
    return acc / (n * n);
}

// The Oklab chroma a mid grey takes at a full tint, and the
// lightness the tint is strongest at, as `tint.rs` has them.
const TINT_CHROMA: f32 = 0.1;
const TINT_MID: f32 = 0.5646;

// A tint on Oklab's a and b at a lightness, as `Tint::applied` in
// `tint.rs`: the vector's length is the amount and its direction the
// hue. The chroma rises toward a floor that peaks at TINT_MID and is
// nothing at black or at twice it; the hue turns toward the tint's by
// the share of the chroma the tint answers for, so a neutral takes
// the tint's hue outright rather than one read off its own noise.
fn tint_ab(tint: vec2<f32>, lightness: f32, ab: vec2<f32>) -> vec2<f32> {
    let amount = clamp(length(tint), 0.0, 1.0);
    if (amount <= 0.0) { return ab; }
    let want_h = degrees(atan2(tint.y, tint.x));
    let d = (lightness - TINT_MID) / TINT_MID;
    let floor_c = TINT_CHROMA * max(1.0 - d * d, 0.0);
    let chroma = length(ab);
    let want_c = max(chroma, floor_c);
    let sum = (1.0 - amount) * chroma + amount * want_c;
    if (sum <= 0.0) { return ab; }
    let share = amount * want_c / sum;
    let chroma2 = chroma + amount * (want_c - chroma);
    let hue = degrees(atan2(ab.y, ab.x));
    let delta = wrap360(want_h - hue + 180.0) - 180.0;
    let h = radians(hue + share * delta);
    return vec2<f32>(chroma2 * cos(h), chroma2 * sin(h));
}

// The color mixer, as `mix_with` in `finish.rs`: to Oklab; the hue
// and the confidence read from `ab`, the local mean, when `by_mean`,
// else from the pixel; the two bands the hue lies between share the
// shift of hue and the scales of chroma and lightness, faded by the
// confidence toward grey; applied to the pixel's own a and b; the
// black and white's conversion to a neutral last; back.
fn mix_color(c: vec3<f32>, look: Look, ab: vec2<f32>, by_mean: bool) -> vec3<f32> {
    let lms = signed_cbrt(vec3<f32>(dot(p.ok_in0.xyz, c), dot(p.ok_in1.xyz, c), dot(p.ok_in2.xyz, c)));
    let lab = LMS_TO_LAB * lms;
    let chroma = length(lab.yz);
    let hue = wrap360(degrees(atan2(lab.z, lab.y)));
    let seen_ab = select(lab.yz, ab, by_mean);
    let seen = wrap360(degrees(atan2(seen_ab.y, seen_ab.x)));
    let trust = smoothstep(0.0, CHROMA_FULL, length(seen_ab));
    var above = 0u;
    for (var k = 0u; k < 8u; k = k + 1u) {
        if (BAND_CENTERS[k] > seen) { above = k; break; }
    }
    let below = (above + 7u) % 8u;
    let span = wrap360(BAND_CENTERS[above] - BAND_CENTERS[below]);
    let t = clamp(wrap360(seen - BAND_CENTERS[below]) / max(span, 1e-3), 0.0, 1.0);
    let shift = trust * mix(look.hue[below], look.hue[above], t);
    let sat = trust * mix(look.sat[below], look.sat[above], t);
    let lum = trust * mix(look.lum[below], look.lum[above], t);
    let h = radians(hue + shift);
    let light = exp2(lum / 3.0);
    let chroma2 = chroma * max(1.0 + sat, 0.0) * light;
    let chroma3 = select(chroma2, chroma2 * color_scale(chroma2, hue + shift, look), look.color);
    // The black and white has the last word here, as `mix_with` in
    // `finish.rs` gives it: the chroma goes, and the band's weight,
    // shared and faded exactly as a mixer slider is, is one more gain
    // on the lightness (`BlackWhite::light` in `bw.rs`).
    var lab2 = vec3<f32>(lab.x * light, chroma3 * cos(h), chroma3 * sin(h));
    if (look.bw) {
        let bw = trust * mix(look.bw_w[below], look.bw_w[above], t) * BW_RANGE;
        lab2 = vec3<f32>(lab.x * light * exp2(bw / 3.0), 0.0, 0.0);
    }
    // The tint last, after the conversion and by addition, as
    // `Tint::applied` in `tint.rs` does it: a mono picture takes a
    // local tint, which is what hand-coloring is.
    lab2 = vec3<f32>(lab2.x, tint_ab(look.tint, lab2.x, lab2.yz));
    let lms2 = LAB_TO_LMS * lab2;
    let lin = lms2 * lms2 * lms2;
    return vec3<f32>(dot(p.ok_out0.xyz, lin), dot(p.ok_out1.xyz, lin), dot(p.ok_out2.xyz, lin));
}

// Global vibrance and saturation's further scale of chroma, as
// `Color::scale` in `color.rs`: saturation flat, vibrance leaning
// on the room left below an Oklab chroma of 0.3 and backing off
// toward the skin hue at 55 degrees, where it acts at half.
fn color_scale(chroma: f32, hue_degrees: f32, look: Look) -> f32 {
    let sat_scale = max(1.0 + look.saturation, 0.0);
    let room = clamp(1.0 - chroma / 0.3, 0.0, 1.0);
    let d = abs(wrap360(hue_degrees - 55.0 + 180.0) - 180.0);
    let protect = 1.0 - 0.5 * (1.0 - smoothstep(15.0, 45.0, d));
    let vib_scale = max(1.0 + look.vibrance * room * protect, 0.0);
    return sat_scale * vib_scale;
}

// The color curves, as `shade` in `finish.rs`: to Oklab, a and b
// shifted by the global table at the lightness and each local's by
// its weight, back.
fn shade(c: vec3<f32>, count: u32, weights: array<f32, 16>) -> vec3<f32> {
    let lms = signed_cbrt(vec3<f32>(dot(p.ok_in0.xyz, c), dot(p.ok_in1.xyz, c), dot(p.ok_in2.xyz, c)));
    let lab = LMS_TO_LAB * lms;
    let v = clamp(lab.x, 0.0, 1.0) * 255.0;
    let i = min(u32(v), 254u);
    let f = v - f32(i);
    var shift = mix(curves[256u + i].xy, curves[257u + i].xy, f);
    for (var k = 0u; k < count; k = k + 1u) {
        let w = weights[k] * f32(locals[k].enabled);
        if (w <= 0.0 || locals[k].shaded < 0.5) { continue; }
        let base = k * 512u + 256u + i;
        shift = shift + w * mix(local_tables[base].xy, local_tables[base + 1u].xy, f);
    }
    let lab2 = vec3<f32>(lab.x, lab.yz + shift);
    let lms2 = LAB_TO_LMS * lab2;
    let lin = lms2 * lms2 * lms2;
    return vec3<f32>(dot(p.ok_out0.xyz, lin), dot(p.ok_out1.xyz, lin), dot(p.ok_out2.xyz, lin));
}

// Catmull-Rom's weights for the four taps about a position `t` past
// the second, as `geometry.rs` has them.
fn cubic_weights(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return 0.5 * vec4<f32>(-t3 + 2.0 * t2 - t, 3.0 * t3 - 5.0 * t2 + 2.0, -3.0 * t3 + 4.0 * t2 + t, t3 - t2);
}

// The source at a position, pixel centers at halves, edges clamped.
fn sample_cubic(s: vec2<f32>) -> vec4<f32> {
    let f = floor(s - 0.5);
    let wx = cubic_weights(s.x - 0.5 - f.x);
    let wy = cubic_weights(s.y - 0.5 - f.y);
    let limit = vec2<i32>(p.image) - vec2<i32>(1, 1);
    var out = vec4<f32>(0.0);
    for (var j = 0; j < 4; j = j + 1) {
        let sy = clamp(i32(f.y) + j - 1, 0, limit.y);
        for (var i = 0; i < 4; i = i + 1) {
            let sx = clamp(i32(f.x) + i - 1, 0, limit.x);
            out = out + wx[i] * wy[j] * textureLoad(src, vec2<i32>(sx, sy), 0);
        }
    }
    return out;
}

// The sRGB transfer function.
fn encode(v: vec3<f32>) -> vec3<f32> {
    let low = v * 12.92;
    let high = 1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, v <= vec3<f32>(0.0031308));
}

fn decode(v: vec3<f32>) -> vec3<f32> {
    let low = v / 12.92;
    let high = pow((v + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, v <= vec3<f32>(0.04045));
}

// Each channel through the locals' tables, each adding its departure
// from the line by its weight, then through the global table,
// linearly between entries.
fn curve(e: vec3<f32>, count: u32, weights: array<f32, 16>) -> vec3<f32> {
    var x = e;
    for (var k = 0u; k < count; k = k + 1u) {
        let w = weights[k] * f32(locals[k].enabled);
        if (w <= 0.0) { continue; }
        let v = e * 255.0;
        let i = min(vec3<u32>(v), vec3<u32>(254u));
        let f = v - vec3<f32>(i);
        let base = k * 512u;
        let a = vec3<f32>(local_tables[base + i.x].x, local_tables[base + i.y].y, local_tables[base + i.z].z);
        let b = vec3<f32>(local_tables[base + i.x + 1u].x, local_tables[base + i.y + 1u].y, local_tables[base + i.z + 1u].z);
        x = x + w * (a + f * (b - a) - e);
    }
    let v = clamp(x, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0;
    let i = min(vec3<u32>(v), vec3<u32>(254u));
    let f = v - vec3<f32>(i);
    let a = vec3<f32>(curves[i.x].x, curves[i.y].y, curves[i.z].z);
    let b = vec3<f32>(curves[i.x + 1u].x, curves[i.y + 1u].y, curves[i.z + 1u].z);
    return a + f * (b - a);
}

// The look table's own transfer function, `lut::Encoding` on the CPU:
// 0 sRGB, 1 Rec.709, 2 linear, 3 a plain gamma in `p.look.w`.
fn look_encode(v: vec3<f32>) -> vec3<f32> {
    let which = u32(p.look.z);
    if (which == 0u) {
        return encode(v);
    } else if (which == 1u) {
        let low = v * 4.5;
        let high = 1.099 * pow(v, vec3<f32>(0.45)) - 0.099;
        return select(high, low, v < vec3<f32>(0.018));
    } else if (which == 2u) {
        return v;
    }
    // Sign times the power of the magnitude, as `Encoding::Gamma` on
    // the CPU: `pow` of a negative number is NaN.
    return sign(v) * pow(abs(v), vec3<f32>(1.0 / max(p.look.w, 1e-3)));
}

fn look_decode(v: vec3<f32>) -> vec3<f32> {
    let which = u32(p.look.z);
    if (which == 0u) {
        return decode(v);
    } else if (which == 1u) {
        let low = v / 4.5;
        let high = pow((v + 0.099) / 1.099, vec3<f32>(1.0 / 0.45));
        return select(high, low, v < vec3<f32>(0.081));
    } else if (which == 2u) {
        return v;
    }
    return sign(v) * pow(abs(v), vec3<f32>(max(p.look.w, 1e-3)));
}

fn look_node(r: i32, g: i32, b: i32) -> vec3<f32> {
    return textureLoad(look_lut, vec3<i32>(r, g, b), 0).rgb;
}

// The table at a color in its own encoding, tetrahedrally, as
// `Lut3d::sample` on the CPU: the cell split into six tetrahedra that
// all share the black-to-white diagonal, so a neutral is a blend of
// two neutral corners and comes out neutral exactly.
fn look_sample(e: vec3<f32>) -> vec3<f32> {
    let last = p.look.y - 1.0;
    let span = p.look_max.xyz - p.look_min.xyz;
    let pos = clamp((e - p.look_min.xyz) / span * last, vec3<f32>(0.0), vec3<f32>(last));
    let top = i32(last) - 1;
    let i = clamp(vec3<i32>(floor(pos)), vec3<i32>(0), vec3<i32>(top));
    let f = pos - vec3<f32>(i);
    let c000 = look_node(i.x, i.y, i.z);
    var er: vec3<f32>;
    var eg: vec3<f32>;
    var eb: vec3<f32>;
    if (f.x > f.y) {
        if (f.y > f.z) {
            er = look_node(i.x + 1, i.y, i.z) - c000;
            eg = look_node(i.x + 1, i.y + 1, i.z) - look_node(i.x + 1, i.y, i.z);
            eb = look_node(i.x + 1, i.y + 1, i.z + 1) - look_node(i.x + 1, i.y + 1, i.z);
        } else if (f.x > f.z) {
            er = look_node(i.x + 1, i.y, i.z) - c000;
            eg = look_node(i.x + 1, i.y + 1, i.z + 1) - look_node(i.x + 1, i.y, i.z + 1);
            eb = look_node(i.x + 1, i.y, i.z + 1) - look_node(i.x + 1, i.y, i.z);
        } else {
            er = look_node(i.x + 1, i.y, i.z + 1) - look_node(i.x, i.y, i.z + 1);
            eg = look_node(i.x + 1, i.y + 1, i.z + 1) - look_node(i.x + 1, i.y, i.z + 1);
            eb = look_node(i.x, i.y, i.z + 1) - c000;
        }
    } else if (f.z > f.y) {
        er = look_node(i.x + 1, i.y + 1, i.z + 1) - look_node(i.x, i.y + 1, i.z + 1);
        eg = look_node(i.x, i.y + 1, i.z + 1) - look_node(i.x, i.y, i.z + 1);
        eb = look_node(i.x, i.y, i.z + 1) - c000;
    } else if (f.z > f.x) {
        er = look_node(i.x + 1, i.y + 1, i.z + 1) - look_node(i.x, i.y + 1, i.z + 1);
        eg = look_node(i.x, i.y + 1, i.z) - c000;
        eb = look_node(i.x, i.y + 1, i.z + 1) - look_node(i.x, i.y + 1, i.z);
    } else {
        er = look_node(i.x + 1, i.y + 1, i.z) - look_node(i.x, i.y + 1, i.z);
        eg = look_node(i.x, i.y + 1, i.z) - c000;
        eb = look_node(i.x + 1, i.y + 1, i.z + 1) - look_node(i.x + 1, i.y + 1, i.z);
    }
    return c000 + f.x * er + f.y * eg + f.z * eb;
}

// One working-space linear color through the look, as `lut::Look::at`
// on the CPU: into the table's primaries, clipped into them (an sRGB
// table has nothing to say about a Rec.2020 green, and the clip is
// the gamut map), encoded, looked up, decoded, back, and blended with
// what came in by the strength.
fn look_at(c: vec3<f32>) -> vec3<f32> {
    let l = vec3<f32>(dot(p.look_in0.xyz, c), dot(p.look_in1.xyz, c), dot(p.look_in2.xyz, c));
    let e = look_encode(clamp(l, vec3<f32>(0.0), vec3<f32>(1.0)));
    let o = look_decode(look_sample(e));
    let back = vec3<f32>(dot(p.look_out0.xyz, o), dot(p.look_out1.xyz, o), dot(p.look_out2.xyz, o));
    return c + p.look.x * (back - c);
}

// A display curve for a linear scene, as `tone` in `finish.rs`:
// Narkowicz's fit of the ACES output transform, per channel, untouched
// up to mid grey; over it a gain eased in by a smoothstep in stops
// until the fit reaches exactly one at the sensor's clip, where the
// baseline puts it, and clipped past that. Mid grey rises by about
// half a stop and a channel the raw clipped is white. Not the engine's
// business (§5): a consumer's choice, and this consumer's default.
const DISPLAY_WHITE_STOPS: f32 = 3.27;
const SHOULDER_GAIN: f32 = 1.114332;

fn tone(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    let fit = (x * (a * x + b)) / (x * (c * x + d) + e);
    let u = log2(max(x, vec3<f32>(1e-9)) / MID_GREY);
    let gain = 1.0 + (SHOULDER_GAIN - 1.0) * smoothstep(vec3<f32>(0.0), vec3<f32>(DISPLAY_WHITE_STOPS), u);
    return clamp(fit * gain, vec3<f32>(0.0), vec3<f32>(1.0));
}
