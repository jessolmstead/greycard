//! The network against the oracle's fixtures
//! (`tests/fixtures/highlight_fill/`, SPEC.md's tolerances), and the
//! weights blob's round trip and refusals.

use super::*;
use crate::develop::highlight_fill::fixtures::{Fixture, random_tensors, random_weights};

/// SPEC.md's tolerances, random weights.
const TOL_CMAP: f32 = 2e-5;
const TOL_CTX: f32 = 2e-5;
const TOL_OUTPUTS: f32 = 5e-6;

fn max_abs(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// SPEC stage 7, the thumbnail, for the scenes that store none: f64
/// sums of max(x, 0) in raster order a block, non-finite read as 0.
fn thumbnail(x: &[f32], codes: &[u8], h: usize, w: usize) -> (Vec<f32>, usize, usize) {
    let f = h.max(w).div_ceil(384).max(1);
    let (th, tw) = (h.div_ceil(f), w.div_ceil(f));
    let mut out = vec![0.0f32; th * tw * 5];
    for ty in 0..th {
        for tx in 0..tw {
            let mut acc = [0.0f64; 3];
            let (mut any, mut full, mut n) = (0usize, 0usize, 0usize);
            for y in ty * f..((ty + 1) * f).min(h) {
                for x_ in tx * f..((tx + 1) * f).min(w) {
                    let p = y * w + x_;
                    for c in 0..3 {
                        let v = x[p * 3 + c];
                        let v = if v.is_finite() { v } else { 0.0 };
                        acc[c] += f64::from(v).max(0.0);
                    }
                    let k = codes[p] & 0x7F;
                    any += usize::from(k > 0);
                    full += usize::from(k == 3);
                    n += 1;
                }
            }
            let o = &mut out[(ty * tw + tx) * 5..][..5];
            for c in 0..3 {
                o[c] = ((acc[c] / n as f64) as f32).max(1.0 / 4096.0).log2();
            }
            o[3] = (any as f64 / n as f64) as f32;
            o[4] = (full as f64 / n as f64) as f32;
        }
    }
    (out, th, tw)
}

/// SPEC stage 9's centroids: per row (ids ascending), the f64 raster
/// sums of the f32 (x + 0.5) / Wh and (y + 0.5) / Hh over the region's
/// cells, over max(count, 1), rounded to f32 once. The outside row has
/// no inside cells: (0, 0).
fn centroids(regions: &[i32], hh: usize, wh: usize, ids: &[i32]) -> Vec<[f32; 2]> {
    let mut sum = vec![[0.0f64; 2]; ids.len()];
    let mut count = vec![0u64; ids.len()];
    for y in 0..hh {
        for x in 0..wh {
            let id = regions[y * wh + x];
            if id == 0 {
                continue;
            }
            let row = ids.binary_search(&id).unwrap();
            sum[row][0] += f64::from((x as f32 + 0.5) / wh as f32);
            sum[row][1] += f64::from((y as f32 + 0.5) / hh as f32);
            count[row] += 1;
        }
    }
    sum.iter()
        .zip(&count)
        .map(|(s, &c)| {
            let c = c.max(1) as f64;
            [(s[0] / c) as f32, (s[1] / c) as f32]
        })
        .collect()
}

fn rows(a: &Fixture, name: &str, k: usize) -> Vec<Vec<f32>> {
    a.f32(name).chunks_exact(k).map(<[f32]>::to_vec).collect()
}

#[test]
fn random_weights_load_with_every_parameter() {
    let w = random_weights();
    assert_eq!(w.parameters(), PARAMETERS);
    let shapes: usize = TENSORS
        .iter()
        .map(|(_, s)| s.iter().product::<usize>())
        .sum();
    assert_eq!(shapes, PARAMETERS);
}

#[test]
fn encoder_sampling_and_mlp_match_lamp_f3() {
    let w = random_weights();
    let a = Fixture::load("lamp_f3");
    let ts = a.dims("thumb");
    let map = w.encode(&a.f32("thumb"), ts[0], ts[1]);
    assert_eq!(a.dims("cmap"), [CONTEXT, map.height(), map.width()]);
    let e_cmap = max_abs(map.data(), &a.f32("cmap"));

    // `where` and the grid from the stored centroids, to the bit.
    let frame = a.i32("frame");
    let regions = a.dims("regions");
    let where_ = frame_where(
        [0, 0],
        [frame[0] as u32, frame[1] as u32],
        [regions[1], regions[0]],
    );
    assert_eq!(where_.to_vec(), a.f32("where"));
    let grid = rows(&a, "grid", 2);
    for (c, g) in rows(&a, "centroid", 2).iter().zip(&grid) {
        assert_eq!(grid_point(where_, [c[0], c[1]]).to_vec(), *g);
    }

    // Sampling the oracle's own map isolates grid_sample.
    let stored = ContextMap::new(map.height(), map.width(), a.f32("cmap")).unwrap();
    let ctx = a.f32("ctx");
    let ours: Vec<f32> = grid.iter().flat_map(|g| map.sample([g[0], g[1]])).collect();
    let theirs: Vec<f32> = grid
        .iter()
        .flat_map(|g| stored.sample([g[0], g[1]]))
        .collect();
    let e_sample = max_abs(&theirs, &ctx);
    let e_ctx = max_abs(&ours, &ctx);

    let feats = rows(&a, "feats", FEATURES);
    let ours_out: Vec<f32> = feats
        .iter()
        .zip(ours.as_chunks::<CONTEXT>().0)
        .flat_map(|(f, c)| w.outputs(f.as_slice().try_into().unwrap(), c))
        .collect();
    let e_out = max_abs(&ours_out, &a.f32("outputs"));
    eprintln!(
        "lamp_f3: cmap {e_cmap:.2e}, sample of the stored map {e_sample:.2e}, ctx {e_ctx:.2e}, outputs {e_out:.2e}"
    );
    assert!(e_cmap <= TOL_CMAP, "cmap {e_cmap}");
    assert!(e_sample <= TOL_CTX, "sample {e_sample}");
    assert!(e_ctx <= TOL_CTX, "ctx {e_ctx}");
    assert!(e_out <= TOL_OUTPUTS, "outputs {e_out}");

    // The rows' pass zeroes the outside row and gives the others as is.
    let ids = a.i32("ids");
    let table: Vec<RegionRow> = feats
        .iter()
        .zip(&grid)
        .zip(&ids)
        .map(|((f, g), &id)| RegionRow {
            outside: id == 0,
            features: f.as_slice().try_into().unwrap(),
            point: [g[0], g[1]],
        })
        .collect();
    assert_eq!(ids[0], 0);
    let out = w.region_outputs(&map, &table);
    assert_eq!(out[0], [0.0; OUTPUTS]);
    for (r, o) in out.iter().enumerate().skip(1) {
        assert_eq!(o.as_slice(), &ours_out[r * OUTPUTS..(r + 1) * OUTPUTS]);
    }
}

/// The MLP on every scene's stored features and context.
#[test]
fn mlp_matches_every_scene() {
    let w = random_weights();
    let mut checked = 0;
    for scene in Fixture::scenes() {
        let a = Fixture::load(&scene);
        if !a.has("feats") {
            continue;
        }
        let ours: Vec<f32> = rows(&a, "feats", FEATURES)
            .iter()
            .zip(rows(&a, "ctx", CONTEXT))
            .flat_map(|(f, c)| {
                w.outputs(
                    f.as_slice().try_into().unwrap(),
                    c.as_slice().try_into().unwrap(),
                )
            })
            .collect();
        let e = max_abs(&ours, &a.f32("outputs"));
        eprintln!(
            "{scene}: outputs {e:.2e} over {} rows",
            ours.len() / OUTPUTS
        );
        assert!(e <= TOL_OUTPUTS, "{scene}: outputs {e}");
        checked += 1;
    }
    assert_eq!(checked, 16);
}

/// The encoder and the sampling on every scene whose regions the
/// fixtures give (or imply), from a thumbnail built here.
#[test]
fn context_matches_every_scene_with_regions() {
    let w = random_weights();
    let lamp = Fixture::load("lamp");
    let mut checked = 0;
    for scene in Fixture::scenes() {
        let a = Fixture::load(&scene);
        if !a.has("ctx") {
            continue;
        }
        // headroom's x is the lamp's over its scalar, its codes and so
        // its regions the lamp's.
        let (x, codes, src) = if scene == "headroom" {
            let k = a.f32("headroom")[0];
            let x: Vec<f32> = lamp.f32("x").iter().map(|v| v / k).collect();
            (x, lamp.u8("codes"), &lamp)
        } else {
            (a.f32("x"), a.u8("codes"), &a)
        };
        let xs = src.dims("x");
        let (h, wd) = (xs[0], xs[1]);
        let (hh, wh) = (h.div_ceil(2), wd.div_ceil(2));
        let ids = a.i32("ids");
        let regions = if src.has("regions") {
            src.i32("regions")
        } else if ids.len() == 1 && ids[0] == (hh * wh) as i32 {
            // all_clipped: one region over every cell, its id the
            // largest flat index plus one.
            vec![ids[0]; hh * wh]
        } else {
            eprintln!("{scene}: no regions stored, context not checked");
            continue;
        };
        let (thumb, th, tw) = thumbnail(&x, &codes, h, wd);
        let map = w.encode(&thumb, th, tw);
        let frame = a.i32("frame");
        let where_ = frame_where([0, 0], [frame[0] as u32, frame[1] as u32], [wh, hh]);
        let ours: Vec<f32> = centroids(&regions, hh, wh, &ids)
            .iter()
            .flat_map(|&c| map.sample(grid_point(where_, c)))
            .collect();
        let e = max_abs(&ours, &a.f32("ctx"));
        eprintln!("{scene}: ctx {e:.2e} over {} rows", ids.len());
        assert!(e <= TOL_CTX, "{scene}: ctx {e}");
        checked += 1;
    }
    assert_eq!(checked, 14);
}

#[test]
fn bytes_round_trip() {
    let w = random_weights();
    let bytes = w.to_bytes();
    assert_eq!(
        bytes.len(),
        16 + 4 * PARAMETERS
            + TENSORS
                .iter()
                .map(|(n, s)| 2 + n.len() + 1 + 4 * s.len())
                .sum::<usize>()
    );
    let back = FillWeights::from_bytes(&bytes).unwrap();
    assert_eq!(back, w);
    assert_eq!(back.to_bytes(), bytes);
    assert_eq!(FillWeights::from_tensors(w.tensors()).unwrap(), w);
}

/// Every prefix of a blob is refused, none panics.
#[test]
fn truncated_blobs_are_refused() {
    let bytes = random_weights().to_bytes();
    let cuts = (0..600).chain((600..bytes.len()).step_by(997));
    for n in cuts {
        let e = FillWeights::from_bytes(&bytes[..n]).unwrap_err();
        assert!(
            matches!(e, WeightsError::Truncated | WeightsError::Missing(_)),
            "{n}: {e:?}"
        );
    }
    let mut long = bytes.clone();
    long.push(0);
    assert_eq!(
        FillWeights::from_bytes(&long),
        Err(WeightsError::Trailing(1))
    );
}

#[test]
fn malformed_blobs_are_refused() {
    let bytes = random_weights().to_bytes();
    let with = |at: usize, new: &[u8]| {
        let mut b = bytes.clone();
        b[at..at + new.len()].copy_from_slice(new);
        FillWeights::from_bytes(&b)
    };
    assert_eq!(with(0, b"GCHLFILX"), Err(WeightsError::Magic));
    assert_eq!(with(8, &2u32.to_le_bytes()), Err(WeightsError::Version(2)));
    assert_eq!(
        with(12, &u32::MAX.to_le_bytes()),
        Err(WeightsError::Count(u32::MAX))
    );
    assert_eq!(
        with(12, &15u32.to_le_bytes()),
        Err(WeightsError::Trailing(4 * 12 + 2 + 10 + 1 + 4))
    );
    // The first tensor: its name at 18, its rank at 34, its dims at 35.
    assert_eq!(
        with(18, b"ctx.net.1"),
        Err(WeightsError::Unknown("ctx.net.1.weight".into()))
    );
    assert_eq!(with(18, &[0xFF]), Err(WeightsError::Name));
    assert!(matches!(
        with(35, &17u32.to_le_bytes()),
        Err(WeightsError::Shape { .. })
    ));
    // A huge dimension is a shape mismatch before anything is read.
    assert!(matches!(
        with(35, &u32::MAX.to_le_bytes()),
        Err(WeightsError::Shape { .. })
    ));
    assert!(matches!(with(34, &[255]), Err(WeightsError::Shape { .. })));
    // A tensor named as another, of its shape or not: out of order.
    let at = |name: &[u8]| bytes.windows(name.len()).position(|w| w == name).unwrap();
    assert_eq!(
        with(at(b"ctx.net.6.weight"), b"ctx.net.4.weight"),
        Err(WeightsError::Order {
            expected: "ctx.net.6.weight",
            found: "ctx.net.4.weight".into(),
        })
    );
    assert_eq!(
        with(at(b"ctx.net.2.bias"), b"ctx.net.0.bias"),
        Err(WeightsError::Order {
            expected: "ctx.net.2.bias",
            found: "ctx.net.0.bias".into(),
        })
    );
    // A NaN weight is refused: it would reach every region.
    assert_eq!(
        with(16 + 2 + 16 + 1 + 16, &f32::NAN.to_le_bytes()),
        Err(WeightsError::NotFinite("ctx.net.0.weight".into()))
    );
    assert_eq!(
        with(16 + 2 + 16 + 1 + 16, &f32::INFINITY.to_le_bytes()),
        Err(WeightsError::NotFinite("ctx.net.0.weight".into()))
    );
}

/// Two tensors of one shape swapped: a valid set, refused for its order,
/// so every accepted blob writes back as its own bytes.
#[test]
fn blobs_out_of_order_are_refused() {
    let w = random_weights();
    let bytes = w.to_bytes();
    let at = |name: &[u8]| bytes.windows(name.len()).position(|x| x == name).unwrap();
    let (a, b) = (at(b"ctx.net.4.weight"), at(b"ctx.net.6.weight"));
    let len = b - a;
    let mut swapped = bytes.clone();
    swapped[a - 2..b - 2].copy_from_slice(&bytes[b - 2..b - 2 + len]);
    swapped[b - 2..b - 2 + len].copy_from_slice(&bytes[a - 2..b - 2]);
    assert_ne!(swapped, bytes);
    assert_eq!(
        FillWeights::from_bytes(&swapped),
        Err(WeightsError::Order {
            expected: "ctx.net.4.weight",
            found: "ctx.net.6.weight".into(),
        })
    );
    // The same set as tensors, in any order, is accepted: each goes by
    // its name.
    let mut t = w.tensors();
    t.reverse();
    assert_eq!(FillWeights::from_tensors(t), Ok(w));
}

#[test]
fn context_maps_are_checked() {
    assert!(ContextMap::new(0, 3, Vec::new()).is_none());
    assert!(ContextMap::new(2, 0, Vec::new()).is_none());
    assert!(ContextMap::new(2, 3, vec![0.0; CONTEXT * 6 - 1]).is_none());
    assert!(ContextMap::new(2, 3, vec![0.0; CONTEXT * 6 + 1]).is_none());
    assert!(ContextMap::new(usize::MAX, 2, Vec::new()).is_none());
    let map = ContextMap::new(2, 3, vec![1.0; CONTEXT * 6]).unwrap();
    assert_eq!(
        (map.height(), map.width(), map.data().len()),
        (2, 3, CONTEXT * 6)
    );
}

#[test]
fn tensor_sets_are_checked() {
    let mut t = random_tensors();
    t.pop();
    assert_eq!(
        FillWeights::from_tensors(t.clone()),
        Err(WeightsError::Missing("mlp.4.bias"))
    );
    let mut dup = t.clone();
    dup.push(t[0].clone());
    assert_eq!(
        FillWeights::from_tensors(dup),
        Err(WeightsError::Duplicate("ctx.net.0.weight".into()))
    );
    let mut extra = random_tensors();
    extra.push(Tensor {
        name: "ctx.pool".into(),
        shape: vec![1],
        data: vec![0.0],
    });
    assert_eq!(
        FillWeights::from_tensors(extra),
        Err(WeightsError::Unknown("ctx.pool".into()))
    );
    let mut short = random_tensors();
    short[1].data.pop();
    assert_eq!(
        FillWeights::from_tensors(short),
        Err(WeightsError::Length {
            name: "ctx.net.0.bias".into(),
            expected: 16,
            found: 15,
        })
    );
    let mut flat = random_tensors();
    flat[0].shape = vec![720];
    assert!(matches!(
        FillWeights::from_tensors(flat),
        Err(WeightsError::Shape { .. })
    ));
}

/// Hostile bytes, a deterministic sweep: never a panic.
#[test]
fn mutated_blobs_never_panic() {
    let bytes = random_weights().to_bytes();
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..2000 {
        let mut b = bytes[..(next() as usize % 400).max(1)].to_vec();
        for _ in 0..4 {
            let at = next() as usize % b.len();
            b[at] = next() as u8;
        }
        let _ = FillWeights::from_bytes(&b);
    }
}

#[test]
fn sampling_clamps_at_the_border() {
    let map = ContextMap::new(2, 3, (0..CONTEXT * 6).map(|v| v as f32).collect()).unwrap();
    // Past every edge: the corner cells.
    assert_eq!(map.sample([-5.0, -5.0])[0], 0.0);
    assert_eq!(map.sample([5.0, 5.0])[0], 5.0);
    assert_eq!(map.sample([5.0, 5.0])[1], 11.0);
    // The centre of the middle column, between the rows.
    assert_eq!(map.sample([0.0, 0.0])[0], 2.5);
    assert_eq!(map.sample([f32::NAN, f32::NAN])[0], 0.0);
}

#[test]
fn gelu_fixed_points() {
    assert_eq!(gelu(0.0), 0.0);
    assert_eq!(gelu(-20.0), -0.0);
    assert_eq!(gelu(20.0), 20.0);
    assert!((gelu(1.0) - 0.841_344_7).abs() < 1e-7);
}
