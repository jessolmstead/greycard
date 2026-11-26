//! What the editor holds of the machine's memory, and what is free of
//! it: the account the shelf of frames left is sized from.
//!
//! A frame's cost is reckoned from its developed size with per-pixel
//! figures ([`Cost`], [`COST`]), split by where it is held: in memory,
//! or on the device. What the device holds counts against the machine's
//! memory only where the GPU is on it ([`Gpu::Unified`]); a discrete
//! card's is its own ([`Held::charged`]).

use crate::stages::{Facts, InputKind};

/// Where the engine's GPU ops keep their pictures, which says whether
/// what is held on the device is held in the machine's memory as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpu {
    /// No GPU ops: the develop on the CPU.
    None,
    /// A GPU on the machine's memory: Apple silicon, an integrated GPU,
    /// or a software one (lavapipe), whose textures are the process's.
    Unified,
    /// A card with memory of its own.
    Discrete,
}

impl Gpu {
    /// The kind of the device `gpu` runs on, none without one: discrete
    /// only for an adapter wgpu calls a discrete GPU, off Apple silicon;
    /// unified for any other (integrated, a CPU adapter such as
    /// lavapipe, a virtual one, or of no known type), so an adapter we
    /// cannot place has its pictures counted against the machine's
    /// memory. greycard-ai's `runtime::webgpu_unified` reads an adapter
    /// otherwise (a virtual GPU discrete, a CPU or unknown one neither),
    /// for its own purpose: which tile the denoiser takes.
    pub fn of(gpu: Option<&greycard_gpu::Context>) -> Self {
        let Some(gpu) = gpu else {
            return Self::None;
        };
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return Self::Unified;
        }
        match gpu.device().adapter_info().device_type {
            greycard_gpu::wgpu::DeviceType::DiscreteGpu => Self::Discrete,
            _ => Self::Unified,
        }
    }

    /// For the log.
    pub fn said(self) -> &'static str {
        match self {
            Self::None => "on the CPU",
            Self::Unified => "on unified memory",
            Self::Discrete => "on a card of its own",
        }
    }
}

/// Bytes held, in the machine's memory and on the device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub memory: u64,
    pub device: u64,
}

impl Held {
    pub const NONE: Self = Self {
        memory: 0,
        device: 0,
    };

    /// What this costs the machine's memory with the GPU `gpu`: the
    /// device's share only on unified memory.
    pub fn charged(self, gpu: Gpu) -> u64 {
        match gpu {
            Gpu::Unified => self.memory.saturating_add(self.device),
            Gpu::None | Gpu::Discrete => self.memory,
        }
    }
}

/// What a frame costs, in bytes a pixel of its developed size: what the
/// open frame holds at rest, what making a base adds while it is made
/// (an open's, or a prefetch's beside the open frame), the sharpen's
/// and the local contrast's working sets, and what an edit's sections
/// and masks keep beside the picture, on the GPU path and on the CPU's.
///
/// Measured on lavapipe, whose device memory is the process's, by
/// `worker::memory_tests::what_an_open_frame_holds`: a Bayer DNG of
/// ours at 24 MP, the device's allocations by label
/// (`Device::generate_allocator_report`) and the process's resident
/// set; and on the owner's Mac (24 GB unified, 102 MP, Metal,
/// 2026-11-26). On unified memory only the total counts, so the larger
/// of the two totals is charged; on a discrete card only the memory's
/// share counts, and the larger of the two memory shares is charged.
///
/// The open frame on the GPU path, at rest, under the default edit:
///
/// | held, by its allocation                                        | where  | B/px |
/// |----------------------------------------------------------------|--------|------|
/// | "developed, full floats": the base uploaded (Rgba32Float, `Context::upload`) | device | 16 |
/// | "developed": the picture drawn (Rgba16Float, `Context::viewport_texture`) | device | 8 |
/// | the base (`Keep::Base`, three f32), the decoded mosaic (u16), the guide plane, the rest | memory | 20.6 |
///
/// 44.6 in all on lavapipe; 57 on the Mac (its 6.6 GB on the device
/// less the sharpen's 1.8 GB set, then kept, and 1 GB in memory, at
/// 102 MP): 57 charged on unified memory, 23 (lavapipe's 20.6, over
/// the Mac's 10) on a discrete card.
///
/// A base in the making, at its peak beside the open frame, over the
/// open frame and the base made: on lavapipe "demosaic detail" 12,
/// "demosaic read back" 8, "demosaic rgb" 4.2, "demosaic mosaic", "a"
/// to "d", and wgpu's staging 1.4 each on the device, 32.6, and the
/// decode and the prepared mosaic in memory, 6.2: 38.8. On the Mac 3 GB
/// in memory and 2.7 GB on the device less the base's 12: 45. Charged
/// 45 on unified memory, 18 (the Mac's) on a discrete card.
///
/// On the CPU (lavapipe, no device lent): the open frame 23.6 at rest
/// (the base and the developed picture, three f32 each, the viewport's
/// halves), and a base in the making 32.6 over it. Without a device the
/// window keeps no picture before the sharpen (`window_steps`), so
/// Clarity and Dehaze, which make the developed picture its own, keep
/// nothing the figure does not count already.
///
/// What comes and goes beside the open frame, made for an adjustment
/// and let go at rest off a discrete card:
///
/// - the sharpen's working set (`sharpen::Work`, §753): "sharpen
///   luminance", "L*", "blend", and "scratch", four R32Float planes the
///   picture's size, 16 a pixel; "sharpen estimate" and "ratio", two
///   R32Float atlases within `ATLAS_BUDGET`'s 96 MiB each; 1.8 GB at
///   102 MP. On a discrete card it is kept, on the card.
/// - the local contrast's planes (`local_contrast::Work`), made for
///   each run off a discrete card: "local contrast log" and "scratch"
///   (R32Float, 8), "a" and "b" for Texture or an exact filter (8),
///   Texture's "gain" (4), and Structure's "top" (4): 24 at most, the
///   grids' a fraction of one. A run of the local contrast inside an
///   adjustment's develop holds them beside the sharpen's set, which the
///   adjustment keeps: the two are counted together.
///
/// What an edit keeps beside the picture at rest, when it asks:
///
/// | kept, by its allocation                                       | where  | B/px |
/// |---------------------------------------------------------------|--------|------|
/// | Clarity, Texture, or Structure: "after the local contrast, full floats" (Rgba32Float, the picture before the sharpen) | device | 16 |
/// | Dehaze: "after the dehaze, full floats" (Rgba32Float); with the local contrast both are held (measured 32) | device | 16 |
/// | a mask's Detail: `DetailPlanes::data` (four f16), and the viewport's "detail planes" (Rgba16Float) | both | 8 + 8 |
/// | a mask's Sharpen: `DetailPlanes::sharpen` (f16), and the viewport's "sharpen delta" (R16Float) | both | 2 + 2 |
/// | a range mask under the Detail section: `DetailPlanes::sample` (three f32), and the viewport's "range sample" (Rgba16Float) | both | 12 + 8 |
/// | a refined mask: the refined masks' picture (`MaskPicture`, three f16) | memory | 6 |
///
/// A mask's own raster ("brushes", R8Unorm, and its strokes in memory)
/// is at most 4096 wide whatever the frame, and is not counted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cost {
    /// The open frame on the GPU path at rest, in all, on unified
    /// memory; and its memory's share, on a discrete card.
    pub held_unified: u64,
    pub held_memory: u64,
    /// A base being made on the GPU path, over what is there before
    /// it: in all, and the memory's share.
    pub making_unified: u64,
    pub making_memory: u64,
    /// The open frame on the CPU path at rest, and a base being made
    /// there, all in memory.
    pub held_cpu: u64,
    pub making_cpu: u64,
    /// The sharpen's working set on the device: so much a pixel, and
    /// its atlases' bytes whatever the size.
    pub sharpen: (u64, u64),
    /// The local contrast's planes for a run, at most.
    pub local_contrast: u64,
    /// The picture before the sharpen kept on the device after the
    /// local contrast, and after the dehaze.
    pub after_local_contrast: u64,
    pub after_dehaze: u64,
    /// A mask's Detail bands, its sharpen's delta, the range masks'
    /// sample, and the refined masks' picture.
    pub bands: Held,
    pub delta: Held,
    pub sample: Held,
    pub refined: Held,
}

/// The figures of [`Cost`].
pub const COST: Cost = Cost {
    held_unified: 57,
    held_memory: 23,
    making_unified: 45,
    making_memory: 18,
    held_cpu: 24,
    making_cpu: 33,
    sharpen: (16, 2 * (96 << 20)),
    local_contrast: 24,
    after_local_contrast: 16,
    after_dehaze: 16,
    bands: Held {
        memory: 8,
        device: 8,
    },
    delta: Held {
        memory: 2,
        device: 2,
    },
    sample: Held {
        memory: 12,
        device: 8,
    },
    refined: Held {
        memory: 6,
        device: 0,
    },
};

/// What of an edit changes what the open frame holds: the Detail
/// section's local contrast and dehaze, and what its masks read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Uses {
    pub local_contrast: bool,
    pub dehaze: bool,
    pub bands: bool,
    pub delta: bool,
    pub sample: bool,
    pub refined: bool,
}

impl Uses {
    /// The default edit's: nothing beside the picture.
    pub const NONE: Self = Self {
        local_contrast: false,
        dehaze: false,
        bands: false,
        delta: false,
        sample: false,
        refined: false,
    };

    /// What `edit` asks for, as the window's walk keeps it
    /// (`stages::stages`): a few reads of the edit, cheap to ask at
    /// every sizing.
    pub fn of(edit: &greycard_edit::Edit) -> Self {
        let wants = edit.detail_wants();
        Self {
            local_contrast: edit.detail.options().is_some(),
            dehaze: edit.detail.dehaze_options().is_some(),
            bands: wants.bands(),
            delta: wants.sharpen,
            sample: edit.samples_before_detail(),
            refined: greycard_render::refine::refines(edit),
        }
    }
}

impl Cost {
    /// What the open frame of `pixels` holds at rest under an edit that
    /// `uses` so much, its develop where `gpu` says, and the room kept
    /// beside it for what comes and goes, as the machine's memory counts
    /// them. The room is one base's making or, on unified memory, the
    /// sharpen's working set with the local contrast's planes when the
    /// edit runs it, where that is more: an adjustment holds those, a
    /// prefetch runs only at rest, and an open makes its base before its
    /// develop sharpens, so a making and the sets are not held at once
    /// (a prefetch beside a set still held counts it: the worker's room
    /// check).
    pub fn charged(&self, pixels: u64, gpu: Gpu, uses: Uses) -> (u64, u64) {
        let per = |b: u64| b.saturating_mul(pixels);
        let on = |used: bool, h: Held| if used { h } else { Held::NONE };
        let beside = [
            on(uses.bands, self.bands),
            on(uses.delta, self.delta),
            on(uses.sample, self.sample),
            on(uses.refined, self.refined),
        ];
        let memory: u64 = beside.iter().map(|h| h.memory).sum();
        match gpu {
            Gpu::Unified => {
                let device: u64 = beside.iter().map(|h| h.device).sum::<u64>()
                    + if uses.local_contrast {
                        self.after_local_contrast
                    } else {
                        0
                    }
                    + if uses.dehaze { self.after_dehaze } else { 0 };
                let sets = self.sharpen(pixels).saturating_add(if uses.local_contrast {
                    per(self.local_contrast)
                } else {
                    0
                });
                (
                    per(self.held_unified + memory + device),
                    per(self.making_unified).max(sets),
                )
            }
            Gpu::Discrete => (per(self.held_memory + memory), per(self.making_memory)),
            Gpu::None => (per(self.held_cpu + memory), per(self.making_cpu)),
        }
    }

    /// What making a base of `pixels` adds while it is made, as the
    /// machine's memory counts it with `gpu`.
    pub fn making(&self, pixels: u64, gpu: Gpu) -> u64 {
        pixels.saturating_mul(match gpu {
            Gpu::Unified => self.making_unified,
            Gpu::Discrete => self.making_memory,
            Gpu::None => self.making_cpu,
        })
    }

    /// The sharpen's working set for a picture of `pixels`, on the
    /// device.
    pub fn sharpen(&self, pixels: u64) -> u64 {
        self.sharpen
            .0
            .saturating_mul(pixels)
            .saturating_add(self.sharpen.1)
    }
}

/// The pixels of the developed picture of a frame `facts` describes:
/// a raw's crop of its sensor, a picture's own size.
pub fn pixels(facts: &Facts) -> u64 {
    let (w, h) = match &facts.input {
        InputKind::Raw { crop, .. } => *crop,
        InputKind::Picture { size, .. } => *size,
    };
    (w as u64) * (h as u64)
}

/// The share of the machine the editor may take in all: two thirds,
/// the last third the system's and the other programs'.
const SHARE: (u64, u64) = (2, 3);

/// What the editor holds besides what the per-pixel figures count,
/// inside its share: the window, its caches and previews, the
/// libraries, a model's session. From the owner's Mac (24 GB unified,
/// 102 MP, 2026-11-26): 13.3 GB at rest with three bases on the shelf
/// (3.7 GB) and the sharpen's set then kept (1.8 GB) leaves 7.8 GB for
/// the open frame and the rest; the open frame's 57 a pixel is 5.8 GB,
/// so 2.0 GB is outside the figures, rounded up to 2 GiB.
pub const BESIDE: u64 = 2 << 30;

/// A machine as the account reads it: what the editor's pictures may hold
/// of its memory in all ([`SHARE`] of it, less [`BESIDE`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Machine {
    pub allowance: u64,
}

impl Machine {
    /// A machine of `ram` bytes.
    pub fn of(ram: u64) -> Self {
        Self {
            allowance: (ram / SHARE.1 * SHARE.0).saturating_sub(BESIDE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: u64 = 1 << 30;

    /// The device's share counts against the machine only on unified
    /// memory.
    #[test]
    fn the_device_counts_only_on_unified_memory() {
        let held = Held {
            memory: 3,
            device: 5,
        };
        assert_eq!(held.charged(Gpu::Unified), 8);
        assert_eq!(held.charged(Gpu::Discrete), 3);
        assert_eq!(held.charged(Gpu::None), 3);
    }

    /// The allowance is two thirds of the machine less what the editor
    /// holds beside its pictures.
    #[test]
    fn a_machine_s_allowance() {
        assert_eq!(Machine::of(24 * G).allowance, 14 * G);
        assert_eq!(Machine::of(3 * G).allowance, 0);
    }

    /// The room beside the open frame is one making or, on unified
    /// memory, the sharpen's working set with the local contrast's
    /// planes where the edit runs it, whichever is more, never the
    /// making and the sets together; on a discrete card the sets are
    /// the card's.
    #[test]
    fn the_room_is_a_making_or_the_sets() {
        let px = 100_000_000;
        let making = COST.making(px, Gpu::Unified);
        let sharpen = COST.sharpen(px);
        assert_eq!(sharpen, 16 * px + (192 << 20));
        let lc = Uses {
            local_contrast: true,
            ..Uses::NONE
        };
        assert_eq!(
            COST.charged(px, Gpu::Unified, Uses::NONE).1,
            making.max(sharpen)
        );
        let more = Cost {
            sharpen: (40, 0),
            ..COST
        };
        assert_eq!(more.charged(px, Gpu::Unified, Uses::NONE).1, making);
        assert_eq!(more.charged(px, Gpu::Unified, lc).1, 64 * px);
        assert_eq!(
            more.charged(px, Gpu::Discrete, lc).1,
            COST.making_memory * px
        );
    }

    /// What an edit keeps beside the picture: the pictures before the
    /// sharpen on the device count on unified memory only, a mask's
    /// planes in memory everywhere; the default edit keeps nothing.
    #[test]
    fn an_edit_s_sections_and_masks_add_what_they_keep() {
        let px = 1000;
        let all = Uses {
            local_contrast: true,
            dehaze: true,
            bands: true,
            delta: true,
            sample: true,
            refined: true,
        };
        let held = |gpu, uses| COST.charged(px, gpu, uses).0;
        assert_eq!(held(Gpu::Unified, Uses::NONE), 57 * px);
        assert_eq!(
            held(Gpu::Unified, all),
            (57 + 16 + 16 + 16 + 4 + 20 + 6) * px
        );
        assert_eq!(held(Gpu::Discrete, Uses::NONE), 23 * px);
        assert_eq!(held(Gpu::Discrete, all), (23 + 8 + 2 + 12 + 6) * px);
        assert_eq!(held(Gpu::None, all), (24 + 8 + 2 + 12 + 6) * px);
        assert_eq!(Uses::of(&greycard_edit::Edit::default()), Uses::NONE);
        let mut edit = greycard_edit::Edit::default();
        edit.detail.clarity = 0.3;
        edit.detail.dehaze = 0.2;
        let uses = Uses::of(&edit);
        assert!(uses.local_contrast && uses.dehaze && !uses.bands);
    }
}
