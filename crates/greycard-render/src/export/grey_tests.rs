//! The grey spaces: a picture with no color as one channel under a grey
//! profile, and one in color as its sRGB export.

use super::*;
use crate::border::Color;
use crate::watermark::{Kind, Position};
use greycard_edit::{Adjustment, BlackWhite, DisplayCurve, Edit, Grading, Tint};
use lcms2::{Intent, PixelFormat, Profile, Transform};

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

/// The samples, 0 to 1.
fn samples(r: &Rendered) -> Vec<f32> {
    match &r.pixels {
        Pixels::Eight(p) => p.iter().map(|v| *v as f32 / 255.0).collect(),
        Pixels::Sixteen(p) => p.iter().map(|v| *v as f32 / 65535.0).collect(),
    }
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

/// What the finish rounds off leaves a neutral picture neutral, by a
/// wide margin, under each edit that puts no color in: its channels'
/// spread in the sRGB export, 16-bit, against [`NEUTRAL`].
#[test]
fn a_neutral_picture_is_one_channel_whatever_rounds_off() {
    for (name, edit) in neutral_edits() {
        let srgb = rendered(&plane(), &edit, &settings(Format::Tiff, Space::Srgb));
        let worst = samples(&srgb)
            .as_chunks::<3>()
            .0
            .iter()
            .map(|[r, g, b]| r.max(*g).max(*b) - r.min(*g).min(*b))
            .fold(0f32, f32::max);
        eprintln!("{name}: channels apart by {worst:e}");
        assert!(worst <= NEUTRAL / 4.0, "{name}: {worst:e}");
        for format in [Format::Jpeg, Format::Png, Format::Tiff] {
            for space in Space::GREY {
                let r = rendered(&plane(), &edit, &settings(format, space));
                assert!(r.is_grey(), "{name}, {}", space.name());
            }
        }
    }
    // A photo made black and white is neutral too.
    let edit = Edit {
        bw: BlackWhite::with_filter(greycard_edit::bw::Filter::ALL[2], true),
        ..Default::default()
    };
    let grey = settings(Format::Tiff, Space::GreyGamma22);
    assert!(rendered(&photo(), &edit, &grey).is_grey());
}

/// The least the sliders tone, a photo, and the grain's dyes are color:
/// the file is the sRGB export's, byte for byte, and its words say so.
#[test]
fn a_picture_in_color_takes_srgb() {
    let toned = |change: &dyn Fn(&mut Edit)| {
        let mut e = Edit::default();
        change(&mut e);
        e
    };
    let cases = [
        ("photo", photo(), Edit::default()),
        ("tint", plane(), toned(&|e| e.tint = Tint::new(40.0, 0.01))),
        (
            "grading",
            plane(),
            toned(&|e| {
                e.grading = Grading::default();
                e.grading.shadows.saturation = 0.01;
            }),
        ),
        ("grain", plane(), toned(&|e| e.grain.amount = 0.1)),
    ];
    let dir = std::env::temp_dir().join(format!("greycard-grey-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, image, edit) in &cases {
        for format in Format::ALL {
            let file = |space: Space| {
                let settings = settings(format, space);
                let r = rendered(image, edit, &settings);
                assert!(!r.is_grey(), "{name}");
                let path = dir.join(format!("c.{}", format.extension()));
                write(&r, &settings, &path, None, &Origin::default()).unwrap();
                std::fs::read(&path).unwrap()
            };
            let srgb = file(Space::Srgb);
            for space in Space::GREY {
                assert!(file(space) == srgb, "{name}, {}", format.name());
            }
        }
    }
    crate::testing::remove_dir_retry(&dir);
}

/// A grey value through the embedded profile is the light the sRGB
/// export's is, to the two files' rounding.
#[test]
fn a_grey_value_is_the_rgb_exports_luminance() {
    let edit = Edit::default();
    for format in [Format::Png, Format::Tiff] {
        let levels = if format == Format::Tiff {
            65535.0
        } else {
            255.0
        };
        let srgb = samples(&rendered(&plane(), &edit, &settings(format, Space::Srgb)));
        for space in Space::GREY {
            let grey = samples(&rendered(&plane(), &edit, &settings(format, space)));
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
                let want = rgb.map(finish::decode);
                let want = 0.2126 * want[0] + 0.7152 * want[1] + 0.0722 * want[2];
                worst = worst.max((xyz[1] - want as f64).abs());
            }
            // Half a level of each, at the curves' steepest (2.3 at
            // white), and the profile's own table and 16-bit arithmetic.
            let bound = 4.6 * 0.5 / levels + 6e-5;
            eprintln!("{} {}: {worst:e} of {bound:e}", space.name(), format.name());
            assert!(worst < bound, "{}: {worst:e}", space.name());
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
        assert_eq!(&icc[16..24], b"GRAYXYZ ");
        assert_eq!(icc, space.icc().unwrap());
        // The ends are the ends, and the curve only rises.
        let curve: Vec<f32> = (0..=1000)
            .map(|i| space.recode(i as f32 / 1000.0))
            .collect();
        assert!(curve[0].abs() < 1e-6 && (curve[1000] - 1.0).abs() < 1e-6);
        assert!(curve.windows(2).all(|w| w[0] < w[1]), "{}", space.name());
    }
    for space in Space::ALL {
        assert_eq!(space.in_color(), space);
    }
    // Half an ink dot prints as 70%.
    assert!((dot_gain_decode(0.5) - 0.3).abs() < 1e-12);
    assert!((dot_gain_encode(0.3) - 0.5).abs() < 1e-6);
}

/// Each format's file: one channel under the space's profile, with the
/// source's tags and the space's name in the XMP; a WebP, with no grey
/// form, sRGB's.
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
    let dir = std::env::temp_dir().join(format!("greycard-grey-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let carries = |bytes: &[u8], what: &[u8]| bytes.windows(what.len()).any(|w| w == what);
    for space in Space::GREY {
        for format in Format::ALL {
            let mut settings = settings(format, space);
            let r = rendered(&plane(), &Edit::default(), &settings);
            let path = dir.join(format!("g.{}", format.extension()));
            let origin = Origin::default();
            write(&r, &settings, &path, Some(&metadata), &origin).unwrap();
            let what = format!("{} {}", space.name(), format.name());
            let wrote = if format == Format::WebP {
                Space::Srgb
            } else {
                space
            };
            let bytes = std::fs::read(&path).unwrap();
            assert!(carries(&bytes, b"EOS R6m2"), "{what}");
            let words = Settings {
                space: wrote,
                ..settings.clone()
            }
            .describe();
            assert!(carries(&bytes, words.as_bytes()), "{what}");
            let back = image::open(&path).unwrap();
            let color = match format {
                Format::Tiff => image::ColorType::L16,
                Format::WebP => image::ColorType::Rgb8,
                _ => image::ColorType::L8,
            };
            assert_eq!(back.color(), color, "{what}");
            if let (Pixels::Eight(p), Format::Png) = (&r.pixels, format) {
                assert_eq!(back.as_bytes(), p, "{what}");
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
        }
    }
    crate::testing::remove_dir_retry(&dir);
}

/// A border's levels stand where they do in the other spaces, a neutral
/// mark is laid on in the space's curve, and a mark in color makes the
/// file sRGB's.
#[test]
fn the_border_and_the_mark_on_a_grey_picture() {
    let dir = std::env::temp_dir().join(format!("greycard-grey-mark-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
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
    // Color under no alpha is no color.
    let clear = png("clear.png", [255, 0, 0, 0]);
    let red = png("red.png", [255, 0, 0, 255]);
    let edit = Edit::default();
    let finished = |settings: &Settings| {
        let mut r = rendered(&plane(), &edit, settings);
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
                    let bare = samples(&rendered(&plane(), &edit, &settings(Format::Png, space)));
                    let mean = bare.iter().sum::<f32>() / bare.len() as f32;
                    assert!((px[0] - mean).abs() <= 0.51 / 255.0, "{} {mean}", px[0]);
                }
            }
        }
        let mut s = settings(Format::Tiff, space);
        s.watermark = clear.clone();
        assert!(finished(&s).is_grey());
        s.watermark = red.clone();
        s.border = Some(Border {
            width: 0.1,
            color: Color::DarkGrey,
        });
        let srgb = Settings {
            space: Space::Srgb,
            ..s.clone()
        };
        assert!(samples(&finished(&s)) == samples(&finished(&srgb)));
    }
    crate::testing::remove_dir_retry(&dir);
}
