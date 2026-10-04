//! The model written as a `.cube` the engine's look slot reads: a
//! 33³ table in encoded sRGB, red fastest, with the encoding and
//! primaries declared in the comment lines `greycard-core/src/lut.rs`
//! honors, the line [`FITTED`] that says the match wrote it, and
//! whatever else the caller declares about the fit as `# key: value`
//! lines under it.
//!
//! What those other lines say (the display transform the frames were
//! developed under, the body they came from) is the caller's: this
//! crate fits numbers and knows nothing of an edit, so it writes the
//! keys and values it is handed as they are.

use std::fmt::Write as _;
use std::path::Path;

use crate::LUT_SIZE;
use crate::fit::Model;

/// The comment line every table the match writes carries, as a
/// reader of the file's comments sees it (the text after the `#`,
/// trimmed). It has been on every table since the first, so it is
/// what tells a fitted table from any other.
pub const FITTED: &str = "fitted from the camera's embedded JPEG by greycard-match";

/// The `.cube` text for the model sampled on the lattice, with
/// `declared` written as `# key: value` lines in the header. A line
/// break in a key or a value would end the comment early, so it is
/// written as a space.
pub fn cube_text(model: &Model, title: &str, declared: &[(&str, &str)]) -> String {
    let n = LUT_SIZE;
    let mut s = String::with_capacity(n * n * n * 30);
    writeln!(s, "TITLE \"{}\"", title.replace('"', "'")).unwrap();
    s.push_str("# encoding: srgb\n# primaries: srgb\n");
    writeln!(s, "# {FITTED}").unwrap();
    let one_line = |t: &str| t.replace(['\n', '\r'], " ").trim().to_string();
    for (key, value) in declared {
        writeln!(s, "# {}: {}", one_line(key), one_line(value)).unwrap();
    }
    writeln!(s, "LUT_3D_SIZE {n}").unwrap();
    s.push_str("DOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n");
    let last = (n - 1) as f32;
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let v = model.apply([r as f32 / last, g as f32 / last, b as f32 / last]);
                writeln!(s, "{:.6} {:.6} {:.6}", v[0], v[1], v[2]).unwrap();
            }
        }
    }
    s
}

/// Write the model to `path` as a `.cube`.
pub fn write_cube(
    model: &Model,
    title: &str,
    declared: &[(&str, &str)],
    path: &Path,
) -> std::io::Result<()> {
    std::fs::write(path, cube_text(model, title, declared))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::decode3;
    use crate::fit::LutParams;
    use greycard_core::lut::{Encoding, Lut3d, declared};

    fn model() -> Model {
        let x: Vec<[f32; 3]> = (0..3000)
            .map(|i| {
                let t = i as f32 * 0.618_034;
                let u = i as f32 * 0.414_214;
                let v = i as f32 * 0.267_949;
                [
                    0.05 + 0.9 * (t - t.floor()),
                    0.05 + 0.9 * (u - u.floor()),
                    0.05 + 0.9 * (v - v.floor()),
                ]
            })
            .collect();
        let y: Vec<[f32; 3]> = x
            .iter()
            .map(|v| decode3(*v))
            .map(|v| [v[0] * 1.1, v[1], v[2] * 0.9 + 0.02 * v[0]])
            .map(crate::color::encode3)
            .collect();
        Model::fit(&x, &y, LutParams::default())
    }

    #[test]
    fn the_engine_reads_the_table_back_and_agrees_at_every_node() {
        let m = model();
        let dir = std::env::temp_dir().join(format!("greycard-match-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.cube");
        write_cube(&m, "test look", &[("display_curve", "agx")], &path).unwrap();
        let lut = Lut3d::load(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(lut.size, LUT_SIZE);
        assert_eq!(lut.title.as_deref(), Some("test look"));
        assert_eq!(lut.encoding, Encoding::Srgb);
        assert!(lut.comments.iter().any(|c| c == FITTED));
        assert_eq!(declared(&lut.comments, "display_curve"), Some("agx"));
        let last = (LUT_SIZE - 1) as f32;
        for (r, g, b) in [
            (0, 0, 0),
            (16, 16, 16),
            (32, 32, 32),
            (5, 20, 30),
            (31, 2, 17),
        ] {
            let x = [r as f32 / last, g as f32 / last, b as f32 / last];
            let ours = m.apply(x);
            let theirs = lut.sample(x);
            for k in 0..3 {
                assert!(
                    (ours[k] - theirs[k]).abs() < 2e-6,
                    "{x:?}: {ours:?} vs {theirs:?}"
                );
            }
        }
        // Between nodes the engine interpolates tetrahedrally and the
        // model trilinearly on the residual; they agree closely but
        // not exactly, and a grey stays grey under both.
        let x = [0.3, 0.3, 0.3];
        let t = lut.sample(x);
        let o = m.apply(x);
        for k in 0..3 {
            assert!((t[k] - o[k]).abs() < 5e-3, "{t:?} vs {o:?}");
        }
    }

    #[test]
    fn the_text_declares_what_the_reader_needs() {
        let s = cube_text(
            &Model::identity(),
            "id",
            &[("display_curve", "channels"), ("model", "EOS\nR6m2")],
        );
        assert!(s.starts_with(
            "TITLE \"id\"\n# encoding: srgb\n# primaries: srgb\n\
             # fitted from the camera's embedded JPEG by greycard-match\n\
             # display_curve: channels\n# model: EOS R6m2\nLUT_3D_SIZE 33\n"
        ));
        assert!(s.contains("LUT_3D_SIZE 33\n"));
        assert_eq!(
            s.lines()
                .filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_digit()))
                .count(),
            33 * 33 * 33
        );
        // Red runs fastest: the second row is one red step from black.
        let rows: Vec<&str> = s
            .lines()
            .filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .collect();
        assert_eq!(rows[0], "0.000000 0.000000 0.000000");
        assert_eq!(rows[1], "0.031250 0.000000 0.000000");
    }
}
