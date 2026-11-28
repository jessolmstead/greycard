//! The engine's image types: linear RGB floats, in camera space or in the
//! working space.

use crate::error::{Error, Result};

/// An RGB image in linear [`crate::color::WORKING_SPACE`], interleaved,
/// row-major, one `f32` per channel. Nominal range 0..1 but nothing is
/// clipped; values above 1 are legitimate scene data.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl WorkingImage {
    pub const CHANNELS: usize = 3;

    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height * Self::CHANNELS],
        }
    }

    pub fn from_data(width: usize, height: usize, data: Vec<f32>) -> Result<Self> {
        if data.len() != width * height * Self::CHANNELS {
            return Err(Error::Unsupported(format!(
                "{width}x{height} RGB image with {} samples",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 3] {
        let i = (y * self.width + x) * Self::CHANNELS;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    pub fn pixels(&self) -> impl Iterator<Item = &[f32; 3]> {
        self.data.as_chunks::<3>().0.iter()
    }

    pub fn pixels_mut(&mut self) -> impl Iterator<Item = &mut [f32; 3]> {
        self.data.as_chunks_mut::<3>().0.iter_mut()
    }
}

/// The largest magnitude a picture's sample is let in at: a half
/// float's largest, the precision the GPU holds a picture in, so the
/// shader and the CPU read the same number. 18 stops over mid grey,
/// past any light a merge keeps.
pub const SAMPLE_LIMIT: f32 = 65504.0;

/// A picture's sample as it is let in: a NaN, which no light is, as
/// zero; an infinity, and anything past [`SAMPLE_LIMIT`], held there,
/// sign and all. A negative channel is a color outside the primaries,
/// legitimate in a linear space, and is carried. Applied where a
/// picture's samples enter: a float picture's reader, before any stage
/// that reads a neighbor (a blur, a resample), where one NaN takes its
/// neighbors with it; the GPU's upload, before the texture filter; and
/// the finish's pixel, so the CPU reads what the shader's texture
/// holds.
#[inline]
pub fn admit(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-SAMPLE_LIMIT, SAMPLE_LIMIT)
    }
}

#[cfg(test)]
mod admit_tests {
    use super::*;

    #[test]
    fn a_sample_is_let_in_finite_and_a_negative_is_carried() {
        assert_eq!(admit(f32::NAN), 0.0);
        assert_eq!(admit(-f32::NAN), 0.0);
        assert_eq!(admit(f32::INFINITY), SAMPLE_LIMIT);
        assert_eq!(admit(f32::NEG_INFINITY), -SAMPLE_LIMIT);
        assert_eq!(admit(1e6), SAMPLE_LIMIT);
        assert_eq!(admit(-0.25), -0.25);
        assert_eq!(admit(3.5), 3.5);
        // A half float's largest: (2 - 2^-10) * 2^15.
        assert_eq!(SAMPLE_LIMIT, (2.0 - 2f32.powi(-10)) * 2f32.powi(15));
    }
}

/// A demosaiced image still in camera space: levels normalized to 0..1, no
/// white balance, no matrix. Interleaved, row-major, one `f32` per channel.
///
/// This is what a linear DNG stores, and the input to everything color.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl CameraImage {
    pub const CHANNELS: usize = 3;

    pub fn from_data(width: usize, height: usize, data: Vec<f32>) -> Result<Self> {
        if data.len() != width * height * Self::CHANNELS {
            return Err(Error::Unsupported(format!(
                "{width}x{height} camera RGB image with {} samples",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 3] {
        let i = (y * self.width + x) * Self::CHANNELS;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    pub fn pixels(&self) -> impl Iterator<Item = &[f32; 3]> {
        self.data.as_chunks::<3>().0.iter()
    }
}
