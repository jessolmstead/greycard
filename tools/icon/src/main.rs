//! Rasterize assets/icon/*.svg into the .ico files Windows links into a
//! binary and hands to Explorer.
//!
//!     cargo run --manifest-path tools/icon/Cargo.toml
//!
//! The SVG stays the one source for every platform's icon: Linux uses it
//! directly, the Mac tarball folds it into an .icns with sips and
//! iconutil at package time (scripts/package.sh), and Windows needs it
//! as an .ico before the link, not at package time, so the result is
//! committed. Run this when an SVG changes; nothing else does.
//!
//! The sizes are the ones Windows asks for: 16 and 32 in Explorer's
//! lists and the window's corner, 24 where the taskbar is small, 48 in
//! Explorer's medium icons, 64 and 128 on the way up, and 256 for the
//! extra-large view and the Start menu tile. An entry at 256 goes in
//! PNG-compressed, which is what the `ico` crate writes for it.

use std::path::Path;

const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tools/icon sits two below the repo root")
        .to_path_buf();

    for name in ["greycard", "application-x-greycard-edit"] {
        let svg = root.join("assets/icon").join(format!("{name}.svg"));
        let out = root.join("assets/icon").join(format!("{name}.ico"));
        write_ico(&svg, &out);
        println!("{}", out.display());
    }
}

fn write_ico(svg: &Path, out: &Path) {
    let data = std::fs::read(svg).unwrap_or_else(|e| panic!("{}: {e}", svg.display()));
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())
        .unwrap_or_else(|e| panic!("{}: {e}", svg.display()));

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in SIZES {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("a nonzero size");
        // The SVGs are square, so one scale covers both axes.
        let scale = size as f32 / tree.size().width();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        // tiny-skia hands back premultiplied alpha; ICO wants it straight.
        let image = ico::IconImage::from_rgba_data(size, size, unpremultiply(pixmap.data()));
        dir.add_entry(ico::IconDirEntry::encode(&image).expect("the entry encodes"));
    }

    let file = std::fs::File::create(out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    dir.write(file).expect("the .ico is written");
}

fn unpremultiply(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for px in data.chunks_exact(4) {
        let a = px[3];
        for c in &px[..3] {
            out.push(if a == 0 {
                0
            } else {
                // Rounded, so a flat color does not drift a step darker.
                ((u32::from(*c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
            });
        }
        out.push(a);
    }
    out
}
