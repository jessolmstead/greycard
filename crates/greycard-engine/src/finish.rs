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
/// refused before it is cut.
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
    let picture: &WorkingImage = if whole {
        image
    } else {
        framed = greycard_render::geometry::apply(image, &edit.geometry)?;
        &framed
    };
    let planes = input
        .planes
        .as_ref()
        .filter(|p| (p.width as u32, p.height as u32) == input.source);
    let detail = planes
        .map(|p| {
            let bands = p.to_image();
            if whole {
                Ok(bands)
            } else {
                bands.framed(&edit.geometry)
            }
        })
        .transpose()?;
    Ok(export::render(
        picture,
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
}
