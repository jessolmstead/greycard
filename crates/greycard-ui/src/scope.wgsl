// Bins of the analysis image: the histogram always, in the first
// 3 * levels bins, and after it the scope the panel shows. The
// histogram is of the encoded output, as an editor's histogram shows
// what is on the screen; so are the others.
//
// `scope.rs` bins the same way on the CPU and is the reference for
// this; change the two together.

struct Mode {
    // 0 the histogram alone, 1 the waveform, 2 the vectorscope.
    kind: u32,
    columns: u32,
    wheel: u32,
    levels: u32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> bins: array<atomic<u32>>;
@group(0) @binding(2) var<uniform> mode: Mode;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(src);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let c = clamp(textureLoad(src, vec2<i32>(id.xy), 0).rgb, vec3(0.0), vec3(1.0));
    let hist = 3u * mode.levels;
    let column = min(id.x * mode.columns / size.x, mode.columns - 1u);
    for (var ch = 0u; ch < 3u; ch++) {
        let level = u32(c[ch] * 255.0 + 0.5);
        atomicAdd(&bins[ch * mode.levels + level], 1u);
        if (mode.kind == 1u) {
            atomicAdd(&bins[hist + (ch * mode.columns + column) * mode.levels + level], 1u);
        }
    }
    if (mode.kind == 2u) {
        let y = dot(c, vec3(0.2126, 0.7152, 0.0722));
        let cb = (c.b - y) / 1.8556;
        let cr = (c.r - y) / 1.5748;
        let n = f32(mode.wheel);
        let u = floor((cb + 0.5) * n);
        let v = floor((0.5 - cr) * n);
        if (u >= 0.0 && v >= 0.0 && u < n && v < n) {
            atomicAdd(&bins[hist + u32(v) * mode.wheel + u32(u)], 1u);
        }
    }
}
