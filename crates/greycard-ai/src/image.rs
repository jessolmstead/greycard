//! The images a model sees and the masks it returns: small, sRGB,
//! resampled to the model's square.

/// An 8-bit sRGB image, rows top to bottom, three bytes a pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgb8 {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Rgb8 {
    pub fn new(width: usize, height: usize, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), width * height * 3, "an Rgb8 is 3 bytes a pixel");
        Self {
            width,
            height,
            data,
        }
    }

    /// The image stretched to `size`×`size`, as the NCHW float tensor
    /// the ImageNet-normalized models take: `(v / 255 - mean) / std`
    /// a channel, channels planar.
    pub fn to_planes(&self, size: usize, mean: [f32; 3], std: [f32; 3]) -> Vec<f32> {
        let mut out = vec![0.0f32; 3 * size * size];
        let sx = self.width as f32 / size as f32;
        let sy = self.height as f32 / size as f32;
        for y in 0..size {
            let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
            for x in 0..size {
                let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                let px = self.bilinear(fx, fy);
                for c in 0..3 {
                    out[c * size * size + y * size + x] = (px[c] / 255.0 - mean[c]) / std[c];
                }
            }
        }
        out
    }

    fn bilinear(&self, fx: f32, fy: f32) -> [f32; 3] {
        let x0 = (fx as usize).min(self.width - 1);
        let y0 = (fy as usize).min(self.height - 1);
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let at = |x: usize, y: usize| {
            let i = (y * self.width + x) * 3;
            [
                self.data[i] as f32,
                self.data[i + 1] as f32,
                self.data[i + 2] as f32,
            ]
        };
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    }
}

/// A float RGB image, three values a pixel, rows top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct Rgbf {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Rgbf {
    pub fn new(width: usize, height: usize, data: Vec<f32>) -> Self {
        assert_eq!(
            data.len(),
            width * height * 3,
            "an Rgbf is 3 values a pixel"
        );
        Self {
            width,
            height,
            data,
        }
    }

    pub fn at(&self, x: usize, y: usize) -> [f32; 3] {
        let i = (y * self.width + x) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// Bilinearly resampled to `width`×`height`.
    pub fn resampled(&self, width: usize, height: usize) -> Self {
        let sx = self.width as f32 / width as f32;
        let sy = self.height as f32 / height as f32;
        let mut data = Vec::with_capacity(width * height * 3);
        for y in 0..height {
            let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
            let y0 = (fy as usize).min(self.height - 1);
            let y1 = (y0 + 1).min(self.height - 1);
            let ty = fy - y0 as f32;
            for x in 0..width {
                let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                let x0 = (fx as usize).min(self.width - 1);
                let x1 = (x0 + 1).min(self.width - 1);
                let tx = fx - x0 as f32;
                let (a, b, c, d) = (
                    self.at(x0, y0),
                    self.at(x1, y0),
                    self.at(x0, y1),
                    self.at(x1, y1),
                );
                for k in 0..3 {
                    let top = a[k] + (b[k] - a[k]) * tx;
                    let bottom = c[k] + (d[k] - c[k]) * tx;
                    data.push(top + (bottom - top) * ty);
                }
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    /// As planar NCHW floats, channels one after another.
    pub fn to_planes(&self) -> Vec<f32> {
        let n = self.width * self.height;
        let mut out = vec![0.0f32; 3 * n];
        for (i, px) in self.data.as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                out[c * n + i] = px[c];
            }
        }
        out
    }

    /// From planar NCHW floats.
    pub fn from_planes(width: usize, height: usize, planes: &[f32]) -> Self {
        let n = width * height;
        assert_eq!(planes.len(), 3 * n);
        let mut data = Vec::with_capacity(3 * n);
        for i in 0..n {
            data.extend([planes[i], planes[n + i], planes[2 * n + i]]);
        }
        Self {
            width,
            height,
            data,
        }
    }
}

/// The mean and standard deviation the ImageNet-trained models expect.
pub const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// A mask in [0, 1], rows top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Mask {
    pub fn new(width: usize, height: usize, data: Vec<f32>) -> Self {
        assert_eq!(data.len(), width * height, "a Mask is one value a pixel");
        Self {
            width,
            height,
            data,
        }
    }

    /// From logits: the sigmoid of each.
    pub fn from_logits(width: usize, height: usize, logits: &[f32]) -> Self {
        Self::new(
            width,
            height,
            logits.iter().map(|&l| 1.0 / (1.0 + (-l).exp())).collect(),
        )
    }

    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Bilinearly resampled to `width`×`height`.
    pub fn resampled(&self, width: usize, height: usize) -> Self {
        let sx = self.width as f32 / width as f32;
        let sy = self.height as f32 / height as f32;
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
            let y0 = (fy as usize).min(self.height - 1);
            let y1 = (y0 + 1).min(self.height - 1);
            let ty = fy - y0 as f32;
            for x in 0..width {
                let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                let x0 = (fx as usize).min(self.width - 1);
                let x1 = (x0 + 1).min(self.width - 1);
                let tx = fx - x0 as f32;
                let top = self.at(x0, y0) + (self.at(x1, y0) - self.at(x0, y0)) * tx;
                let bottom = self.at(x0, y1) + (self.at(x1, y1) - self.at(x0, y1)) * tx;
                data.push(top + (bottom - top) * ty);
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    /// As bytes, 0 to 255.
    pub fn to_u8(&self) -> Vec<u8> {
        self.data
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planes_are_normalized_per_channel() {
        let img = Rgb8::new(2, 2, vec![255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0]);
        let planes = img.to_planes(2, IMAGENET_MEAN, IMAGENET_STD);
        assert_eq!(planes.len(), 12);
        let r = (1.0 - IMAGENET_MEAN[0]) / IMAGENET_STD[0];
        let g = (0.0 - IMAGENET_MEAN[1]) / IMAGENET_STD[1];
        assert!((planes[0] - r).abs() < 1e-6);
        assert!((planes[4] - g).abs() < 1e-6);
    }

    #[test]
    fn a_flat_image_resamples_to_itself() {
        let img = Rgb8::new(3, 5, vec![7; 45]);
        let planes = img.to_planes(8, [0.0; 3], [1.0; 3]);
        assert!(planes.iter().all(|&v| (v - 7.0 / 255.0).abs() < 1e-6));
    }

    #[test]
    fn a_mask_resamples_to_the_same_ramp() {
        let m = Mask::new(4, 1, vec![0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);
        let r = m.resampled(7, 2);
        assert_eq!(r.at(0, 0), 0.0);
        assert!((r.at(6, 1) - 1.0).abs() < 1e-6);
        let mut prev = -1.0;
        for x in 0..7 {
            assert!(r.at(x, 0) >= prev);
            prev = r.at(x, 0);
        }
    }

    #[test]
    fn planes_round_trip_and_a_flat_rgbf_resamples_to_itself() {
        let img = Rgbf::new(2, 1, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let planes = img.to_planes();
        assert_eq!(planes, vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
        assert_eq!(Rgbf::from_planes(2, 1, &planes), img);
        let flat = Rgbf::new(3, 3, vec![0.25; 27]);
        assert!(
            flat.resampled(7, 5)
                .data
                .iter()
                .all(|v| (v - 0.25).abs() < 1e-6)
        );
    }

    #[test]
    fn logits_become_probabilities() {
        let m = Mask::from_logits(3, 1, &[-20.0, 0.0, 20.0]);
        assert_eq!(m.to_u8(), vec![0, 128, 255]);
    }
}
