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
    assert!(version("hl-2").is_none());
    assert!(version("").is_none());
    assert!(version("HL-1").is_none());
    let look = version("hl-1").unwrap().look;
    assert_eq!(look.depth_tie, [12.0, 48.0]);
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
