//! The vignette through the export: in the scene, as every edit made
//! before its place came in has it, the same bytes as before; on the
//! picture as shown, after every display curve and the look table.

use super::*;
use greycard_edit::camera::{Camera, ProfileChoice};
use greycard_edit::look::{LookLut, LutChoice};
use greycard_edit::vignette::Place;
use greycard_edit::{DisplayCurve, Edit, Vignette};

/// A synthetic camera profile with a tone curve, and a look table that
/// swaps red and blue, by any name.
struct Profiled;

impl greycard_edit::Loader for Profiled {
    fn profile(&self, _: &str) -> Option<std::sync::Arc<greycard_core::color::Profile>> {
        let bytes = greycard_core::testing::dcp::synthetic();
        let dcp = greycard_core::dcp::Dcp::from_bytes(&bytes).unwrap();
        Some(std::sync::Arc::new(dcp.profile().unwrap()))
    }
    fn look(&self, _: &str, _: f32, _: DisplayCurve) -> Option<greycard_core::lut::Look> {
        let cube = "LUT_3D_SIZE 2\n0 0 0\n0 0 1\n0 1 0\n0 1 1\n1 0 0\n1 0 1\n1 1 0\n1 1 1\n";
        let lut = greycard_core::lut::Lut3d::from_cube(cube).unwrap();
        Some(greycard_core::lut::Look::new(std::sync::Arc::new(lut), 0.7).unwrap())
    }
}

/// A frame with a bright sky across its top, over display white, and
/// colored ramps under it.
fn frame(w: usize, h: usize) -> WorkingImage {
    let mut image = WorkingImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let i = (y * w + x) * 3;
            let px = if v < 0.3 {
                [6.0 + 2.0 * u, 6.5, 7.0 - u]
            } else {
                [0.02 + 0.6 * u, 0.05 + 0.3 * v, 0.03 + 0.2 * u * v]
            };
            image.data[i..i + 3].copy_from_slice(&px);
        }
    }
    image
}

/// The ways a picture reaches the display: per channel, AgX, either
/// under a look table, the camera profile's own curve, and a picture
/// already rendered.
fn finishes() -> Vec<(Edit, finish::Source)> {
    let mut out = Vec::new();
    for curve in [DisplayCurve::Channels, DisplayCurve::Agx] {
        for look in [false, true] {
            let edit = Edit {
                display_curve: curve,
                look_lut: LookLut {
                    lut: if look {
                        LutChoice::Named("Film".into())
                    } else {
                        LutChoice::None
                    },
                    strength: 1.0,
                },
                ..Default::default()
            };
            out.push((edit, finish::Source::SCENE));
        }
    }
    let profiled = Edit {
        camera: Camera {
            profile: ProfileChoice::Named("P".into()),
            look_table: true,
            tone_curve: true,
        },
        ..Default::default()
    };
    out.push((profiled, finish::Source::SCENE));
    out.push((Edit::default(), finish::Source::Display));
    out
}

fn rendered(image: &WorkingImage, edit: &Edit, source: finish::Source) -> Vec<u16> {
    let settings = Settings {
        format: Format::Tiff,
        ..Settings::default()
    };
    let r = render(
        image,
        edit,
        &Profiled,
        (image.width as u32, image.height as u32),
        &settings,
        &Default::default(),
        1.0,
        None,
        source,
        None,
        None,
    );
    match r.pixels {
        Pixels::Sixteen(p) => p,
        Pixels::Eight(p) => p.into_iter().map(u16::from).collect(),
    }
}

/// FNV-1a over the bytes, which no release of anything changes.
fn fnv(hash: u64, values: &[u16]) -> u64 {
    values
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .fold(hash, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// The vignettes swept: amounts both ways and none, the shape at its
/// ends, the switch off.
fn vignettes() -> Vec<Vignette> {
    let mut out = Vec::new();
    for amount in [-5.0, -2.0, -0.6, 0.0, 0.7, 3.0] {
        for (midpoint, feather, roundness) in [(0.5, 0.5, 0.0), (0.1, 0.0, -1.0), (0.8, 1.0, 1.0)] {
            out.push(Vignette {
                amount,
                midpoint,
                feather,
                roundness,
                ..Vignette::default()
            });
        }
    }
    out.push(Vignette {
        enabled: false,
        amount: -2.0,
        ..Vignette::default()
    });
    out
}

/// Every vignette over every finish, the output sharpen and the grain
/// on one each, in the scene: the bytes the build before the place
/// wrote, by their hash, taken from it on Linux: another platform's
/// `powf` and `exp2` round their own way.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "glibc's rounding")]
fn the_scene_vignette_renders_as_before() {
    let image = frame(40, 28);
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for (base, source) in finishes() {
        for (i, vignette) in vignettes().into_iter().enumerate() {
            let mut edit = Edit {
                vignette,
                ..base.clone()
            };
            if i % 5 == 1 {
                edit.output_sharpen.amount = 1.2;
                edit.output_sharpen.radius = 1.5;
            }
            if i % 7 == 2 {
                edit.grain.amount = 0.5;
            }
            hash = fnv(hash, &rendered(&image, &edit, source));
        }
    }
    assert_eq!(hash, SCENE_HASH, "{hash:#018x}");
}

/// [`the_scene_vignette_renders_as_before`]'s hash: from the build
/// before the vignette's place, then taken again when the gamut op came
/// in the clamp's place (`crate::gamut`), which moves the bytes of
/// every finish but AgX's; the place's own change was held to the old
/// hash before.
const SCENE_HASH: u64 = 0xc284_48f7_3e04_fd65;

fn placed(vignette: Vignette, place: Place) -> Vignette {
    Vignette { place, ..vignette }
}

/// The mean encoded value of the pixel at (x, y).
fn mean_at(p: &[u16], w: usize, x: usize, y: usize) -> f32 {
    let i = (y * w + x) * 3;
    p[i..i + 3].iter().map(|&v| f32::from(v)).sum::<f32>() / 3.0
}

/// On the display a bright sky in the corner darkens, under every
/// display curve, a look table, the profile's curve, and a picture
/// already rendered; in the scene it rolls through the curve's
/// shoulder and stays bright. The middle, where the vignette does not
/// reach, is the same either way.
#[test]
fn the_display_vignette_darkens_a_bright_edge() {
    let (w, h) = (40, 28);
    let image = frame(w, h);
    let vignette = Vignette {
        amount: -1.5,
        midpoint: 0.2,
        feather: 0.3,
        ..Vignette::default()
    };
    for (base, source) in finishes() {
        let render_at = |place| {
            let edit = Edit {
                vignette: placed(vignette, place),
                ..base.clone()
            };
            rendered(&image, &edit, source)
        };
        let (scene, shown) = (render_at(Place::Scene), render_at(Place::Display));
        let middle = (w / 2, h / 2);
        assert_eq!(
            mean_at(&scene, w, middle.0, middle.1),
            mean_at(&shown, w, middle.0, middle.1),
            "{:?} {source:?}",
            base.display_curve
        );
        let (s, d) = (mean_at(&scene, w, 0, 0), mean_at(&shown, w, 0, 0));
        assert!(
            d < s - 0.1 * 65535.0,
            "{:?} {source:?}: the corner {d} on the display, {s} in the scene",
            base.display_curve
        );
    }
}

/// At no amount, or with the section off, where it acts is nothing.
#[test]
fn at_no_amount_the_place_changes_nothing() {
    let image = frame(24, 16);
    for (base, source) in finishes() {
        for vignette in [
            Vignette::default(),
            Vignette {
                enabled: false,
                amount: -3.0,
                ..Vignette::default()
            },
        ] {
            let at = |place| {
                let edit = Edit {
                    vignette: placed(vignette, place),
                    ..base.clone()
                };
                rendered(&image, &edit, source)
            };
            assert_eq!(at(Place::Scene), at(Place::Display));
        }
    }
}

/// Going up on the display the corners go toward white: a dark corner
/// lightens, and nothing anywhere darkens.
#[test]
fn the_display_vignette_lightens_toward_white() {
    let (w, h) = (40, 28);
    let image = frame(w, h);
    for (base, source) in finishes() {
        let plain = rendered(&image, &base, source);
        let edit = Edit {
            vignette: Vignette {
                amount: 2.0,
                midpoint: 0.2,
                feather: 0.3,
                place: Place::Display,
                ..Vignette::default()
            },
            ..base.clone()
        };
        let up = rendered(&image, &edit, source);
        let (a, b) = (mean_at(&plain, w, 0, h - 1), mean_at(&up, w, 0, h - 1));
        assert!(b > a + 0.03 * 65535.0, "{source:?}: {a} to {b}");
        // No pixel's luminance darkens: the gamut op takes chroma for
        // the lightness a light color gains, so a channel may.
        let luminance = |px: &[u16]| -> f32 {
            let lin = |v: u16| encode_inverse(f32::from(v) / 65535.0);
            0.2126 * lin(px[0]) + 0.7152 * lin(px[1]) + 0.0722 * lin(px[2])
        };
        for (p, u) in plain.chunks(3).zip(up.chunks(3)) {
            assert!(
                luminance(u) >= luminance(p) - 1e-4,
                "{source:?}: {p:?} to {u:?}"
            );
        }
    }
}

/// The vignette on the display is [`Vignette::shown`] of the picture at
/// the look table, whatever the curve: the stage the shader holds to.
#[test]
fn the_display_vignette_is_shown_on_the_finished_picture() {
    for (base, source) in finishes() {
        let look = base
            .look_lut
            .look_under(&Profiled, base.display_curve)
            .filter(|l| !l.is_off());
        let global = |place| {
            let mut edit = base.clone();
            edit.vignette.place = place;
            finish::Baked::global(&edit, source)
                .with_profile_tone(crate::profile_tone::ProfileTone::of(&edit, &Profiled))
        };
        let (scene, shown) = (global(Place::Scene), global(Place::Display));
        for px in [[0.01, 0.02, 0.03], [0.18, 0.12, 0.05], [3.0, 6.0, 9.0]] {
            for stops in [-4.0, -0.5, 0.0, 0.25, 3.0] {
                let before = finish::finish_pixel_display(
                    px,
                    &scene,
                    &[],
                    0.0,
                    None,
                    None,
                    None,
                    look.as_ref(),
                );
                let after = finish::finish_pixel_display(
                    px,
                    &shown,
                    &[],
                    stops,
                    None,
                    None,
                    None,
                    look.as_ref(),
                );
                assert_eq!(after, Vignette::shown(before, stops), "{px:?} {stops}");
            }
        }
    }
}

/// Worked by hand, on a picture already rendered (no curve, a clip at
/// white) of a neutral grey, the vignette whole over it (midpoint and
/// feather 0, so every pixel off the exact middle takes all of it),
/// in sRGB at 16 bits:
/// - grey 0.5, a stop down on the display: 0.25, encoded
///   1.055 · 0.25^(1/2.4) − 0.055 = 0.53710, level 35199;
/// - grey 2.0, over white, a stop down: the clip first, so 1 then 0.5,
///   encoded 0.73535, level 48192; in the scene 2 · 0.5 = 1, white;
/// - grey 0.5, a stop up: 0.5 / (0.5 + 0.5 · 0.5) = 2/3, encoded
///   0.83601, level 54788.
#[test]
fn the_display_vignette_worked_by_hand() {
    let level = |grey: f32, amount: f32, place: Place| -> u16 {
        let (w, h) = (4, 2);
        let mut image = WorkingImage::new(w, h);
        image.data.fill(grey);
        let edit = Edit {
            vignette: Vignette {
                amount,
                midpoint: 0.0,
                feather: 0.0,
                place,
                ..Vignette::default()
            },
            ..Default::default()
        };
        let settings = Settings {
            format: Format::Tiff,
            ..Settings::default()
        };
        let r = render(
            &image,
            &edit,
            &Profiled,
            (w as u32, h as u32),
            &settings,
            &Default::default(),
            1.0,
            None,
            finish::Source::Display,
            None,
            None,
        );
        let Pixels::Sixteen(p) = r.pixels else {
            panic!("sixteen bits asked")
        };
        assert!(p.iter().all(|&v| v == p[0]), "{p:?}");
        p[0]
    };
    assert_eq!(level(0.5, -1.0, Place::Display), 35199);
    assert_eq!(level(2.0, -1.0, Place::Display), 48192);
    assert_eq!(level(2.0, -1.0, Place::Scene), 65535);
    assert_eq!(level(0.5, 1.0, Place::Display), 54788);
}

/// A place a later build writes renders as the scene.
#[test]
fn an_unknown_place_renders_as_the_scene() {
    let image = frame(24, 16);
    let json = r#"{"vignette": {"amount": -1.5, "midpoint": 0.2, "place": "lens"}}"#;
    let unknown = Edit::from_json(json).unwrap();
    assert_eq!(unknown.vignette.place, Place::Unknown);
    let mut scene = unknown.clone();
    scene.vignette.place = Place::Scene;
    let source = finish::Source::SCENE;
    assert_eq!(
        rendered(&image, &unknown, source),
        rendered(&image, &scene, source)
    );
}

/// The sRGB curve undone, for a luminance of encoded values.
fn encode_inverse(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
