//! The oracle: the Python fit's numbers on the R6 II Faithful set
//! (docs/camera-match.md), which this crate is held to. The fixture is
//! every 8th kept block pair of the 32 frames, in encoded sRGB, with
//! the matrix, the curves and the held-out ΔE per frame the script
//! produced on the full set.

use std::path::PathBuf;

use greycard_match::Model;
use greycard_match::fit::{CURVE_KNOTS, LutParams, leave_one_out};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fit {
    frames: Vec<String>,
    matrix: [[f32; 3]; 3],
    curve: Option<Vec<Vec<f32>>>,
    held_out_lut: std::collections::HashMap<String, f32>,
    fitted_per_frame: std::collections::HashMap<String, f32>,
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The pairs' render side, camera side, frame index per pair, and
/// the Python's results.
type Fixture = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<usize>, Fit);

fn load() -> Fixture {
    let text = std::fs::read_to_string(fixtures().join("r6ii-faithful-pairs.csv")).unwrap();
    let (mut x, mut y, mut ids) = (vec![], vec![], vec![]);
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let v: Vec<f32> = line.split(',').map(|s| s.trim().parse().unwrap()).collect();
        ids.push(v[0] as usize);
        x.push([v[1], v[2], v[3]]);
        y.push([v[4], v[5], v[6]]);
    }
    let fit: Fit = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("r6ii-faithful-fit.json")).unwrap(),
    )
    .unwrap();
    (x, y, ids, fit)
}

/// The Python's mean fitted ΔE over all pairs of the full set.
const PYTHON_FITTED: f32 = 0.0136;
/// Tight enough to tell the table from none: with the table
/// effectively off the held-out mean sits 0.0019 over the Python's.
const TOLERANCE: f32 = 0.0012;

/// Every fourth frame held out, so a debug build finishes in
/// seconds; the ignored sweep below does all 32.
fn chosen_frames(n: usize) -> Vec<usize> {
    (0..n).step_by(4).collect()
}

fn held_out_mean(
    x: &[[f32; 3]],
    y: &[[f32; 3]],
    ids: &[usize],
    chosen: &[usize],
    params: LutParams,
) -> (Vec<f32>, f32) {
    let held = leave_one_out(x, y, ids, chosen, params);
    let mean = held.iter().sum::<f32>() / held.len() as f32;
    (held, mean)
}

#[test]
fn the_fit_reproduces_the_python_on_the_r6ii_faithful_set() {
    let (x, y, ids, py) = load();
    assert_eq!(x.len(), 12025);
    let m = Model::fit(&x, &y, LutParams::default());

    for i in 0..3 {
        for j in 0..3 {
            assert!(
                (m.matrix[i][j] - py.matrix[i][j]).abs() < 0.03,
                "matrix {:?} against the Python's {:?}",
                m.matrix,
                py.matrix
            );
        }
    }
    assert_eq!(
        m.curves.is_some(),
        py.curve.is_some(),
        "the curves decision"
    );
    // The curve values, knot by knot, in linear light, except the
    // last: ours is pinned to one where the Python's is the top bin's
    // median carried up (0.89), since no pair passes the clipping
    // cut there.
    let ours = m.curves.as_ref().unwrap();
    let theirs = py.curve.as_ref().unwrap();
    let mut worst = 0.0f32;
    for (c, (our, their)) in ours.y.iter().zip(theirs).enumerate() {
        for (k, (o, t)) in our.iter().zip(their).take(CURVE_KNOTS - 1).enumerate() {
            let d = (o - t).abs();
            worst = worst.max(d);
            assert!(d < 0.02, "curve {c} knot {k}: {o} against the Python's {t}");
        }
        assert_eq!(our[CURVE_KNOTS - 1], 1.0);
    }
    eprintln!("curves: worst knot difference {worst:.4} (linear)");
    let white = m.apply([1.0; 3]);
    assert!(
        white.iter().all(|v| (v - 1.0).abs() < 1e-6),
        "white {white:?}"
    );

    let fitted = m.mean_delta_e(&x, &y);
    assert!(
        (fitted - PYTHON_FITTED).abs() < TOLERANCE,
        "fitted mean ΔE {fitted:.4} against the Python's {PYTHON_FITTED:.4}"
    );

    let chosen = chosen_frames(py.frames.len());
    let (held, ours) = held_out_mean(&x, &y, &ids, &chosen, LutParams::default());
    let theirs = chosen
        .iter()
        .map(|&i| py.held_out_lut[&py.frames[i]])
        .sum::<f32>()
        / chosen.len() as f32;
    eprintln!(
        "fitted {fitted:.4} (python {PYTHON_FITTED:.4}); held out over {} frames {ours:.4} (python {theirs:.4})",
        chosen.len()
    );
    for (k, &i) in chosen.iter().enumerate() {
        let f = &py.frames[i];
        eprintln!(
            "  {f}: held out {:.4} (python {:.4}), fitted-all python {:.4}",
            held[k], py.held_out_lut[f], py.fitted_per_frame[f]
        );
    }
    assert!(
        (ours - theirs).abs() < TOLERANCE,
        "held-out mean ΔE {ours:.4} against the Python's {theirs:.4}"
    );

    // And the table earns its place: held out, it beats the matrix
    // and curves alone, which a pull large enough to switch it off
    // reproduces.
    let off = LutParams {
        pull: 1e6,
        ..LutParams::default()
    };
    let (_, without) = held_out_mean(&x, &y, &ids, &chosen, off);
    eprintln!("held out without the table {without:.4}");
    // On these eight frames the table's gain is small (0.0007); over
    // all 32 it is 0.0027, which the sweep shows.
    assert!(
        ours < without - 0.0003,
        "the table held out at {ours:.4} against {without:.4} without it"
    );
}

/// A sweep over the table's regularization, for choosing the
/// defaults; prints and never fails.
#[test]
#[ignore]
fn sweep_the_lattice_regularization() {
    let (x, y, ids, py) = load();
    let theirs = py.frames.iter().map(|f| py.held_out_lut[f]).sum::<f32>() / py.frames.len() as f32;
    // The defaults are in the grid, so the 32-frame number the crate
    // claims reproduces from the tree.
    let d = LutParams::default();
    assert!([0.1, 0.3, 0.5, 1.0, 3.0].contains(&d.smoothness));
    assert!([0.005, 0.02, 0.1].contains(&d.pull));
    for smoothness in [0.1, 0.3, 0.5, 1.0, 3.0] {
        for pull in [0.005, 0.02, 0.1] {
            let params = LutParams {
                smoothness,
                pull,
                iterations: 300,
            };
            let m = Model::fit(&x, &y, params);
            let fitted = m.mean_delta_e(&x, &y);
            let all: Vec<usize> = (0..py.frames.len()).collect();
            let held = leave_one_out(&x, &y, &ids, &all, params);
            let ours = held.iter().sum::<f32>() / held.len() as f32;
            eprintln!(
                "smoothness {smoothness} pull {pull}: fitted {fitted:.4} (py {PYTHON_FITTED:.4}) held {ours:.4} (py {theirs:.4})"
            );
        }
    }
}
