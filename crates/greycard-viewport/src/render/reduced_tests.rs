//! A picture past the device's textures: up reduced, drawn in its own
//! geometry, and nothing made from its size past the device.

use super::*;
use greycard_core::image::WorkingImage;
use greycard_edit::Mask;
use greycard_edit::mask::{Component, Mode};
use greycard_render::finish::Baked;

use crate::testing::{device, device_with};

/// A device whose textures stop at `side`, standing in for a GPU a
/// picture is past.
fn small_device(what: &str, side: u32) -> Option<(gpu::Device, gpu::Queue)> {
    device_with(
        what,
        gpu::Limits {
            max_texture_dimension_2d: side,
            ..gpu::Limits::default()
        },
    )
}

/// A smooth picture with a sharp block at a known place: a ramp each
/// way, a slow ripple, and a bright square from (`bx`, `by`) 40
/// pixels a side, so a draw in the wrong place shows.
fn marked(w: usize, h: usize, (bx, by): (usize, usize)) -> WorkingImage {
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let ripple = 0.05 * ((x as f32 / 23.0).sin() + (y as f32 / 31.0).cos());
            let block = (bx..bx + 40).contains(&x) && (by..by + 40).contains(&y);
            let base = 0.05 + 0.3 * u + 0.2 * v + ripple;
            data.extend(if block {
                [0.9, 0.85, 0.2]
            } else {
                [base, base * 0.8 + 0.05, 0.3 - 0.2 * u]
            });
        }
    }
    WorkingImage {
        width: w,
        height: h,
        data,
    }
}

/// A view of the whole `w` by `h` picture at `zoom`, centered on
/// `center`.
fn view_of(w: usize, h: usize, zoom: f32, center: (f32, f32)) -> View {
    let mut v = View::blank();
    v.zoom = zoom;
    v.center = center;
    v.plane = (w as f32, h as f32);
    v.frame_size = (w as f32, h as f32);
    v
}

/// The mean difference of two shots, in levels.
fn mean_levels(a: &image::RgbaImage, b: &image::RgbaImage) -> f32 {
    assert_eq!(a.dimensions(), b.dimensions());
    let sum: u64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    sum as f32 / a.as_raw().len() as f32
}

/// A picture past the device's textures goes up reduced by the least
/// whole factor that fits, without a validation error (which the
/// window's handler would end the process on), and draws where the
/// whole picture draws: at fit, at 1:1 off center, and magnified,
/// within the reduction's blur of the same picture whole on a device
/// that holds it, and far nearer it than the same picture drawn a few
/// pixels off. The pick, the scopes, the navigator, and the planes of
/// its size read it in its own pixels too.
#[test]
fn a_picture_past_the_devices_textures_draws_reduced_in_its_own_geometry() {
    let Some((big_device, big_queue)) = device("the reduced picture's reference") else {
        return;
    };
    let Some((small_device, small_queue)) = small_device("the reduced picture", 256) else {
        return;
    };
    let (w, h) = (400usize, 300usize);
    let image = marked(w, h, (290, 60));
    let halves = Halves::from_image(&image, None);
    let mut whole = Renderer::new(&big_device, &big_queue);
    whole.upload(&halves);
    let mut reduced = Renderer::new(&small_device, &small_queue);
    reduced.upload(&halves);
    let texture = reduced.developed().expect("a picture");
    assert_eq!((texture.width(), texture.height()), (200, 150));
    assert_eq!(reduced.developed_size(), Some((400, 300)));
    assert_eq!(reduced.reduced_by(), 2);
    assert_eq!(whole.reduced_by(), 1);
    let shot = |r: &mut Renderer, tw: u32, th: u32, v: &View| {
        let target = r.render(tw, th, v).0;
        r.read_back(&target).expect("read back")
    };
    for (what, tw, th, zoom, center, most) in [
        ("fit", 200u32, 150u32, 0.5f32, (200.0f32, 150.0f32), 1.5f32),
        ("1:1 on the block", 120, 90, 1.0, (310.0, 80.0), 2.5),
        ("magnified", 96, 64, 3.0, (300.0, 70.0), 2.5),
    ] {
        for cubic in [false, true] {
            let mut v = view_of(w, h, zoom, center);
            v.cubic = cubic;
            let want = shot(&mut whole, tw, th, &v);
            let got = shot(&mut reduced, tw, th, &v);
            let near = mean_levels(&want, &got);
            let off = View {
                center: (
                    center.0 + 6.0 / zoom.min(1.0),
                    center.1 + 4.0 / zoom.min(1.0),
                ),
                ..v.clone()
            };
            let moved = mean_levels(&want, &shot(&mut reduced, tw, th, &off));
            eprintln!(
                "{what}, cubic {cubic}: {near:.2} levels from the whole picture, {moved:.2} when off"
            );
            assert!(near <= most, "{what}, cubic {cubic}: {near} levels off");
            assert!(
                near * 3.0 < moved,
                "{what}, cubic {cubic}: not told from off"
            );
        }
    }
    // The pick, in the picture's own pixels: inside the block and on
    // the ramp, with each size of square.
    for (x, y) in [(305i64, 75i64), (100, 200), (399, 299)] {
        for reach in [2u32, 10] {
            let want = whole.sample(x, y, reach).unwrap().expect("on the picture");
            let got = reduced
                .sample(x, y, reach)
                .unwrap()
                .expect("on the picture");
            for k in 0..3 {
                assert!(
                    (want[k] - got[k]).abs() <= 0.03,
                    "the pick at {x},{y} reach {reach}: {got:?} against {want:?}"
                );
            }
        }
    }
    assert!(
        reduced.sample(400, 10, 2).unwrap().is_none(),
        "off the picture"
    );
    // The scopes and the navigator, of the whole frame.
    let v = view_of(w, h, 0.5, (200.0, 150.0));
    let bins = |r: &mut Renderer| {
        for _ in 0..100 {
            let (bins, in_flight) = r.analyze(&v, Scope::Rgb, None);
            if !in_flight && let Some((_, bins)) = bins {
                return bins.to_vec();
            }
            let _ = r.device.poll(gpu::PollType::wait_indefinitely());
        }
        panic!("the scope's read back never landed");
    };
    // The analysis is as wide as each device takes, so the counts
    // differ by the pixels drawn: their shares are compared.
    let (a, b) = (bins(&mut whole), bins(&mut reduced));
    let share = |bins: &[u32]| {
        let total: u64 = bins.iter().map(|&v| u64::from(v)).sum();
        assert!(total > 0, "the bins are empty");
        bins.iter()
            .map(|&v| v as f64 / total as f64)
            .collect::<Vec<_>>()
    };
    let apart: f64 = share(&a)
        .iter()
        .zip(share(&b))
        .map(|(x, y)| (x - y).abs())
        .sum();
    eprintln!("the scopes' shares: {apart:.3} apart of 2");
    assert!(apart < 0.25, "the scopes' shares are {apart} apart");
    let navigator = reduced.take_navigator().expect("a navigator picture");
    // As wide as the small device takes, at the frame's shape.
    assert_eq!(
        super::scopes::analysis_size((400.0, 300.0), 256),
        (256, 192)
    );
    assert_eq!((navigator.width, navigator.height), (256, 192));
    assert!(
        reduced.analyzed_at(0.76, 0.25, 0.0).is_some(),
        "the readout"
    );
    // Planes and the range masks' picture at the picture's size go up
    // reduced alike.
    let planes = greycard_render::detail::DetailPlanes::with_sample(
        None,
        Some(Arc::new(image.clone())),
        1.0,
    );
    reduced.set_detail(planes.as_ref());
    let mut local = Local {
        baked: Baked::of(&greycard_edit::Look::default()),
        mask: Mask {
            components: vec![Component {
                shape: Shape::LUMINANCE,
                mode: Mode::Add,
                ..Default::default()
            }],
            ..Default::default()
        },
        enabled: true,
        rasters: Vec::new(),
        refined: None,
        opacity: 1.0,
    };
    local.baked.light.exposure = 1.0;
    let mut masked = view_of(w, h, 0.5, (200.0, 150.0));
    masked.locals = vec![local];
    let _ = shot(&mut reduced, 200, 150, &masked);
    assert_eq!(
        (reduced.sample.width(), reduced.sample.height()),
        (200, 150)
    );
}

/// The edges of the limit, either way round, and a tiny limit with
/// odd sides: a picture at the limit goes up whole, a pixel over it
/// halved, and every texture made of it is within the limit.
#[test]
fn a_picture_at_the_limit_goes_up_whole_and_one_over_halved() {
    let Some((device, queue)) = small_device("the limit's edges", 64) else {
        return;
    };
    let mut renderer = Renderer::new(&device, &queue);
    for ((w, h), up) in [
        ((64, 9), (64, 9)),
        ((65, 9), (33, 5)),
        ((9, 65), (5, 33)),
        ((9, 64), (9, 64)),
        ((129, 7), (43, 3)),
        ((1, 1), (1, 1)),
    ] {
        let image = marked(w, h, (0, 0));
        renderer.upload(&Halves::from_image(&image, None));
        let t = renderer.developed().expect("a picture").clone();
        assert_eq!((t.width(), t.height()), up, "{w}x{h}");
        assert_eq!(renderer.developed_size(), Some((w as u32, h as u32)));
        let v = view_of(w, h, 0.5, (w as f32 / 2.0, h as f32 / 2.0));
        let target = renderer.render(40, 40, &v).0;
        renderer.read_back(&target).expect("read back");
        // Handed back as the Original hands it, it draws the same.
        let size = renderer.developed_size().unwrap();
        renderer.set_source_sized(t, size);
        assert_eq!(renderer.developed_size(), Some(size));
    }
    // The camera's picture for the loupe: reduced the same way, its
    // texture the size it holds.
    let rgba = vec![200u8; 130 * 3 * 4];
    let t = renderer.encoded_texture(130, 3, &rgba);
    assert_eq!((t.width(), t.height()), (44, 1));
}

/// A frame far taller than wide: its scopes' picture stays within the
/// side every device takes, narrower than its usual width, in the
/// 64s a copy's rows want, and a frame of the usual shapes keeps the
/// width it had. On a device, the analysis runs.
#[test]
fn a_tall_frames_scopes_stay_within_every_devices_textures() {
    use super::scopes::{ANALYSIS_WIDTH, analysis_size};
    let card = 16384;
    assert_eq!(analysis_size((6000.0, 4000.0), card), (ANALYSIS_WIDTH, 341));
    assert_eq!(analysis_size((4000.0, 6000.0), card), (ANALYSIS_WIDTH, 768));
    assert_eq!(analysis_size((17000.0, 400.0), card), (ANALYSIS_WIDTH, 12));
    for side in [card, 8192, 2048, 300] {
        for frame in [
            (400.0, 17000.0),
            (100.0, 9000.0),
            (10.0, 90000.0),
            (6000.0, 4000.0),
        ] {
            let (w, h) = analysis_size(frame, side);
            assert!(
                w <= ANALYSIS_WIDTH && w <= side,
                "{frame:?} at {side}: {w}x{h}"
            );
            assert!(h <= side, "{frame:?} at {side}: {w}x{h}");
            assert!(w.is_multiple_of(64) && w >= 64, "{frame:?} at {side}: {w}");
        }
    }
    assert_eq!(analysis_size((400.0, 17000.0), card), (384, 16320));
    // A frame that fit the device's textures before is drawn as it
    // was, however tall: 512 wide.
    assert_eq!(
        analysis_size((1000.0, 20000.0), card),
        (ANALYSIS_WIDTH, 10240)
    );
    assert_eq!(analysis_size((400.0, 17000.0), 8192), (192, 8160));
    let Some((device, queue)) = device("a tall frame's scopes") else {
        return;
    };
    let (w, h) = (20usize, 900usize);
    let image = marked(w, h, (0, 400));
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&Halves::from_image(&image, None));
    let v = view_of(w, h, 0.5, (10.0, 450.0));
    for _ in 0..100 {
        let (bins, in_flight) = renderer.analyze(&v, Scope::Rgb, None);
        if !in_flight && bins.is_some() {
            let nav = renderer.take_navigator().expect("a navigator picture");
            assert_eq!((nav.width, nav.height), analysis_size((20.0, 900.0), 8192));
            return;
        }
        let _ = renderer.device.poll(gpu::PollType::wait_indefinitely());
    }
    panic!("the scope's read back never landed");
}

/// A brush's raster on a frame more than four times taller than wide
/// is taller than 8192: on a device that stops there it goes up
/// reduced and its mask draws where it should; on one that takes it,
/// it goes up whole as it always did.
#[test]
fn a_tall_frames_raster_goes_up_within_every_devices_textures() {
    use greycard_edit::brush::{MOST_SIDE, RASTER_WIDTH, Raster};
    let (w, h) = (40usize, 240usize);
    let aspect = h as f32 / w as f32;
    let rows = greycard_edit::brush::raster_height(RASTER_WIDTH, aspect);
    assert!(rows > MOST_SIDE, "the premise: {rows} rows");
    // The top third painted in, the rest not.
    let data: Vec<u8> = (0..rows)
        .flat_map(|y| std::iter::repeat_n(if y < rows / 3 { 255 } else { 0 }, RASTER_WIDTH))
        .collect();
    let raster = RasterRef(Arc::new(Raster::from_data(aspect, RASTER_WIDTH, data)));
    assert_eq!(
        super::locals::raster_size(&raster.0, 8192),
        (1024, rows as u32 / 2)
    );
    assert_eq!(
        super::locals::raster_size(&raster.0, 16384),
        (RASTER_WIDTH as u32, rows as u32),
        "whole on a card that takes it"
    );
    let mut local = Local {
        baked: Baked::of(&greycard_edit::Look::default()),
        mask: Mask {
            components: vec![Component {
                shape: Shape::Subject {},
                mode: Mode::Add,
                ..Default::default()
            }],
            ..Default::default()
        },
        enabled: true,
        rasters: vec![Some(raster)],
        refined: None,
        opacity: 1.0,
    };
    local.baked.light.exposure = 1.0;
    let packed = super::locals::locals_gpu(std::slice::from_ref(&local), 8192, None);
    let (aw, ah) = super::locals::Brushes::size(&packed.rasters);
    assert!(aw as usize <= MOST_SIDE && ah as usize <= MOST_SIDE);
    let Some((device, queue)) = device("a tall frame's raster") else {
        return;
    };
    let image = marked(w, h, (0, 200));
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&Halves::from_image(&image, None));
    let mut v = view_of(w, h, 1.0, (w as f32 / 2.0, h as f32 / 2.0));
    v.locals = vec![local];
    v.show_mask = Some(0);
    v.mask_alone = true;
    let target = renderer.render(w as u32, h as u32, &v).0;
    let shown = renderer.read_back(&target).expect("read back");
    assert!(shown.get_pixel(20, 20)[0] > 250, "in the painted third");
    assert!(shown.get_pixel(20, 200)[0] < 5, "below it");
}

/// The viewport shader with no mapping into a reduced texture at all:
/// `to_texture` the identity, which is what the shader was before
/// pictures past the device could be shown. A picture that fits must
/// draw through the real shader byte for byte as through this one.
fn mapping_free_shader() -> String {
    let shader = include_str!("../viewport.wgsl");
    let start = shader.find("fn to_texture(").expect("to_texture");
    let end = start + shader[start..].find("\n}\n").expect("its end") + 3;
    format!(
        "{}fn to_texture(s: vec2<f32>) -> vec2<f32> {{\n    return s;\n}}\n{}",
        &shader[..start],
        &shader[end..]
    )
}

/// A picture with something at every scale: ramps, a ripple, a
/// checker of odd periods, and a hash of noise.
fn busy(w: usize, h: usize) -> WorkingImage {
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let n = ((x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)) % 1000) as f32 / 1000.0;
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let r = 0.02 + 0.6 * u + 0.05 * n + 0.1 * (x as f32 / 7.0).sin().abs();
            let g = 0.03 + 0.5 * v + 0.04 * n;
            let checker = if (x / 9 + y / 13) % 2 == 0 { 0.2 } else { 0.0 };
            data.extend([r, g, 0.4 * (1.0 - u) * v + 0.03 * n + checker]);
        }
    }
    WorkingImage {
        width: w,
        height: h,
        data,
    }
}

/// A picture that fits the device draws exactly as it did before a
/// picture could go up reduced: through the shader as it is and
/// through the one with no mapping (`mapping_free_shader`), the same
/// bytes at every zoom from a sixth to three, cubic or not, at each
/// turn the texture can lag by, straight and angled under a
/// perspective, with the mixer and the output sharpen and without,
/// under masks that read the guide, the planes, the sharpen's delta,
/// and the range masks' own picture, the masks drawn alone too; and
/// the pick, the scopes' bins, and the navigator the same. Any
/// arithmetic on the way to a texel moves some by an ULP on some
/// cards, which this sees.
#[test]
fn a_picture_that_fits_draws_byte_for_byte_as_before() {
    use greycard_core::develop::sharpen::{Radius, SharpenOptions, Threshold, sharpen_with_delta};
    let Some((device, queue)) = device("a fitting picture's draw against the mapping-free one")
    else {
        return;
    };
    let reference = mapping_free_shader();
    assert_ne!(reference, include_str!("../viewport.wgsl"));
    let (mut views, mut differ) = (0usize, Vec::new());
    for (w, h) in [(360usize, 240usize), (1001, 667), (257, 1031), (4100, 37)] {
        let mut image = busy(w, h);
        let before = image.clone();
        let options = SharpenOptions {
            radius: Radius::Fixed(0.9),
            contrast: Threshold::Fixed(0.0),
            ..Default::default()
        };
        let (_, _, delta) = sharpen_with_delta(&mut image, None, &options, None, 1.0);
        let planes = greycard_render::detail::DetailPlanes::make(
            &image,
            None,
            1.0,
            greycard_edit::DetailWants {
                texture: true,
                clarity: true,
                structure: true,
                ..Default::default()
            },
        )
        .expect("planes")
        .with_sharpen(greycard_render::detail::sharpen_delta(&delta));
        let planes = greycard_render::detail::DetailPlanes::with_sample(
            Some(Arc::new(planes)),
            Some(Arc::new(before)),
            1.0,
        );
        let scale = 8;
        let data: Vec<f32> = (0..h.div_ceil(scale))
            .flat_map(|y| (0..w.div_ceil(scale)).map(move |x| (x, y)))
            .map(|(x, y)| ((x * 7 + y * 3) % 11) as f32 * 0.3 - 1.5)
            .collect();
        let guide = greycard_render::finish::Guide {
            width: w.div_ceil(scale),
            height: h.div_ceil(scale),
            scale,
            range: greycard_render::finish::plane_range(&data),
            data,
            picture: None,
        };
        let halves = Halves::from_image(&image, None);
        let made = |r: &mut Renderer| {
            r.upload(&halves);
            r.set_guide(&guide);
            r.set_detail(planes.as_ref());
            assert_eq!(r.reduced_by(), 1);
        };
        let mut real = Renderer::new(&device, &queue);
        let mut free = Renderer::with_shader(&device, &queue, &reference);
        made(&mut real);
        made(&mut free);
        let local = |shapes: Vec<(Shape, Mode)>, detail: [f32; 5]| {
            let mut l = Local {
                baked: Baked::of(&greycard_edit::Look::default()),
                mask: Mask {
                    components: shapes
                        .into_iter()
                        .map(|(shape, mode)| Component {
                            shape,
                            mode,
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
                enabled: true,
                rasters: Vec::new(),
                refined: None,
                opacity: 1.0,
            };
            l.baked.light.exposure = 0.7;
            l.baked.detail = detail;
            l
        };
        let locals = vec![
            local(
                vec![(
                    Shape::Radial {
                        center: [0.4, 0.3],
                        radius: [0.2, 0.15],
                        angle: 0.3,
                        feather: 0.4,
                    },
                    Mode::Add,
                )],
                [-0.7, 0.5, 0.0, 0.3, 0.8],
            ),
            local(
                vec![(
                    Shape::Luminance {
                        low: 0.3,
                        high: 0.8,
                        low_feather: 0.1,
                        high_feather: 0.05,
                    },
                    Mode::Add,
                )],
                [0.4, 0.0, 0.0, 0.0, -0.6],
            ),
            local(vec![(Shape::skin(), Mode::Add)], [0.0; 5]),
        ];
        let mut both = |v: &View, what: String| {
            let a = real.render(173, 131, v).0;
            let a = real.read_back(&a).unwrap();
            let b = free.render(173, 131, v).0;
            let b = free.read_back(&b).unwrap();
            views += 1;
            if a.as_raw() != b.as_raw() {
                differ.push(what);
            }
        };
        for zoom in [0.17f32, 0.5, 1.0, 1.7, 3.0] {
            for cubic in [false, true] {
                for turn in 0u8..4 {
                    for angled in [false, true] {
                        for mixer in [false, true] {
                            let (pw, ph) = if turn % 2 == 1 { (h, w) } else { (w, h) };
                            let mut v = View::blank();
                            v.zoom = zoom;
                            v.center = (pw as f32 * 0.43, ph as f32 * 0.57);
                            v.plane = (pw as f32, ph as f32);
                            v.frame_size = (pw as f32, ph as f32);
                            v.cubic = cubic;
                            v.source_turn = turn;
                            v.light.tone.shadows = 0.5;
                            v.light.tone.highlights = -0.4;
                            if angled {
                                let a: f32 = 0.11;
                                v.matrix = [[a.cos(), -a.sin()], [a.sin(), a.cos()]];
                                v.frame_origin = (pw as f32 * 0.1, ph as f32 * 0.1);
                                v.frame_size = (pw as f32 * 0.8, ph as f32 * 0.8);
                                v.persp = [0.00002, -0.00001];
                            }
                            v.mixer.enabled = mixer;
                            v.mixer.hue[1] = 20.0;
                            v.mixer.saturation[5] = 0.6;
                            v.output_sharpen.enabled = mixer;
                            v.output_sharpen.amount = 1.0;
                            v.output_sharpen.radius = 1.5;
                            v.locals = locals.clone();
                            v.vignette.enabled = true;
                            v.vignette.amount = -0.3;
                            v.grain.enabled = true;
                            v.grain.amount = 0.2;
                            let what = format!(
                                "{w}x{h} zoom {zoom} cubic {cubic} turn {turn} angled {angled} mixer {mixer}"
                            );
                            both(&v, what.clone());
                            if !mixer && !angled {
                                for k in 0..3 {
                                    let m = View {
                                        show_mask: Some(k),
                                        mask_alone: true,
                                        ..v.clone()
                                    };
                                    both(&m, format!("{what} mask {k}"));
                                }
                            }
                        }
                    }
                }
            }
        }
        for (x, y) in [
            (0i64, 0i64),
            (w as i64 / 3, h as i64 / 2),
            (w as i64 - 1, h as i64 - 1),
        ] {
            for reach in [0u32, 2, 10] {
                assert_eq!(
                    real.sample(x, y, reach).unwrap(),
                    free.sample(x, y, reach).unwrap(),
                    "{w}x{h}: the pick at {x},{y} reach {reach}"
                );
            }
        }
        let mut v = View::blank();
        v.zoom = 0.5;
        v.center = (w as f32 / 2.0, h as f32 / 2.0);
        v.plane = (w as f32, h as f32);
        v.frame_size = (w as f32, h as f32);
        v.locals = locals.clone();
        let scopes = |r: &mut Renderer| {
            for _ in 0..200 {
                let (bins, in_flight) = r.analyze(&v, Scope::Rgb, None);
                if !in_flight && let Some((_, bins)) = bins {
                    let bins = bins.to_vec();
                    let nav = r.take_navigator().map(|n| n.pixels);
                    return (bins, nav);
                }
                let _ = r.device.poll(gpu::PollType::wait_indefinitely());
            }
            panic!("the scope's read back never landed");
        };
        assert!(
            scopes(&mut real) == scopes(&mut free),
            "{w}x{h}: the scopes and the navigator"
        );
    }
    eprintln!("{views} views, {} differ", differ.len());
    assert!(
        differ.is_empty(),
        "{} of {views} views differ from the mapping-free draw, the first {:?}",
        differ.len(),
        &differ[..differ.len().min(5)]
    );
}

/// Where a one-box-wide bright column and row land, drawn from the
/// picture reduced by k against the picture whole: the same place to
/// a tenth of a pixel, for sides that are not multiples of k and k of
/// 2, 3, and 4, near each edge and in the middle, at 1:1 and above;
/// and the pick reads the box there.
#[test]
fn a_reduced_picture_draws_each_box_where_the_whole_picture_has_it() {
    let Some((big, big_queue)) = device("the whole picture for the drift") else {
        return;
    };
    for (w, h, k) in [
        (701usize, 233usize, 3usize),
        (700, 311, 3),
        (511, 157, 2),
        (999, 254, 4),
        (257, 255, 2),
    ] {
        let most = w.max(h).div_ceil(k) as u32;
        assert_eq!(
            reduce::factor(w as u32, h as u32, most) as usize,
            k,
            "{w}x{h}"
        );
        assert!(
            w % k != 0 || h % k != 0,
            "{w}x{h}: a side past a multiple of {k}"
        );
        let Some((small, small_queue)) = small_device("the reduced picture for the drift", most)
        else {
            return;
        };
        // Boxes: near the near edge, the middle, and near the far one
        // (the measure's window of two boxes each side inside the
        // picture), each way.
        let boxes = |n: usize| [2, n / k / 2, n / k - 3];
        for (vertical, at) in boxes(w)
            .map(|b| (true, b))
            .into_iter()
            .chain(boxes(h).map(|b| (false, b)))
        {
            let lo = at * k;
            let mut data = vec![0.02f32; w * h * 3];
            for y in 0..h {
                for x in 0..w {
                    let on = if vertical {
                        (lo..lo + k).contains(&x)
                    } else {
                        (lo..lo + k).contains(&y)
                    };
                    if on {
                        data[(y * w + x) * 3..][..3].copy_from_slice(&[0.9; 3]);
                    }
                }
            }
            let image = WorkingImage {
                width: w,
                height: h,
                data,
            };
            let halves = Halves::from_image(&image, None);
            let mid = lo as f32 + k as f32 / 2.0;
            let center = if vertical {
                (mid, h as f32 / 2.0)
            } else {
                (w as f32 / 2.0, mid)
            };
            for zoom in [1.0f32, 3.0] {
                let centroid = |d: &gpu::Device, q: &gpu::Queue| {
                    let mut r = Renderer::new(d, q);
                    r.upload(&halves);
                    let v = view_of(w, h, zoom, center);
                    let n = 64u32;
                    let t = r.render(n, n, &v).0;
                    let img = r.read_back(&t).unwrap();
                    // Within two boxes of the middle, over the
                    // ground's level there, so neither the canvas past
                    // an edge nor the ground weighs.
                    let reach = (2.0 * k as f32 * zoom).ceil() as u32;
                    let span = n / 2 - reach..n / 2 + reach;
                    let level = |i: u32| {
                        let (x, y) = if vertical { (i, n / 2) } else { (n / 2, i) };
                        f64::from(img.get_pixel(x, y)[0])
                    };
                    let ground = span.clone().map(level).fold(f64::MAX, f64::min);
                    let (mut s, mut m) = (0f64, 0f64);
                    for i in span {
                        let p = level(i) - ground;
                        s += p * (f64::from(i) + 0.5);
                        m += p;
                    }
                    assert!(m > 0.0, "{w}x{h}: the box is drawn");
                    // In the picture's pixels from the box's center.
                    ((s / m) - f64::from(n) / 2.0) / f64::from(zoom)
                };
                let whole = centroid(&big, &big_queue);
                let reduced = centroid(&small, &small_queue);
                let what = format!(
                    "{w}x{h} k {k}, {} box {at}, zoom {zoom}",
                    if vertical { "column" } else { "row" }
                );
                eprintln!("{what}: whole {whole:+.3}, reduced {reduced:+.3} px");
                // The whole picture's is off the box's center by the
                // draw's own sampling, under half a pixel.
                assert!(whole.abs() <= 0.5, "{what}: the whole picture's at {whole}");
                assert!(
                    (reduced - whole).abs() <= 0.1,
                    "{what}: the reduced picture's {reduced} against {whole}"
                );
            }
            // The pick at the box's middle reads it bright, and just
            // past it dark.
            let mut r = Renderer::new(&small, &small_queue);
            r.upload(&halves);
            let (inside, past) = if vertical {
                (
                    (lo as i64 + k as i64 / 2, h as i64 / 2),
                    (lo as i64 + k as i64, h as i64 / 2),
                )
            } else {
                (
                    (w as i64 / 2, lo as i64 + k as i64 / 2),
                    (w as i64 / 2, lo as i64 + k as i64),
                )
            };
            let read = |(x, y): (i64, i64)| r.sample(x, y, 0).unwrap().expect("on it")[0];
            assert!(read(inside) > 0.8, "{w}x{h} box {at}: the pick inside");
            assert!(read(past) < 0.1, "{w}x{h} box {at}: the pick past it");
        }
    }
}
