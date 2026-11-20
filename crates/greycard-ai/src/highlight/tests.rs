use std::path::{Path, PathBuf};

use greycard_core::develop::highlight_fill::net::{
    CONTEXT, ContextMap, FEATURES, OUTPUTS, PARAMETERS, TENSORS, grid_point,
};
use sha2::{Digest, Sha256};

use super::*;

#[test]
fn hl_1_blob_is_the_one_checked_in() {
    let hex: String = Sha256::digest(HL_1.blob)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hex, HL_1.sha256);
}

#[test]
fn hl_1_parses_to_every_tensor() {
    let w = HL_1.weights();
    assert_eq!(w.parameters(), PARAMETERS);
    assert_eq!(PARAMETERS, 57_964);
    let tensors = w.tensors();
    assert_eq!(tensors.len(), TENSORS.len());
    for (t, (name, shape)) in tensors.iter().zip(TENSORS) {
        assert_eq!((t.name.as_str(), t.shape.as_slice()), (name, shape));
    }
    // The exporter writes the bytes core writes.
    assert_eq!(w.to_bytes(), HL_1.blob);
}

#[test]
fn versions_by_name() {
    assert_eq!(version("hl-1").map(|v| v.name), Some("hl-1"));
    assert_eq!(version("hl-2").map(|v| v.name), Some("hl-2"));
    assert!(version("hl-3").is_none());
    assert!(version("").is_none());
    assert!(version("HL-1").is_none());
    let look = version("hl-1").unwrap().look;
    assert_eq!(look.depth_tie, [12.0, 48.0]);
    assert!(look.reach.is_none());
}

/// hl-2 is hl-1's weights and look with the reach added: nothing else
/// of hl-1's moves under it.
#[test]
fn hl_2_is_hl_1_with_its_reach() {
    assert_eq!(HL_2.blob, HL_1.blob);
    assert_eq!(HL_2.sha256, HL_1.sha256);
    assert_eq!(HL_2.weights(), HL_1.weights());
    let reach = HL_2.look.reach.expect("hl-2 has a reach");
    assert_eq!(
        FillLook {
            reach: None,
            ..HL_2.look
        },
        HL_1.look
    );
    assert!(reach.gate[0] < reach.gate[1] && reach.ramp.is_none() && reach.near == Some(96.0));
}

/// A rawio directory (SPEC.md, "Fixture format and layouts"): its f32
/// arrays by name, with their shapes.
fn read(dir: &Path, name: &str) -> (Vec<usize>, Vec<f32>) {
    let manifest = std::fs::read_to_string(dir.join("manifest.txt")).unwrap();
    let line = manifest
        .lines()
        .find(|l| l.split_whitespace().next() == Some(name))
        .unwrap_or_else(|| panic!("{name} in {}", dir.display()));
    let mut it = line.split_whitespace().skip(1);
    assert_eq!(it.next(), Some("f32"));
    let shape = it.map(|d| d.parse().unwrap()).collect();
    let data = std::fs::read(dir.join(format!("{name}.bin")))
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| f32::from_le_bytes(b))
        .collect();
    (shape, data)
}

fn max_abs(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// The built-in weights on lamp_f3's thumbnail, grid points, and
/// features, against the oracle run with the trained checkpoint on the
/// same inputs, written by
///
/// ```sh
/// CUDA_VISIBLE_DEVICES="" python -I -B scripts/highlight-fill-oracle/export_weights.py \
///     r6e-s1/best.pt crates/greycard-ai/src/highlight/hl-1.bin \
///     crates/greycard-core/tests/fixtures/highlight_fill/lamp_f3 \
///     crates/greycard-ai/src/highlight/hl-1-lamp_f3
/// ```
///
/// SPEC.md's tolerances with r6e-s1: 2e-5 for the map and the context,
/// 5e-5 for the outputs.
#[test]
fn hl_1_runs_as_the_oracle() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scene: PathBuf = root.join("../greycard-core/tests/fixtures/highlight_fill/lamp_f3");
    let expected = root.join("src/highlight/hl-1-lamp_f3");
    let w = HL_1.weights();
    let (ts, thumb) = read(&scene, "thumb");
    let map = w.encode(&thumb, ts[0], ts[1]);
    let (cs, cmap) = read(&expected, "cmap");
    assert_eq!(cs, [CONTEXT, map.height(), map.width()]);
    let e_cmap = max_abs(map.data(), &cmap);

    // The grid points from the stored centroids and extent.
    let (_, where_) = read(&scene, "where");
    let (_, centroid) = read(&scene, "centroid");
    let (_, grid) = read(&scene, "grid");
    let where_: [f32; 4] = where_.try_into().unwrap();
    let points: Vec<[f32; 2]> = centroid
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| grid_point(where_, c))
        .collect();
    assert_eq!(points.concat(), grid);

    let ctx: Vec<f32> = points.iter().flat_map(|&p| map.sample(p)).collect();
    let e_ctx = max_abs(&ctx, &read(&expected, "ctx").1);
    let (_, feats) = read(&scene, "feats");
    let outputs: Vec<f32> = feats
        .as_chunks::<FEATURES>()
        .0
        .iter()
        .zip(ctx.as_chunks::<CONTEXT>().0)
        .flat_map(|(f, c)| w.outputs(f, c))
        .collect();
    let e_out = max_abs(&outputs, &read(&expected, "outputs").1);
    assert_eq!(outputs.len(), 2 * OUTPUTS);
    eprintln!("hl-1 on lamp_f3: cmap {e_cmap:.2e}, ctx {e_ctx:.2e}, outputs {e_out:.2e}");
    assert!(e_cmap <= 2e-5, "cmap {e_cmap}");
    assert!(e_ctx <= 2e-5, "ctx {e_ctx}");
    assert!(e_out <= 5e-5, "outputs {e_out}");

    // The oracle's own map, sampled here, isolates the sampling.
    let stored = ContextMap::new(map.height(), map.width(), cmap).unwrap();
    let theirs: Vec<f32> = points.iter().flat_map(|&p| stored.sample(p)).collect();
    assert!(max_abs(&theirs, &read(&expected, "ctx").1) <= 2e-5);
}

/// A rawio directory's u8 array (the codes).
fn read_u8(dir: &Path, name: &str) -> Vec<u8> {
    let manifest = std::fs::read_to_string(dir.join("manifest.txt")).unwrap();
    assert!(
        manifest
            .lines()
            .any(|l| l.split_whitespace().take(2).eq([name, "u8"])),
        "{name} u8 in {}",
        dir.display()
    );
    std::fs::read(dir.join(format!("{name}.bin"))).unwrap()
}

/// The whole fill with the built-in weights and look, on two fixture
/// scenes' inputs (one at the look scale 2.5), against the oracle run
/// with the same weights, read from the blob, written by
///
/// ```sh
/// CUDA_VISIBLE_DEVICES="" python -I -B scripts/highlight-fill-oracle/run_blob.py \
///     crates/greycard-ai/src/highlight/hl-1.bin crates/greycard-ai/src/highlight/hl-1-fill \
///     crates/greycard-core/tests/fixtures/highlight_fill/lamp \
///     crates/greycard-core/tests/fixtures/highlight_fill/depth_tie_f2_5
/// ```
///
/// SPEC.md's tolerances: 1e-5 for gain_h, 5e-6 for dab_h, and 5e-6
/// relative to max(|v|, 1) for the picture; every pixel the oracle
/// leaves as x is x to the bit.
#[test]
fn hl_1_fills_as_the_oracle() {
    use greycard_core::develop::highlight_fill::fill_fields;
    use greycard_core::develop::highlight_fill::half::CodePlane;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for scene in ["lamp", "depth_tie_f2_5"] {
        let dir = root
            .join("../greycard-core/tests/fixtures/highlight_fill")
            .join(scene);
        let expected = root.join("src/highlight/hl-1-fill").join(scene);
        let (shape, x) = read(&dir, "x");
        let (h, w) = (shape[0], shape[1]);
        let codes = read_u8(&dir, "codes");
        let clips: [f32; 3] = read(&dir, "clips").1.try_into().unwrap();
        let f = read(&dir, "f").1[0] as f64;
        let plane = CodePlane {
            codes: &codes,
            width: w,
        };
        let fields = fill_fields(&x, w, h, &plane, clips, HL_1.weights(), &HL_1.look, f).unwrap();
        let (hh, wh) = (h.div_ceil(2), w.div_ceil(2));
        let cells: Vec<[f32; 3]> = (0..hh * wh).map(|i| fields.cell(i % wh, i / wh)).collect();
        let gain: Vec<f32> = cells.iter().map(|c| c[0]).collect();
        let dab: Vec<f32> = [1, 2]
            .iter()
            .flat_map(|&k| cells.iter().map(move |c| c[k]))
            .collect();
        let e_gain = max_abs(&gain, &read(&expected, "gain_h").1);
        let e_dab = max_abs(&dab, &read(&expected, "dab_h").1);
        let mut out = x.clone();
        fields.apply(&mut out, &plane);
        let (_, want) = read(&expected, "out_clipped");
        let mut e_pic = 0.0f32;
        let mut k = 0;
        for p in 0..w * h {
            let (o, xp) = (&out[3 * p..3 * p + 3], &x[3 * p..3 * p + 3]);
            if codes[p] & 0x7F == 0 {
                assert!(o.iter().zip(xp).all(|(a, b)| a.to_bits() == b.to_bits()));
                continue;
            }
            let wp = &want[3 * k..3 * k + 3];
            k += 1;
            if wp.iter().zip(xp).all(|(a, b)| a.to_bits() == b.to_bits()) {
                assert!(
                    o.iter().zip(xp).all(|(a, b)| a.to_bits() == b.to_bits()),
                    "{scene}: pixel {p}"
                );
            } else {
                for (&g, &w) in o.iter().zip(wp) {
                    e_pic = e_pic.max((g - w).abs() / w.abs().max(1.0));
                }
            }
        }
        assert_eq!(3 * k, want.len());
        eprintln!(
            "hl-1 on {scene}: gain_h {e_gain:.2e}, dab_h {e_dab:.2e}, picture {e_pic:.2e} relative"
        );
        assert!(e_gain <= 1e-5, "{scene}: gain_h {e_gain}");
        assert!(e_dab <= 5e-6, "{scene}: dab_h {e_dab}");
        assert!(e_pic <= 5e-6, "{scene}: picture {e_pic}");
    }
}
