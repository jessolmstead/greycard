//! The view past 1:1 on the GPU: each picture pixel a block, as the
//! CPU reference (`magnify`) draws it, and nothing changed at 1:1 and
//! below.

use super::*;
use greycard_core::image::WorkingImage;
use greycard_edit::Mask;
use greycard_edit::mask::{Component, Mode};
use greycard_render::finish::Baked;

use crate::testing::device;

/// A checker of two levels, a hue across, so every picture pixel
/// differs from the one beside it.
fn checker(w: usize, h: usize) -> WorkingImage {
    WorkingImage {
        width: w,
        height: h,
        data: (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                let v = if (x + y).is_multiple_of(2) { 0.08 } else { 0.5 };
                [v, v * 0.7 + 0.02 * x as f32, 0.05 + 0.03 * y as f32]
            })
            .collect(),
    }
}

fn view_of(w: usize, h: usize, zoom: f32, center: (f32, f32)) -> View {
    let mut v = View::blank();
    v.zoom = zoom;
    v.center = center;
    v.plane = (w as f32, h as f32);
    v.frame_size = (w as f32, h as f32);
    v.canvas = [1.0, 0.0, 1.0];
    v
}

fn shot(renderer: &mut Renderer, w: u32, h: u32, v: &View) -> image::RgbaImage {
    let target = renderer.render(w, h, v).0;
    renderer.read_back(&target).expect("read back")
}

/// A shot's pixels as a picture, its bytes over 255.
fn as_image(shot: &image::RgbaImage) -> WorkingImage {
    WorkingImage {
        width: shot.width() as usize,
        height: shot.height() as usize,
        data: shot
            .pixels()
            .flat_map(|p| [0, 1, 2].map(|c| f32::from(p[c]) / 255.0))
            .collect(),
    }
}

/// Every target pixel is its block's: the block `side` target pixels
/// a side starting `offset` in, each pixel the same as the block's
/// first. How many blocks differ from the one to their right, so a
/// flat shot does not pass.
fn blocks(shot: &image::RgbaImage, side: u32, offset: (u32, u32), what: &str) -> usize {
    let (w, h) = shot.dimensions();
    let first = |v: u32, o: u32| if v < o { 0 } else { (v - o) / side * side + o };
    let mut steps = 0;
    for y in 0..h {
        for x in 0..w {
            let (bx, by) = (first(x, offset.0), first(y, offset.1));
            assert_eq!(
                shot.get_pixel(x, y),
                shot.get_pixel(bx, by),
                "{what}: {x},{y} is not its block's at {bx},{by}"
            );
            if x == bx
                && y == by
                && x + side < w
                && shot.get_pixel(x, y) != shot.get_pixel(x + side, y)
            {
                steps += 1;
            }
        }
    }
    steps
}

/// A tiny checker at 400% is exact 4x4 blocks, each block the pixel
/// the 1:1 draw shows there: the CPU reference (`magnify::drawn`)
/// over the 1:1 draw, to the byte. Panned on the grid and a third of
/// a picture pixel off it, the blocks moving with the pan.
#[test]
fn a_checker_at_400_is_the_cpus_blocks() {
    let Some((device, queue)) = device("a checker at 400%") else {
        return;
    };
    let mut renderer = Renderer::new(&device, &queue);
    let (w, h) = (12usize, 8usize);
    renderer.upload(&Halves::from_image(&checker(w, h), None));
    let center = (w as f32 / 2.0, h as f32 / 2.0);
    let one = shot(
        &mut renderer,
        w as u32,
        h as u32,
        &view_of(w, h, 1.0, center),
    );
    let picture = as_image(&one);
    let zoom = 4.0;
    let (tw, th) = (40u32, 28u32);
    for (pan, offset) in [(0.0f32, (0u32, 2u32)), (1.0 / 3.0, (3, 1))] {
        let at = (center.0 + pan, center.1 + pan);
        let four = shot(&mut renderer, tw, th, &view_of(w, h, zoom, at));
        let to_frame = |(x, y): (f32, f32)| {
            (
                (x - tw as f32 / 2.0) / zoom + at.0,
                (y - th as f32 / 2.0) / zoom + at.1,
            )
        };
        for y in 0..th {
            for x in 0..tw {
                let at = (x as f32 + 0.5, y as f32 + 0.5);
                let want = magnify::drawn(&picture, at, zoom, 1.0, false, to_frame, |q| q);
                let want = want.map(|v| (v * 255.0).round() as u8);
                let got = four.get_pixel(x, y);
                assert_eq!(&got.0[..3], &want, "pan {pan}: {x},{y}");
            }
        }
        let steps = blocks(&four, 4, offset, &format!("pan {pan}"));
        assert!(steps > 20, "pan {pan}: {steps} steps");
    }
}

/// Past 1:1 the whole of the draw is a picture pixel's, not the
/// picture alone: a mask shown over it, the output sharpen, and the
/// grain come out in the same blocks; and so does a picture turned
/// through the cubic, its blocks the plane's pixels, where no canvas
/// is in them.
#[test]
fn masks_sharpen_grain_and_a_turn_are_blocks_too() {
    let Some((device, queue)) = device("the rest of the draw at 400%") else {
        return;
    };
    let mut renderer = Renderer::new(&device, &queue);
    let (w, h) = (16usize, 12usize);
    renderer.upload(&Halves::from_image(&checker(w, h), None));
    let center = (w as f32 / 2.0, h as f32 / 2.0);
    let base = view_of(w, h, 4.0, center);
    let mut masked = base.clone();
    masked.locals = vec![Local {
        baked: Baked::of(&greycard_edit::Look::default()),
        mask: Mask {
            components: vec![Component {
                shape: greycard_edit::mask::Shape::Linear {
                    from: [0.2, 0.0],
                    to: [0.8, 0.0],
                },
                mode: Mode::Add,
                ..Default::default()
            }],
            ..Default::default()
        },
        enabled: true,
        rasters: Vec::new(),
        refined: None,
        opacity: 1.0,
    }];
    masked.show_mask = Some(0);
    let mut alone = masked.clone();
    alone.mask_alone = true;
    let mut sharpened = base.clone();
    sharpened.output_sharpen.amount = 2.0;
    sharpened.output_sharpen.radius = 1.0;
    sharpened.output_sharpen.threshold = 0.0;
    let mut grained = base.clone();
    grained.grain.amount = 0.5;
    for (what, v) in [
        ("a mask shown", &masked),
        ("a mask alone", &alone),
        ("the output sharpen", &sharpened),
        ("grain", &grained),
    ] {
        let s = shot(&mut renderer, 64, 48, v);
        assert!(blocks(&s, 4, (0, 0), what) > 20, "{what}");
    }
    // A sixth of a turn through the cubic: blocks whose pixels are
    // all picture are one color.
    let mut turned = base.clone();
    let (c, s) = (0.2f32.cos(), 0.2f32.sin());
    turned.matrix = [[c, -s], [s, c]];
    turned.cubic = true;
    let shot = shot(&mut renderer, 64, 48, &turned);
    let canvas = |p: &image::Rgba<u8>| p.0[..3] == [255, 0, 255];
    let mut whole = 0;
    for by in (0..48).step_by(4) {
        for bx in (0..64).step_by(4) {
            let block: Vec<_> = (0..16)
                .map(|k| *shot.get_pixel(bx + k % 4, by + k / 4))
                .collect();
            if block.iter().any(canvas) {
                continue;
            }
            assert!(
                block.iter().all(|p| *p == block[0]),
                "turned: the block at {bx},{by}"
            );
            whole += 1;
        }
    }
    assert!(whole > 50, "{whole} blocks of picture");
}

/// The frame pixel under each pixel of a `tw` by `th` shot of `v`, as
/// the shader's `frame_at` and the snap find it, its picture pixel's
/// column and row.
fn picture_pixel(v: &View, (tw, th): (u32, u32), x: u32, y: u32) -> (i64, i64) {
    let q = (
        (x as f32 + 0.5 - tw as f32 / 2.0) / v.zoom + v.center.0,
        (y as f32 + 0.5 - th as f32 / 2.0) / v.zoom + v.center.1,
    );
    (
        (q.0 / v.pixel).floor() as i64,
        (q.1 / v.pixel).floor() as i64,
    )
}

/// Every pixel of `shot` is the first one seen of its picture pixel:
/// one color a picture pixel, however wide the blocks come out at a
/// zoom that is no whole number. How many picture pixels differ from
/// the one to their right, so a flat shot does not pass.
fn picture_blocks(shot: &image::RgbaImage, v: &View, what: &str) -> usize {
    let size = shot.dimensions();
    let mut first = std::collections::HashMap::new();
    for y in 0..size.1 {
        for x in 0..size.0 {
            let k = picture_pixel(v, size, x, y);
            let p = *shot.get_pixel(x, y);
            let seen = *first.entry(k).or_insert(p);
            assert_eq!(p, seen, "{what}: {x},{y} of picture pixel {k:?}");
        }
    }
    first
        .iter()
        .filter(|(k, p)| first.get(&(k.0 + 1, k.1)).is_some_and(|r| r != *p))
        .count()
}

/// Past 1:1 at zooms that are no whole number, panned off the grid,
/// the blocks are still one picture pixel each, one or two target
/// pixels wide at 150%: the plain picture, a mask shown and alone,
/// the output sharpen (whose blur is read at one pixel of the block),
/// and grain. The plain picture's blocks are the 1:1 draw's pixels to
/// the byte, and the sharpened ones near the sharpened 1:1 draw's: the
/// blur is the same one, taken over blocks rather than pixels.
#[test]
fn fractional_zooms_are_blocks_too() {
    let Some((device, queue)) = device("the draw at 150%, 250%, and 333%") else {
        return;
    };
    let mut renderer = Renderer::new(&device, &queue);
    let (w, h) = (16usize, 12usize);
    renderer.upload(&Halves::from_image(&checker(w, h), None));
    let center = (w as f32 / 2.0, h as f32 / 2.0);
    let with = |v: &View, what: &str| -> View {
        let mut v = v.clone();
        match what {
            "a mask shown" | "a mask alone" => {
                v.locals = vec![Local {
                    baked: Baked::of(&greycard_edit::Look::default()),
                    mask: Mask {
                        components: vec![Component {
                            shape: greycard_edit::mask::Shape::Linear {
                                from: [0.2, 0.0],
                                to: [0.8, 0.0],
                            },
                            mode: Mode::Add,
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    enabled: true,
                    rasters: Vec::new(),
                    refined: None,
                    opacity: 1.0,
                }];
                v.show_mask = Some(0);
                v.mask_alone = what == "a mask alone";
            }
            "the output sharpen" => {
                v.output_sharpen.amount = 2.0;
                v.output_sharpen.radius = 1.0;
                v.output_sharpen.threshold = 0.0;
            }
            "grain" => v.grain.amount = 0.5,
            _ => {}
        }
        v
    };
    let one = view_of(w, h, 1.0, center);
    let plain_one = shot(&mut renderer, w as u32, h as u32, &one);
    let sharp_one = shot(
        &mut renderer,
        w as u32,
        h as u32,
        &with(&one, "the output sharpen"),
    );
    for (zoom, pan) in [(1.5f32, 0.37f32), (2.5, 0.29), (10.0 / 3.0, 0.61)] {
        // The view inside the picture, a picture pixel in from its
        // edges, so every pixel is picture.
        let (tw, th) = (
            ((w as f32 - 2.0) * zoom) as u32,
            ((h as f32 - 2.0) * zoom) as u32,
        );
        let base = view_of(w, h, zoom, (center.0 + pan, center.1 - pan));
        for what in [
            "the picture",
            "a mask shown",
            "a mask alone",
            "the output sharpen",
            "grain",
        ] {
            let v = with(&base, what);
            let s = shot(&mut renderer, tw, th, &v);
            let what = format!("{what} at {zoom}");
            let steps = picture_blocks(&s, &v, &what);
            assert!(steps > 20, "{what}: {steps} steps");
            let against = match what.as_str() {
                w if w.starts_with("the picture") => Some((&plain_one, 0u8)),
                // 8 levels at 150% on lavapipe, where the blocks are
                // one and two pixels wide under a blur of whole ones.
                w if w.starts_with("the output sharpen") => Some((&sharp_one, 12)),
                _ => None,
            };
            let Some((at_one, most)) = against else {
                continue;
            };
            let mut worst = 0u8;
            for y in 0..th {
                for x in 0..tw {
                    let (i, j) = picture_pixel(&v, (tw, th), x, y);
                    let a = s.get_pixel(x, y);
                    let b = at_one.get_pixel(i as u32, j as u32);
                    for k in 0..3 {
                        worst = worst.max(a[k].abs_diff(b[k]));
                    }
                }
            }
            eprintln!("{what}: at most {worst} levels from the 1:1 draw");
            assert!(worst <= most, "{what}: {worst} levels from the 1:1 draw");
        }
    }
}

/// A free crop's origin falls between source pixels, and the export,
/// with nothing to resample, takes the source pixel each frame pixel's
/// center falls in. Past 1:1 the blocks are those pixels, to the byte
/// as the 1:1 draw of the export shows them, at fractional origins
/// under every quarter turn, mirrored and not.
#[test]
fn a_free_crops_blocks_are_the_exports_pixels() {
    use greycard_edit::geometry::{Crop, Geometry};
    let Some((device, queue)) = device("a free crop at 400%") else {
        return;
    };
    let mut renderer = Renderer::new(&device, &queue);
    let (w, h) = (24usize, 16usize);
    let image = WorkingImage {
        width: w,
        height: h,
        data: (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [
                    0.05 + 0.9 * x as f32 / w as f32,
                    0.05 + 0.9 * y as f32 / h as f32,
                    0.05 + 0.04 * ((x * 7 + y * 13) % 11) as f32,
                ]
            })
            .collect(),
    };
    let (sw, sh) = (w as f32, h as f32);
    let mut cases = 0;
    for (cx, cy) in [(3.37f32, 2.61f32), (0.5, 0.25), (5.9, 1.05)] {
        for turns in 0..4u8 {
            for flip in [false, true] {
                let mut g = Geometry {
                    turns,
                    flip,
                    ..Geometry::default()
                };
                let (pw, ph) = g.plane_size(sw, sh);
                g.crop = Some(Crop {
                    x: cx / pw,
                    y: cy / ph,
                    w: (pw * 0.6) / pw,
                    h: (ph * 0.55) / ph,
                });
                let frame = g.frame(sw, sh, None);
                let export = greycard_render::geometry::apply(&image, &g).unwrap();
                let (fw, fh) = (export.width, export.height);
                assert_eq!((fw as f32, fh as f32), frame.size);
                renderer.upload(&Halves::from_image(&export, None));
                let at_one = shot(
                    &mut renderer,
                    fw as u32,
                    fh as u32,
                    &view_of(fw, fh, 1.0, (fw as f32 / 2.0, fh as f32 / 2.0)),
                );
                renderer.upload(&Halves::from_image(&image, None));
                let v = View {
                    matrix: g.matrix(),
                    persp: g.perspective(sw, sh),
                    plane: (pw, ph),
                    cubic: g.resamples(),
                    frame_origin: frame.origin,
                    frame_size: frame.size,
                    ..view_of(w, h, 4.0, (fw as f32 / 2.0, fh as f32 / 2.0))
                };
                assert!(!v.cubic);
                let s = shot(&mut renderer, 4 * fw as u32, 4 * fh as u32, &v);
                let what = format!("origin ({cx}, {cy}), {turns} turns, flip {flip}");
                for y in 0..s.height() {
                    for x in 0..s.width() {
                        assert_eq!(
                            s.get_pixel(x, y),
                            at_one.get_pixel(x / 4, y / 4),
                            "{what}: {x},{y}"
                        );
                    }
                }
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 24);
}

/// The loupe's copy half the size of the picture it stands for, at
/// 200% of the picture: the blocks are the picture's pixels, two
/// target pixels a side, read from the copy between its texels, not
/// the copy's own four-pixel texels.
#[test]
fn a_half_size_copy_at_200_is_the_pictures_blocks() {
    let Some((device, queue)) = device("a half-size copy at 200%") else {
        return;
    };
    let mut renderer = Renderer::new(&device, &queue);
    let (cw, ch) = (16u32, 12u32);
    let rgba: Vec<u8> = (0..cw * ch)
        .flat_map(|i| {
            let (x, y) = (i % cw, i / cw);
            let v = if (x + y) % 2 == 0 { 40 } else { 200 };
            [v, (x * 12) as u8, (y * 15) as u8, 255]
        })
        .collect();
    let texture = renderer.encoded_texture(cw, ch, &rgba);
    let scale = 0.5f32;
    let (tw, th) = (40u32, 32u32);
    let view = View {
        zoom: 2.0 / scale,
        pixel: scale,
        center: (cw as f32 / 2.0, ch as f32 / 2.0),
        plane: (cw as f32, ch as f32),
        frame_size: (cw as f32, ch as f32),
        canvas: [1.0, 0.0, 1.0],
        ..View::blank()
    };
    let target = renderer.render_encoded(
        tw,
        th,
        [1.0, 0.0, 1.0],
        &[Tile {
            texture: &texture,
            full: (cw, ch),
            view: view.clone(),
            rect: (0, 0, tw, th),
        }],
        None,
    );
    let s = renderer.read_back(&target).expect("read back");
    let steps = picture_blocks(&s, &view, "a half-size copy");
    assert!(steps > 20, "{steps} steps");
    // Two picture pixels in one copy texel differ: the blocks are not
    // the copy's.
    let inside = apart_in_a_texel(&s, &view);
    assert!(inside > 20, "{inside} picture pixels apart in a texel");
}

/// How many pixels of `shot` differ from the pixel two to their right
/// where that is the next picture pixel in the same texel of a texture
/// whose texels are two picture pixels a side.
fn apart_in_a_texel(shot: &image::RgbaImage, v: &View) -> usize {
    let size = shot.dimensions();
    (0..size.1)
        .flat_map(|y| (0..size.0 - 2).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (a, b) = (
                picture_pixel(v, size, x, y),
                picture_pixel(v, size, x + 2, y),
            );
            b.0 == a.0 + 1 && a.0 % 2 == 0 && shot.get_pixel(x, y) != shot.get_pixel(x + 2, y)
        })
        .count()
}

/// A picture past the device's textures goes up reduced by two; past
/// 1:1 its blocks are still the picture's pixels, read from the
/// reduced texture between its texels, not the reduced texels.
#[test]
fn a_reduced_texture_past_1_to_1_is_the_pictures_blocks() {
    let Some((device, queue)) = crate::testing::device_with(
        "a reduced texture at 200%",
        gpu::Limits {
            max_texture_dimension_2d: 64,
            ..gpu::Limits::default()
        },
    ) else {
        return;
    };
    let mut renderer = Renderer::new(&device, &queue);
    let (w, h) = (96usize, 64usize);
    let image = WorkingImage {
        width: w,
        height: h,
        data: (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [
                    0.05 + 0.9 * x as f32 / w as f32,
                    0.05 + 0.04 * ((x * 7 + y * 13) % 11) as f32,
                    0.05 + 0.9 * y as f32 / h as f32,
                ]
            })
            .collect(),
    };
    renderer.upload(&Halves::from_image(&image, None));
    assert_eq!(renderer.reduced_by(), 2);
    let v = view_of(w, h, 2.0, (w as f32 / 2.0 + 0.3, h as f32 / 2.0));
    let (tw, th) = (60u32, 60u32);
    let s = shot(&mut renderer, tw, th, &v);
    let steps = picture_blocks(&s, &v, "a reduced texture");
    assert!(steps > 100, "{steps} steps");
    let inside = apart_in_a_texel(&s, &v);
    assert!(inside > 100, "{inside} picture pixels apart in a texel");
}
