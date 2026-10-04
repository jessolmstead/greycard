//! Fit a look from a pairs file and write it as a `.cube`: the way to
//! turn the trial's block pairs (`tools/camera-match/fit.py`, written
//! in the oracle fixture's CSV layout) into a table through this
//! crate's lattice solve rather than the script's thin-plate fit.
//!
//!     cargo run --release -p greycard-match --example refit -- PAIRS.csv "TITLE" OUT.cube [KEY=VALUE...]
//!
//! Each `KEY=VALUE` after the output is declared in the table's header
//! as `# KEY: VALUE`. A table that declares no `display_curve` reads in
//! the editor as fitted under per channel, which the trial's pairs
//! were; pairs from a develop under AgX want `display_curve=agx`.

use std::path::Path;

use greycard_match::Model;
use greycard_match::cube::write_cube;
use greycard_match::fit::LutParams;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: refit PAIRS.csv TITLE OUT.cube [KEY=VALUE...]");
        std::process::exit(2);
    }
    let declared: Vec<(&str, &str)> = args[4..]
        .iter()
        .map(|a| a.split_once('=').expect("a declaration is KEY=VALUE"))
        .collect();
    let text = std::fs::read_to_string(&args[1]).expect("read the pairs");
    let mut x = Vec::new();
    let mut y = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let v: Vec<f32> = line
            .split(',')
            .map(|s| s.trim().parse().expect("a number"))
            .collect();
        x.push([v[1], v[2], v[3]]);
        y.push([v[4], v[5], v[6]]);
    }
    let model = Model::fit(&x, &y, LutParams::default());
    let de = model.mean_delta_e(&x, &y);
    write_cube(&model, &args[2], &declared, Path::new(&args[3])).expect("write the cube");
    println!(
        "{} pairs, fitted mean ΔE {:.4}, wrote {}",
        x.len(),
        de,
        args[3]
    );
}
