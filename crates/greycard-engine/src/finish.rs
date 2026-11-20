//! The one finish: what the shader and the render crate's CPU finish
//! both read off a walk, and the CPU's.

use std::sync::Arc;

use greycard_core::image::WorkingImage;
use greycard_edit::{Edit, WhiteShift};
use greycard_render::detail::DetailPlanes;
use greycard_render::export;
use greycard_render::finish::{Guide, Source};

use crate::walk::{Buf, Rasters, Walked};

/// What either finish reads beside the picture.
#[derive(Clone)]
pub struct FinishInput {
    /// The developed picture's size, before its geometry.
    pub source: (u32, u32),
    /// The full develop's size, before any bin.
    pub full: (u32, u32),
    pub guide: Option<Arc<Guide>>,
    pub planes: Option<Arc<DetailPlanes>>,
    pub rasters: Rasters,
    pub white: Option<WhiteShift>,
    pub clip_level: f32,
    pub kind: Source,
    /// The camera's frame on the picture (`stages::Facts::camera`),
    /// which frames an edit with no crop; none where the camera framed
    /// all of it, or where the caller frames the picture its own way.
    pub camera: Option<greycard_edit::geometry::Crop>,
}

impl Walked {
    /// What the finish reads of this walk.
    pub fn finish_input(&self) -> FinishInput {
        let source = match &self.buf {
            Buf::Working(image) => (image.width as u32, image.height as u32),
            Buf::Device(image) => (image.width(), image.height()),
            Buf::Texture(texture) => (texture.width(), texture.height()),
            _ => (0, 0),
        };
        FinishInput {
            source,
            full: self.meta.full,
            guide: self.taps.guide_for_finish(),
            planes: self.taps.planes.clone(),
            rasters: self.rasters.clone(),
            white: self.meta.white_shift(),
            clip_level: self.meta.clip_level,
            kind: self.meta.source,
            camera: None,
        }
    }
}

/// `image` finished on the CPU under `edit` and the sheet's
/// `settings`, as an export is: the edit's geometry, then the render
/// crate's finish, with the masks' bands taken through the same
/// geometry when they are the picture's size. No border and no mark.
/// An edit with no crop on a frame the camera framed is cut to the
/// camera's frame (`input.camera`). A frame too large to hold is
/// refused before it is cut, unless the export asks for it smaller:
/// then it is cut near the size asked ([`export::reduced_frame`]) and
/// finished as the whole frame would have been, the fit taking it the
/// rest of the way.
pub fn render(
    image: &WorkingImage,
    edit: &Edit,
    input: &FinishInput,
    settings: &export::Settings,
) -> Result<export::Rendered, greycard_render::geometry::TooLarge> {
    let (w, h) = (image.width as f32, image.height as f32);
    let edit = &Edit {
        geometry: edit.geometry.with_camera_frame(w, h, input.camera),
        ..edit.clone()
    };
    let framed;
    let whole = edit.geometry.is_identity_on(w, h, None);
    // The frame's own size and the smaller one it was cut at instead.
    let mut reduced = None;
    let picture: &WorkingImage = if whole {
        image
    } else {
        framed = match greycard_render::geometry::apply(image, &edit.geometry) {
            Ok(framed) => framed,
            Err(e) => {
                let at = export::reduced_frame(e.size, settings.long_edge, e.limit).ok_or(e)?;
                tracing::info!(
                    "a frame of {} by {} cut at {} by {} for a long edge of {:?}",
                    e.size.0,
                    e.size.1,
                    at.0,
                    at.1,
                    settings.long_edge
                );
                reduced = Some((e.size, at));
                greycard_render::geometry::apply_at(image, &edit.geometry, at)?
            }
        };
        &framed
    };
    let planes = input
        .planes
        .as_ref()
        .filter(|p| (p.width as u32, p.height as u32) == input.source);
    let detail = planes
        .map(|p| {
            let bands = p.to_image();
            match reduced {
                _ if whole => Ok(bands),
                Some((_, at)) => bands.framed_at(&edit.geometry, at),
                None => bands.framed(&edit.geometry),
            }
        })
        .transpose()?;
    // The picture stands for the frame at its own width, as one cut
    // whole stands for itself.
    let own = reduced.map_or(picture.width, |(size, _)| size.0) as f32;
    Ok(export::render_framed(
        picture,
        own,
        own,
        edit,
        &greycard_edit_io::Stores::user(),
        input.source,
        settings,
        &input.rasters,
        input.clip_level,
        input.guide.as_deref(),
        input.kind,
        input.white.as_ref(),
        detail.as_ref().zip(planes.map(|p| &p.meta)),
    ))
}

/// [`render`], then the border and the mark: the file's pixels. A mark
/// that cannot be drawn fails the finish rather than let an unmarked
/// picture out.
pub fn finish_file(
    image: &WorkingImage,
    edit: &Edit,
    input: &FinishInput,
    settings: &export::Settings,
) -> anyhow::Result<export::Rendered> {
    let mut rendered = render(image, edit, input, settings)?;
    export::border(&mut rendered, settings);
    export::mark(&mut rendered, settings)?;
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::geometry::{Aspect, Geometry};
    use greycard_edit::mask::{Component, Shape};

    /// A gradient across and down, so a picture shifted by a pixel or
    /// cut elsewhere reads differently.
    fn ramp(w: usize, h: usize) -> WorkingImage {
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let k = (y * w + x) * 3;
                image.data[k] = 0.05 + 0.4 * x as f32 / w as f32;
                image.data[k + 1] = 0.05 + 0.4 * y as f32 / h as f32;
                image.data[k + 2] = 0.2;
            }
        }
        image
    }

    fn input(w: usize, h: usize) -> FinishInput {
        FinishInput {
            source: (w as u32, h as u32),
            full: (w as u32, h as u32),
            guide: None,
            planes: None,
            rasters: Rasters::default(),
            white: None,
            clip_level: f32::INFINITY,
            kind: Source::Display,
            camera: None,
        }
    }

    /// An edit placing things by the frame: a vignette over it and a
    /// radial mask off its center.
    fn placed() -> Edit {
        let mut edit = Edit::default();
        edit.vignette.enabled = true;
        edit.vignette.amount = -0.6;
        let mut a = greycard_edit::Adjustment {
            id: 1,
            ..Default::default()
        };
        a.mask.components.push(Component {
            shape: Shape::Radial {
                center: [0.3, 0.4],
                radius: [0.2, 0.15],
                angle: 10.0,
                feather: 0.5,
            },
            ..Default::default()
        });
        a.look.light.exposure = 1.0;
        edit.adjustments.push(a);
        edit
    }

    fn pixels(r: &export::Rendered) -> &[u8] {
        match &r.pixels {
            export::Pixels::Eight(p) => p,
            export::Pixels::Sixteen(_) => panic!("eight bits asked"),
        }
    }

    /// On a frame the camera framed at 3:2 of its 4:3 sensor, an edit
    /// with no crop exports the camera's frame: the same file, to the
    /// byte, as that frame stored as the crop, the vignette and the mask
    /// placed alike. A crop set to the whole sensor exports all of it,
    /// and a frame the camera framed nothing of is as it was.
    #[test]
    fn no_crop_on_a_frame_the_camera_framed_exports_its_frame() {
        let (w, h) = (120, 90);
        let image = ramp(w, h);
        let settings = export::Settings::default();
        let band = greycard_edit::geometry::Crop {
            x: 0.0,
            y: 5.0 / 90.0,
            w: 1.0,
            h: 80.0 / 90.0,
        };
        let framed = FinishInput {
            camera: Some(band),
            ..input(w, h)
        };
        let uncropped = placed();
        let mut by_crop = uncropped.clone();
        by_crop.geometry.crop = Some(band);
        let a = render(&image, &uncropped, &framed, &settings).unwrap();
        let b = render(&image, &by_crop, &input(w, h), &settings).unwrap();
        assert_eq!((a.width, a.height), (120, 80), "3:2, not the whole 4:3");
        assert!(pixels(&a) == pixels(&b), "placed alike");
        let mut whole = uncropped.clone();
        whole.geometry.crop = Some(greycard_edit::geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        });
        let c = render(&image, &whole, &framed, &settings).unwrap();
        assert_eq!((c.width, c.height), (120, 90), "the whole sensor");
        let d = render(&image, &uncropped, &input(w, h), &settings).unwrap();
        assert_eq!((d.width, d.height), (120, 90), "no camera frame");
    }

    /// An aspect over no crop (a sidecar 0.11.0 wrote on Crop and Done
    /// under Last used) exports the frame the viewport shows: the same
    /// file, to the byte, as the crop that aspect fits, stored, with the
    /// vignette and the mask placed alike.
    #[test]
    fn an_aspect_over_no_crop_exports_as_its_fitted_crop() {
        let (w, h) = (120, 90);
        let image = ramp(w, h);
        let settings = export::Settings::default();
        let mut by_aspect = placed();
        by_aspect.geometry = Geometry {
            aspect: Aspect::Ratio { w: 3.0, h: 2.0 },
            ..Default::default()
        };
        let mut by_crop = by_aspect.clone();
        by_crop.geometry.crop = Some(by_aspect.geometry.effective_crop(w as f32, h as f32, None));
        let a = render(&image, &by_aspect, &input(w, h), &settings).unwrap();
        let b = render(&image, &by_crop, &input(w, h), &settings).unwrap();
        assert_eq!((a.width, a.height), (120, 80), "3:2, not the whole 4:3");
        assert_eq!((a.width, a.height), (b.width, b.height));
        assert!(pixels(&a) == pixels(&b), "placed alike");
    }

    /// A ratio that is the frame's own shape cuts nothing: the file is
    /// the one no geometry writes, to the byte.
    #[test]
    fn the_frames_own_ratio_exports_as_no_geometry() {
        let (w, h) = (120, 80);
        let image = ramp(w, h);
        let settings = export::Settings::default();
        let plain = placed();
        let mut own = plain.clone();
        own.geometry.aspect = Aspect::Ratio { w: 3.0, h: 2.0 };
        let a = render(&image, &plain, &input(w, h), &settings).unwrap();
        let b = render(&image, &own, &input(w, h), &settings).unwrap();
        assert_eq!((a.width, a.height), (b.width, b.height));
        assert!(pixels(&a) == pixels(&b));
    }

    /// A crop dragged out to an extreme keystone's bounds on a 12 MP
    /// picture fails the finish, and the file with it, with the frame's
    /// size and the limit, before any of the frame is allocated.
    #[test]
    fn a_frame_too_large_to_hold_fails_the_finish() {
        let (w, h) = (4320, 2880);
        let edit = Edit {
            geometry: Geometry {
                vertical: -40.0,
                horizontal: -40.0,
                angle: -30.0,
                crop: Some(greycard_edit::geometry::Crop {
                    x: -3.49,
                    y: 0.26,
                    w: 3.98,
                    h: 4.22,
                }),
                ..Default::default()
            },
            ..Edit::default()
        };
        let image = WorkingImage::new(w, h);
        let settings = export::Settings::default();
        let err = render(&image, &edit, &input(w, h), &settings)
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.limit, greycard_render::geometry::MAX_FRAME_PIXELS);
        assert!(err.size.0 * err.size.1 > err.limit, "{err}");
        let err = finish_file(&image, &edit, &input(w, h), &settings)
            .map(|_| ())
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("megapixels one may hold"),
            "{err:#}"
        );
    }

    /// The frame of [`a_frame_too_large_to_hold_fails_the_finish`]
    /// asked for small is cut near that size rather than refused: the
    /// export is the whole frame's shape at the size asked, and each
    /// of the source's marks lands where the frame's mapping puts it:
    /// lit to a pixel of its edge, black past the fit's reach. Asked
    /// for at full size it is still refused.
    #[test]
    fn a_frame_too_large_to_hold_is_finished_small() {
        let (w, h) = (4320, 2880);
        let geometry = Geometry {
            vertical: -40.0,
            horizontal: -40.0,
            angle: -30.0,
            crop: Some(greycard_edit::geometry::Crop {
                x: -3.49,
                y: 0.26,
                w: 3.98,
                h: 4.22,
            }),
            ..Default::default()
        };
        let edit = Edit {
            geometry,
            ..Edit::default()
        };
        // Black, with a white square in each corner and the middle.
        let marks = [
            (400, 300),
            (3600, 300),
            (400, 2300),
            (3600, 2300),
            (2000, 1300),
        ];
        let mark = |x: f32, y: f32| {
            marks.iter().any(|&(mx, my)| {
                (mx as f32..mx as f32 + 300.0).contains(&x)
                    && (my as f32..my as f32 + 300.0).contains(&y)
            })
        };
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                if mark(x as f32 + 0.5, y as f32 + 0.5) {
                    let k = (y * w + x) * 3;
                    image.data[k..k + 3].fill(1.0);
                }
            }
        }
        let full = export::Settings::default();
        let err = render(&image, &edit, &input(w, h), &full)
            .map(|_| ())
            .unwrap_err();
        let settings = export::Settings {
            long_edge: Some(512),
            sharpen: export::Sharpen::Off,
            ..Default::default()
        };
        let out = render(&image, &edit, &input(w, h), &settings).unwrap();
        let (fw, fh) = (err.size.0 as f64, err.size.1 as f64);
        assert_eq!(out.width, 512, "{err}");
        assert_eq!(out.height, (fh * 512.0 / fw).round() as u32, "{err}");
        // Where each pixel's center falls in the source, as the frame
        // maps it: inside a mark, or not.
        let (sw, sh) = (w as f32, h as f32);
        let frame = edit.geometry.frame(sw, sh, None);
        let scale = err.size.0 as f32 / out.width as f32;
        let inside = |x: f32, y: f32| {
            let r = (frame.origin.0 + x * scale, frame.origin.1 + y * scale);
            let s = edit.geometry.to_source(r, sw, sh);
            mark(s.0, s.1)
        };
        let p = pixels(&out);
        let (ow, oh) = (out.width as i64, out.height as i64);
        let all = |x: i64, y: i64, reach: i64, want: bool| {
            (-reach..=reach).all(|j| {
                (-reach..=reach).all(|i| inside((x + i) as f32 + 0.5, (y + j) as f32 + 0.5) == want)
            })
        };
        let (mut bright, mut dark) = (0, 0);
        for y in 0..oh {
            for x in 0..ow {
                let v = p[((y * ow + x) * 3) as usize];
                // A mark's edge may fall a pixel either way, and the
                // fit's Lanczos reaches three pixels past it.
                if all(x, y, 1, true) {
                    assert!(v > 160, "{v} at {x},{y} in a mark");
                    bright += 1;
                } else if all(x, y, 3, false) {
                    assert!(v < 24, "{v} at {x},{y} off the marks");
                    dark += 1;
                }
            }
        }
        assert!(bright > 200 && dark > 100_000, "{bright} {dark}");
        // The output sharpening runs after the fit, as ever.
        let sharpened = export::Settings {
            long_edge: Some(512),
            ..Default::default()
        };
        let s = render(&image, &edit, &input(w, h), &sharpened).unwrap();
        assert_eq!((s.width, s.height), (out.width, out.height));
        assert!(pixels(&s) != p);
    }
}
