//! The grey spaces: every picture as one channel under a grey profile,
//! its color, toning and grain's dyes among it, taken to its luma.

use super::*;
use crate::border::Color;
use crate::watermark::{Kind, Position};
use greycard_edit::{Adjustment, BlackWhite, DisplayCurve, Edit, Grading, Tint};
use lcms2::{Intent, PixelFormat, Profile, Transform};
use std::path::PathBuf;

const W: usize = 96;
const H: usize = 8;

/// A monochrome frame: three equal channels, from under black to two
/// stops over white, with edges for the sharpens to find.
fn plane() -> WorkingImage {
    let mut image = WorkingImage::new(W, H);
    for (i, px) in image.data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        let (x, y) = (i % W, i / W);
        let v = -0.002 + 4.0 * (x as f32 / (W - 1) as f32).powi(3);
        *px = [v * [1.0, 0.7, 1.3][(x / 5 + y / 3) % 3]; 3];
    }
    image
}

/// A photo: the plane, unevenly colored.
fn photo() -> WorkingImage {
    let mut image = plane();
    for (i, v) in image.data.iter_mut().enumerate() {
        *v *= 0.4 + ((i * 37) % 23) as f32 / 20.0;
    }
    image
}

fn settings(format: Format, space: Space) -> Settings {
    Settings {
        format,
        space,
        quality: 100,
        ..Settings::default()
    }
}

fn rendered(image: &WorkingImage, edit: &Edit, settings: &Settings) -> Rendered {
    render(
        image,
        edit,
        &greycard_edit::NoStores,
        (image.width as u32, image.height as u32),
        settings,
        &Default::default(),
        1.0,
        None,
        finish::Source::SCENE,
        None,
        None,
    )
}

/// What the finish hands the encode for `render`'s picture: three
/// channels a pixel, under the sRGB curve, unquantized.
fn encoded(image: &WorkingImage, edit: &Edit, settings: &Settings) -> Vec<[f32; 3]> {
    let (floats, _) = finish_framed(
        image,
        image.width as f32,
        image.width as f32,
        edit,
        &greycard_edit::NoStores,
        (image.width as u32, image.height as u32),
        settings,
        &Default::default(),
        1.0,
        None,
        finish::Source::SCENE,
        None,
        None,
        |px| px,
    );
    floats.as_chunks::<3>().0.to_vec()
}

/// The samples, 0 to 1.
fn samples(r: &Rendered) -> Vec<f32> {
    match &r.pixels {
        Pixels::Eight(p) => p.iter().map(|v| *v as f32 / 255.0).collect(),
        Pixels::Sixteen(p) => p.iter().map(|v| *v as f32 / 65535.0).collect(),
    }
}

/// A scratch folder of the test's own, emptied first.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("greycard-{name}-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Edits that leave a neutral picture neutral, each through different
/// arithmetic.
fn neutral_edits() -> Vec<(&'static str, Edit)> {
    let of = |name, change: &dyn Fn(&mut Edit)| {
        let mut e = Edit::default();
        change(&mut e);
        (name, e)
    };
    vec![
        of("default", &|_| {}),
        of("AgX", &|e| e.display_curve = DisplayCurve::Agx),
        of("pushed", &|e| {
            e.light.exposure = 2.5;
            e.light.tone.contrast = 0.8;
            e.light.tone.shadows = 0.7;
            e.light.tone.highlights = -0.8;
        }),
        of("pulled", &|e| e.light.exposure = -4.0),
        of("pushed AgX", &|e| {
            e.display_curve = DisplayCurve::Agx;
            e.light.exposure = 2.5;
            e.light.tone.contrast = 0.8;
        }),
        of("color", &|e| {
            e.color.saturation = 0.9;
            e.color.vibrance = 0.9;
        }),
        of("black and white", &|e| {
            e.bw = BlackWhite::with_filter(greycard_edit::bw::Filter::ALL[2], true)
        }),
        of("vignette", &|e| e.vignette.amount = -1.5),
        of("output sharpen", &|e| e.output_sharpen.amount = 2.0),
        of("a mask", &|e| {
            let mut a = Adjustment::default();
            a.mask.components.push(greycard_edit::mask::Component {
                shape: greycard_edit::mask::Shape::Linear {
                    from: [0.2, 0.0],
                    to: [0.8, 0.0],
                },
                ..Default::default()
            });
            a.look.light.exposure = 1.0;
            e.adjustments.push(a);
        }),
    ]
}

/// Edits that put color in, the least of each: a tint, a grade, grain
/// (its crystals are dyed, §33) over black and white, a photo itself.
fn toned() -> Vec<(&'static str, WorkingImage, Edit)> {
    let of = |change: &dyn Fn(&mut Edit)| {
        let mut e = Edit::default();
        change(&mut e);
        e
    };
    vec![
        ("photo", photo(), Edit::default()),
        ("tint", plane(), of(&|e| e.tint = Tint::new(40.0, 0.01))),
        (
            "grading",
            plane(),
            of(&|e| {
                e.grading = Grading::default();
                e.grading.shadows.saturation = 0.3;
            }),
        ),
        (
            "grain over black and white",
            photo(),
            of(&|e| {
                e.bw = BlackWhite::with_filter(greycard_edit::bw::Filter::ALL[2], true);
                e.grain.amount = 0.5;
            }),
        ),
    ]
}

/// A neutral frame's file is what it was when a grey space wrote one
/// channel only for a frame with no color: the merged rule's check
/// (every pixel's channels within 1/4096, encoded) passes on each, and
/// the grey sample is that version's arithmetic on the finish's floats,
/// to the bit.
#[test]
fn a_neutral_frame_is_as_before() {
    let before = |px: &[f32; 3], space: Space, levels: f32| {
        let [r, g, b] = *px;
        (space.recode(0.2126 * r + 0.7152 * g + 0.0722 * b) * levels)
            .round()
            .clamp(0.0, levels)
    };
    for (name, edit) in neutral_edits() {
        for format in [Format::Jpeg, Format::Png, Format::Tiff] {
            for space in Space::GREY {
                let s = settings(format, space);
                let floats = encoded(&plane(), &edit, &s);
                let worst = floats
                    .iter()
                    .map(|[r, g, b]| r.max(*g).max(*b) - r.min(*g).min(*b))
                    .fold(0f32, f32::max);
                assert!(worst <= 1.0 / 4096.0, "{name}: {worst:e}");
                let r = rendered(&plane(), &edit, &s);
                assert!(r.is_grey(), "{name}");
                let want: Vec<f32> = match format {
                    Format::Tiff => floats.iter().map(|p| before(p, space, 65535.0)).collect(),
                    _ => floats.iter().map(|p| before(p, space, 255.0)).collect(),
                };
                let got: Vec<f32> = match &r.pixels {
                    Pixels::Eight(p) => p.iter().map(|v| *v as f32).collect(),
                    Pixels::Sixteen(p) => p.iter().map(|v| *v as f32).collect(),
                };
                assert!(got == want, "{name}, {}", format.name());
            }
        }
    }
}

/// The CPU reference of the grey encode: a picture in color is one
/// channel in every format, each sample its pixel's luma on the
/// space's curve, rounded once; and a WebP, which has no one channel,
/// holds the luma under the sRGB curve.
#[test]
fn a_toned_picture_is_one_channel_of_its_luma() {
    for (name, image, edit) in toned() {
        for format in Format::ALL {
            for space in Space::GREY {
                let s = settings(format, space);
                let floats = encoded(&image, &edit, &s);
                // The color is there to drop: past what the merged rule
                // took for neutral, 1/4096.
                let spread = floats
                    .iter()
                    .map(|[r, g, b]| r.max(*g).max(*b) - r.min(*g).min(*b))
                    .fold(0f32, f32::max);
                assert!(spread > 1.0 / 4096.0, "{name}: {spread:e}");
                let r = rendered(&image, &edit, &s);
                assert!(r.is_grey(), "{name}, {}", format.name());
                let curve = s.file_space();
                let want = |px: &[f32; 3], levels: f32| {
                    let v = 0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2];
                    (curve.recode(v) * levels).round().clamp(0.0, levels)
                };
                let (got, want): (Vec<f32>, Vec<f32>) = match &r.pixels {
                    Pixels::Eight(p) => (
                        p.iter().map(|v| *v as f32).collect(),
                        floats.iter().map(|px| want(px, 255.0)).collect(),
                    ),
                    Pixels::Sixteen(p) => (
                        p.iter().map(|v| *v as f32).collect(),
                        floats.iter().map(|px| want(px, 65535.0)).collect(),
                    ),
                };
                assert!(got == want, "{name}, {}", format.name());
            }
        }
    }
}

/// A grey file through its embedded profile is the light of the sRGB
/// export of the same picture made grey (each pixel at its luma), to
/// the two files' rounding: for a neutral picture and for each toned
/// one.
#[test]
fn a_grey_file_is_the_light_of_the_srgb_export_made_grey() {
    let mut cases: Vec<(&str, WorkingImage, Edit)> = vec![("neutral", plane(), Edit::default())];
    cases.extend(toned());
    for (name, image, edit) in &cases {
        for format in [Format::Png, Format::Tiff] {
            let levels = if format == Format::Tiff {
                65535.0
            } else {
                255.0
            };
            let srgb = samples(&rendered(image, edit, &settings(format, Space::Srgb)));
            for space in Space::GREY {
                let grey = samples(&rendered(image, edit, &settings(format, space)));
                assert_eq!(grey.len() * 3, srgb.len());
                let profile = Profile::new_icc(&space.icc().unwrap()).unwrap();
                let to_xyz: Transform<f32, [f64; 3]> = Transform::new(
                    &profile,
                    PixelFormat::GRAY_FLT,
                    &Profile::new_xyz(),
                    PixelFormat::XYZ_DBL,
                    Intent::RelativeColorimetric,
                )
                .unwrap();
                let mut xyz = vec![[0f64; 3]; grey.len()];
                to_xyz.transform_pixels(&grey, &mut xyz);
                let mut worst = 0f64;
                for (xyz, rgb) in xyz.iter().zip(srgb.as_chunks::<3>().0) {
                    let want = finish::decode(luma(*rgb));
                    worst = worst.max((xyz[1] - want as f64).abs());
                }
                // Half a level of each, at the curves' steepest (2.3 at
                // white), and the profile's own 16-bit arithmetic.
                let bound = 4.6 * 0.5 / levels + 6e-5;
                eprintln!(
                    "{name}, {} {}: {worst:e} of {bound:e}",
                    space.name(),
                    format.name()
                );
                assert!(worst < bound, "{name}, {}: {worst:e}", space.name());
            }
        }
    }
}

#[test]
fn a_grey_space_by_its_name_and_its_profile() {
    for space in Space::GREY {
        assert_eq!(Space::from_name(space.name()), Some(space));
        assert_eq!(space.in_color(), Space::Srgb);
        assert_eq!(space.matrix(), Space::Srgb.matrix());
        assert_eq!(
            space.finish_icc().unwrap(),
            Space::Srgb.finish_icc().unwrap()
        );
        let icc = space.icc().unwrap();
        assert_eq!(&icc[8..10], &[2, 0x10], "version 2.1");
        assert_eq!(&icc[12..24], b"mntrGRAYXYZ ");
        assert_eq!(icc, space.icc().unwrap());
        // The ends are the ends, and the curve only rises.
        let curve: Vec<f32> = (0..=1000)
            .map(|i| space.recode(i as f32 / 1000.0))
            .collect();
        assert!(curve[0].abs() < 1e-6 && (curve[1000] - 1.0).abs() < 1e-6);
        assert!(curve.windows(2).all(|w| w[0] < w[1]), "{}", space.name());
    }
    // The profile names itself as a stand-in, as Adobe RGB's does.
    let named = |icc: &[u8], words: &[u8]| icc.windows(words.len()).any(|w| w == words);
    let grey = Space::GreyGamma22.icc().unwrap();
    assert!(named(&grey, b"Gray Gamma 2.2 (compatible)"));
    assert!(named(
        &Space::AdobeRgb.icc().unwrap(),
        b"Adobe RGB (compatible)"
    ));
    for space in Space::ALL {
        assert_eq!(space.in_color(), space);
        assert!(!space.is_grey());
    }
}

/// A space no longer offered, or never known, is no space: whoever reads
/// the name takes their default, as for any name from another version.
#[test]
fn a_removed_space_is_no_space() {
    for name in ["Dot Gain 20%", "Gray Gamma 1.8", ""] {
        assert_eq!(Space::from_name(name), None, "{name}");
    }
}

/// Each format's file is one channel under the space's profile, with the
/// source's tags and the space's name in the XMP; a WebP, with no grey
/// form, three equal channels under sRGB's. A picture of the wrong
/// channels for the settings is refused.
#[test]
fn a_grey_file_is_one_channel_under_its_profile() {
    let metadata = RawMetadata {
        exif: Default::default(),
        model: "EOS R6m2".into(),
        make: "Canon".into(),
        lens: None,
        unique_image_id: None,
        rating: None,
    };
    let dir = scratch("grey-files");
    let carries = |bytes: &[u8], what: &[u8]| bytes.windows(what.len()).any(|w| w == what);
    for space in Space::GREY {
        for format in Format::ALL {
            let mut settings = settings(format, space);
            let r = rendered(&photo(), &Edit::default(), &settings);
            let path = dir.join(format!("g.{}", format.extension()));
            let origin = Origin::default();
            write(&r, &settings, &path, Some(&metadata), &origin).unwrap();
            let what = format!("{} {}", space.name(), format.name());
            let wrote = settings.file_space();
            assert_eq!(wrote == Space::Srgb, format == Format::WebP, "{what}");
            let bytes = std::fs::read(&path).unwrap();
            assert!(carries(&bytes, b"EOS R6m2"), "{what}");
            // The XMP says what the sheet asked for, a WebP's too.
            let words = settings.describe();
            assert!(words.ends_with(space.name()), "{words}");
            assert!(carries(&bytes, words.as_bytes()), "{what}");
            let back = image::open(&path).unwrap();
            let color = match format {
                Format::Tiff => image::ColorType::L16,
                Format::WebP => image::ColorType::Rgb8,
                _ => image::ColorType::L8,
            };
            assert_eq!(back.color(), color, "{what}");
            match (&r.pixels, format) {
                (Pixels::Eight(p), Format::Png) => assert_eq!(back.as_bytes(), p, "{what}"),
                (Pixels::Eight(p), Format::WebP) => {
                    let rgb = back.as_bytes().as_chunks::<3>().0;
                    assert!(rgb.iter().zip(p).all(|(c, v)| *c == [*v; 3]), "{what}");
                }
                _ => {}
            }
            // The image crate reads no profile out of a TIFF this small.
            let icc = wrote.icc().unwrap();
            let embedded = |path: &Path, bytes: &[u8]| match format {
                Format::Tiff => carries(bytes, &icc).then(|| icc.clone()),
                _ => {
                    let mut d = image::ImageReader::open(path)
                        .unwrap()
                        .into_decoder()
                        .unwrap();
                    image::ImageDecoder::icc_profile(&mut d).unwrap()
                }
            };
            assert_eq!(embedded(&path, &bytes), Some(icc.clone()), "{what}");
            settings.embed_profile = false;
            write(&r, &settings, &path, None, &origin).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(embedded(&path, &bytes), None, "{what}");
            // Three channels under a grey space, or one under a color
            // space, are no file.
            let color = rendered(
                &photo(),
                &Edit::default(),
                &self::settings(format, Space::Srgb),
            );
            assert!(write(&color, &settings, &path, None, &origin).is_err());
            let srgb = self::settings(format, Space::Srgb);
            assert!(write(&r, &srgb, &path, None, &origin).is_err());
        }
    }
    crate::testing::remove_dir_retry(&dir);
}

/// A border's levels stand where they do in the other spaces, and a
/// mark is laid on as its luma in the file's curve, a red one too;
/// the WebP's three channels stay equal under a red mark.
#[test]
fn the_border_and_the_mark_on_a_grey_picture() {
    let dir = scratch("grey-mark");
    let png = |name: &str, color: [u8; 4]| {
        let path = dir.join(name);
        image::RgbaImage::from_pixel(8, 8, image::Rgba(color))
            .save(&path)
            .unwrap();
        Some(Mark {
            kind: Kind::Image { path },
            position: Position::Center,
            size: 0.05,
            margin: 0.0,
            opacity: 1.0,
        })
    };
    let neutral = png("neutral.png", [128, 128, 128, 255]);
    let red = png("red.png", [255, 0, 0, 255]);
    let edit = Edit::default();
    let finished = |settings: &Settings| {
        let mut r = rendered(&photo(), &edit, settings);
        border(&mut r, settings);
        mark(&mut r, settings).unwrap();
        r
    };
    for space in Space::GREY {
        for color in Color::ALL {
            let mut s = settings(Format::Png, space);
            s.border = Some(Border { width: 0.1, color });
            s.watermark = neutral.clone();
            let r = finished(&s);
            assert!(r.is_grey());
            assert_eq!((r.width, r.height), (W as u32 + 20, H as u32 + 20));
            let px = samples(&r);
            let (cx, cy) = (r.width as usize / 2, r.height as usize / 2);
            let at = |v: f32| (space.recode(v) * 255.0).round() / 255.0;
            assert_eq!(px[cy * r.width as usize + cx], at(128.0 / 255.0));
            match color {
                Color::White => assert_eq!(px[0], 1.0),
                Color::LightGrey => assert_eq!(px[0], at(0.8)),
                Color::DarkGrey => assert_eq!(px[0], at(0.2)),
                Color::Black => assert_eq!(px[0], 0.0),
                Color::Match => {
                    let bare = samples(&rendered(&photo(), &edit, &settings(Format::Png, space)));
                    let mean = bare.iter().sum::<f32>() / bare.len() as f32;
                    assert!((px[0] - mean).abs() <= 0.51 / 255.0, "{} {mean}", px[0]);
                }
            }
        }
        for format in Format::ALL {
            let mut s = settings(format, space);
            s.watermark = red.clone();
            s.border = Some(Border {
                width: 0.1,
                color: Color::DarkGrey,
            });
            let r = finished(&s);
            assert!(r.is_grey(), "{}", format.name());
            let px = samples(&r);
            let (cx, cy) = (r.width as usize / 2, r.height as usize / 2);
            let levels = if format == Format::Tiff {
                65535.0
            } else {
                255.0
            };
            let want = (s.file_space().recode(luma([1.0, 0.0, 0.0])) * levels).round() / levels;
            assert_eq!(px[cy * r.width as usize + cx], want, "{}", format.name());
            let path = dir.join(format!("m.{}", format.extension()));
            write(&r, &s, &path, None, &Origin::default()).unwrap();
            if format == Format::WebP {
                let back = image::open(&path).unwrap().into_rgb8();
                assert!(back.pixels().all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2]));
            }
        }
    }
    crate::testing::remove_dir_retry(&dir);
}
