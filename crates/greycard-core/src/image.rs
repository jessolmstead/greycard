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
