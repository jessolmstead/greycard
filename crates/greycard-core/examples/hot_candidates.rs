//! The hot pixel repair's candidates on real frames, so the counts in
//! the notes can be taken again from the tree.
//!
//! Prints one CSV line a photosite the same-color tests flag, at the
//! sigma and ratio given, with how its other colors read: whether the
//! repair at a given others factor takes it is a filter over these
//! lines. Coordinates are the crop's, as the develop sees it.
//!
//! ```text
//! cargo run --release -p greycard-core --example hot_candidates -- 6 2 FILE...
//! ```

use greycard_core::decode::decode_path;
use greycard_core::develop::hotpixels::{OTHERS_SIGMAS, candidates};
use greycard_core::develop::noise::estimate_noise;
use greycard_core::develop::{crop_samples, normalize_levels};
use greycard_core::raw::SensorLayout;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [sigmas, ratio, files @ ..] = &args[..] else {
        panic!("want: SIGMAS RATIO FILE...");
    };
    let sigmas: f32 = sigmas.parse().expect("sigmas");
    let ratio: f32 = ratio.parse().expect("ratio");
    println!(
        "file,x,y,color,hot,value,replacement,ratio,\
         h_near,h_bg,h_sigma,v_near,v_bg,v_sigma,d_near,d_bg,d_sigma,follow"
    );
    for file in files {
        let frame = match decode_path(file) {
            Ok(frame) => frame,
            Err(e) => {
                eprintln!("{file}: {e}");
                continue;
            }
        };
        let SensorLayout::Cfa(pattern) = &frame.layout else {
            eprintln!("{file}: not a CFA frame");
            continue;
        };
        let normalized = normalize_levels(&frame);
        let crop = frame.effective_crop();
        let (samples, w, h) = crop_samples(&normalized, frame.width, frame.channels, crop);
        let pattern = pattern.shifted(crop.x, crop.y);
        let noise = estimate_noise(&samples, w, h, &pattern).expect("noise");
        let found =
            candidates(&samples, w, h, &pattern, &noise.model, sigmas, ratio).expect("candidates");
        eprintln!("{file}: {w}x{h}, {} candidates", found.len());
        let name = std::path::Path::new(file)
            .file_name()
            .map_or(file.clone(), |n| n.to_string_lossy().into_owned());
        for c in &found {
            let (y, x) = (c.index / w, c.index % w);
            let color = pattern.color_at(y, x).rgb_index().unwrap_or(9);
            let r = if c.hot {
                c.value / c.replacement.max(1e-9)
            } else {
                c.replacement / c.value.max(1e-9)
            };
            let o: Vec<String> = c
                .others
                .iter()
                .map(|n| format!("{:.6},{:.6},{:.6}", n.near, n.background, n.sigma))
                .collect();
            // The others factor the repair compares with its setting:
            // above it, the site is spared.
            let follow = c.others_factor(OTHERS_SIGMAS);
            println!(
                "{name},{x},{y},{color},{},{:.6},{:.6},{r:.3},{},{follow:.3}",
                c.hot as u8,
                c.value,
                c.replacement,
                o.join(",")
            );
        }
    }
}
