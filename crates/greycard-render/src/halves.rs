//! A developed picture as the GPU takes it, for the viewport to
//! upload: the engine makes it, the viewport draws it.

use greycard_core::image::WorkingImage;

/// A developed image as the GPU takes it: RGBA half floats, alpha one.
pub struct Halves {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<half::f16>,
}

impl Halves {
    /// The picture, with `mask` in the alpha when there is one: the
    /// sharpen's blend, for the viewport to paint where it acted.
    /// Zero alpha otherwise, so nothing is painted.
    pub fn from_image(image: &WorkingImage, mask: Option<&[f32]>) -> Self {
        use rayon::prelude::*;
        // A row at a time, in parallel: a 24 MP frame is a hundred
        // million conversions.
        let (w, h) = (image.width, image.height);
        let mut pixels = vec![half::f16::ZERO; w * h * 4];
        pixels
            .par_chunks_mut(w * 4)
            .zip(image.data.par_chunks(w * 3))
            .enumerate()
            .for_each(|(y, (out, row))| {
                let mask = mask.map(|m| &m[y * w..(y + 1) * w]);
                let (out, _) = out.as_chunks_mut::<4>();
                let (row, _) = row.as_chunks::<3>();
                for (x, (o, px)) in out.iter_mut().zip(row).enumerate() {
                    // Let in finite (`image::admit`): a half float takes
                    // anything past its largest to an infinity, and the
                    // texture filter spreads a NaN or an infinity to the
                    // pixels beside it.
                    let px = px.map(greycard_core::image::admit);
                    o[0] = half::f16::from_f32(px[0]);
                    o[1] = half::f16::from_f32(px[1]);
                    o[2] = half::f16::from_f32(px[2]);
                    o[3] = half::f16::from_f32(mask.map_or(0.0, |m| m[x]));
                }
            });
        Self {
            width: w as u32,
            height: h as u32,
            pixels,
        }
    }
}
