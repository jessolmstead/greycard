//! The display transform's last step as data: a 3D lookup table from
//! the output's encoded RGB to the monitor's, built through Little
//! CMS from the output space's profile and the monitor's, or the
//! identity when both are sRGB. A soft proof puts a third profile
//! between them, rendered with an intent, and marks in the table's
//! alpha what that profile cannot hold. The viewport shader samples
//! the table after the tone curve and the encoding, so a profile is
//! a data change and nothing else.

use std::path::PathBuf;

use anyhow::{Context, Result};
use lcms2::{
    CIExyY, CIExyYTRIPLE, Flags, Intent as Lcms, PixelFormat, Profile, ThreadContext, ToneCurve,
    Transform,
};

use crate::export::Space;

/// Points per axis; 33 is the usual compromise of size and smoothness
/// for a display transform.
pub const LUT_SIZE: usize = 33;
/// The space the gamut marks are judged in: the working space, as the
/// shader has it before the output matrix, with the output's transfer.
const WORKING: Space = Space::Rec2020;

/// A lookup table over the output's encoded RGB, red fastest, then
/// green, then blue, each entry the monitor's encoded RGB and whether
/// the proof's profile cannot hold that color.
pub struct Lut3d {
    pub size: usize,
    pub rgb: Vec<[f32; 3]>,
    /// Out of the proof's gamut, one per entry; empty without a proof
    /// that asks.
    pub warn: Vec<bool>,
}

/// How a color the proof's profile cannot hold is brought in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProofIntent {
    /// The whole picture squeezed to fit, as a print usually is.
    #[default]
    Perceptual,
    /// Colors that fit kept exactly and the rest clipped to the edge,
    /// with black point compensation.
    Relative,
}

impl ProofIntent {
    pub const ALL: [ProofIntent; 2] = [ProofIntent::Perceptual, ProofIntent::Relative];

    pub fn name(self) -> &'static str {
        match self {
            ProofIntent::Perceptual => "Perceptual",
            ProofIntent::Relative => "Relative",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.name() == name)
    }
}

/// The profile a soft proof shows the picture through: one of the
/// export's spaces, or a file, a printer's usually.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofProfile {
    Space(Space),
    File(PathBuf),
}

impl ProofProfile {
    /// As the settings keep it: a space's name, or the file's path.
    pub fn key(&self) -> String {
        match self {
            ProofProfile::Space(s) => s.name().to_string(),
            ProofProfile::File(p) => p.to_string_lossy().into_owned(),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        if key.is_empty() {
            return None;
        }
        Some(match Space::from_name(key) {
            Some(s) => ProofProfile::Space(s),
            None => ProofProfile::File(PathBuf::from(key)),
        })
    }

    fn profile(&self, ctx: &ThreadContext) -> Result<Profile<ThreadContext>> {
        match self {
            ProofProfile::Space(s) => Profile::new_icc_context(ctx, &s.icc()?)
                .map_err(|e| anyhow::anyhow!("the {} profile: {e}", s.name())),
            ProofProfile::File(p) => Profile::new_file_context(ctx, p)
                .with_context(|| format!("reading {}", p.display())),
        }
    }
}

/// A soft proof: the profile, how the picture is rendered to it, and
/// whether to mark what it cannot hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    pub profile: ProofProfile,
    pub intent: ProofIntent,
    pub warn: bool,
}

/// The monitor's profile, the table's far end. `Srgb` is no
/// correction: the output's sRGB goes to the screen as it is. The two
/// standards are for a monitor whose own menu emulates one of them,
/// which the profile colord derives from the EDID cannot know, since
/// the EDID describes the panel's native gamut; a file is a measured
/// profile, or any other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorProfile {
    Srgb,
    AdobeRgb,
    DisplayP3,
    File(PathBuf),
}

impl MonitorProfile {
    pub const STANDARD: [MonitorProfile; 3] = [
        MonitorProfile::Srgb,
        MonitorProfile::AdobeRgb,
        MonitorProfile::DisplayP3,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            MonitorProfile::Srgb => "sRGB",
            MonitorProfile::AdobeRgb => "Adobe RGB",
            MonitorProfile::DisplayP3 => "Display P3",
            MonitorProfile::File(_) => "File",
        }
    }

    /// As the settings keep it: a standard's name, or the file's path.
    pub fn key(&self) -> String {
        match self {
            MonitorProfile::File(p) => p.to_string_lossy().into_owned(),
            standard => standard.name().to_string(),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        if key.is_empty() {
            return None;
        }
        Some(
            Self::STANDARD
                .into_iter()
                .find(|s| s.name() == key)
                .unwrap_or_else(|| MonitorProfile::File(PathBuf::from(key))),
        )
    }

    fn profile(&self, ctx: &ThreadContext) -> Result<Profile<ThreadContext>> {
        // D65, as every display standard has it.
        let white = CIExyY {
            x: 0.3127,
            y: 0.3290,
            Y: 1.0,
        };
        let point = |x: f64, y: f64| CIExyY { x, y, Y: 1.0 };
        let standard = |name: &str, primaries: CIExyYTRIPLE, curve: ToneCurve| {
            Profile::new_rgb_context(ctx, &white, &primaries, &[&curve, &curve, &curve])
                .map_err(|e| anyhow::anyhow!("the {name} monitor profile: {e}"))
        };
        match self {
            MonitorProfile::Srgb => Ok(Profile::new_srgb_context(ctx)),
            // Adobe RGB (1998): its primaries and its gamma, 563/256.
            MonitorProfile::AdobeRgb => standard(
                "Adobe RGB",
                CIExyYTRIPLE {
                    Red: point(0.64, 0.33),
                    Green: point(0.21, 0.71),
                    Blue: point(0.15, 0.06),
                },
                ToneCurve::new(crate::export::ADOBE_RGB_GAMMA),
            ),
            // Display P3: DCI-P3's primaries under the sRGB curve.
            MonitorProfile::DisplayP3 => standard(
                "Display P3",
                CIExyYTRIPLE {
                    Red: point(0.680, 0.320),
                    Green: point(0.265, 0.690),
                    Blue: point(0.150, 0.060),
                },
                ToneCurve::new_parametric(
                    4,
                    &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045],
                )
                .map_err(|e| anyhow::anyhow!("the sRGB curve: {e}"))?,
            ),
            MonitorProfile::File(p) => Profile::new_file_context(ctx, p)
                .with_context(|| format!("reading {}", p.display())),
        }
    }
}

impl Lut3d {
    pub fn identity() -> Self {
        Self {
            size: LUT_SIZE,
            rgb: grid().collect(),
            warn: Vec::new(),
        }
    }

    /// The table for `output` shown on `monitor`, through `proof` if
    /// there is one. Without a proof, relative colorimetric, which is
    /// what a display transform wants: white stays white. With one,
    /// the proof's intent from the output to its profile, then
    /// relative to the monitor, and when it asks, every grid point
    /// the proof's profile cannot hold is marked. The marks are over
    /// the working space's grid, not the output's: the output is the
    /// picture clamped into its space already, so an sRGB output
    /// proofed for sRGB would never warn, though the picture had
    /// colors sRGB cannot hold. The shader looks the mark up at the
    /// color before the output matrix.
    pub fn build(output: Space, monitor: &MonitorProfile, proof: Option<&Proof>) -> Result<Self> {
        if output == Space::Srgb && *monitor == MonitorProfile::Srgb && proof.is_none() {
            return Ok(Self::identity());
        }
        let mut ctx = ThreadContext::new();
        // The gamut check paints what is out with this color; it is
        // compared with the unchecked table below, so its value only
        // has to be one the transform would not land on itself.
        let mut alarm = [0u16; 16];
        alarm[0] = 0xbeef;
        alarm[1] = 0x0042;
        alarm[2] = 0xf00d;
        ctx.set_alarm_codes(alarm);
        let from = Profile::new_icc_context(&ctx, &output.finish_icc()?)
            .map_err(|e| anyhow::anyhow!("the {} profile: {e}", output.name()))?;
        let to = monitor.profile(&ctx)?;
        let input: Vec<[f32; 3]> = grid().collect();
        let mut rgb = vec![[0.0f32; 3]; input.len()];
        let mut warn = Vec::new();
        match proof {
            None => {
                let transform = Transform::new_flags_context(
                    &ctx,
                    &from,
                    PixelFormat::RGB_FLT,
                    &to,
                    PixelFormat::RGB_FLT,
                    Lcms::RelativeColorimetric,
                    Flags::default(),
                )
                .context("building the display transform")?;
                transform.transform_pixels(&input, &mut rgb);
            }
            Some(proof) => {
                let through = proof.profile.profile(&ctx)?;
                let (intent, flags) = match proof.intent {
                    ProofIntent::Perceptual => (Lcms::Perceptual, Flags::SOFT_PROOFING),
                    ProofIntent::Relative => (
                        Lcms::RelativeColorimetric,
                        Flags::SOFT_PROOFING | Flags::BLACKPOINT_COMPENSATION,
                    ),
                };
                let proofing = |from: &Profile<ThreadContext>, flags| {
                    Transform::new_proofing_context(
                        &ctx,
                        from,
                        PixelFormat::RGB_FLT,
                        &to,
                        PixelFormat::RGB_FLT,
                        &through,
                        intent,
                        Lcms::RelativeColorimetric,
                        flags,
                    )
                    .context("building the proofing transform")
                };
                proofing(&from, flags)?.transform_pixels(&input, &mut rgb);
                if proof.warn {
                    let wide = Profile::new_icc_context(&ctx, &WORKING.icc()?)
                        .map_err(|e| anyhow::anyhow!("the {} profile: {e}", WORKING.name()))?;
                    let mut plain = vec![[0.0f32; 3]; input.len()];
                    let mut checked = vec![[0.0f32; 3]; input.len()];
                    proofing(&wide, flags)?.transform_pixels(&input, &mut plain);
                    proofing(&wide, flags | Flags::GAMUT_CHECK)?
                        .transform_pixels(&input, &mut checked);
                    warn = plain.iter().zip(&checked).map(|(a, b)| a != b).collect();
                }
            }
        }
        for px in rgb.iter_mut() {
            for v in px.iter_mut() {
                *v = v.clamp(0.0, 1.0);
            }
        }
        Ok(Self {
            size: LUT_SIZE,
            rgb,
            warn,
        })
    }

    /// From a monitor's ICC profile alone: sRGB output, no proof.
    #[cfg(test)]
    pub fn from_icc(path: &std::path::Path) -> Result<Self> {
        Self::build(Space::Srgb, &MonitorProfile::File(path.to_path_buf()), None)
    }

    /// How far from the identity the table is, at most, per channel.
    pub fn max_deviation(&self) -> f32 {
        self.rgb
            .iter()
            .zip(grid())
            .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs()))
            .fold(0.0, f32::max)
    }

    /// The entry nearest an encoded color.
    #[cfg(test)]
    fn at(&self, c: [f32; 3]) -> (usize, [f32; 3]) {
        let n = self.size;
        let k = |v: f32| (v * (n - 1) as f32).round() as usize;
        let i = k(c[0]) + n * k(c[1]) + n * n * k(c[2]);
        (i, self.rgb[i])
    }
}

fn grid() -> impl Iterator<Item = [f32; 3]> {
    let n = LUT_SIZE;
    (0..n * n * n).map(move |i| {
        let step = |k: usize| k as f32 / (n - 1) as f32;
        [step(i % n), step(i / n % n), step(i / (n * n))]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_is_the_grid() {
        let lut = Lut3d::identity();
        assert_eq!(lut.rgb.len(), LUT_SIZE * LUT_SIZE * LUT_SIZE);
        assert_eq!(lut.rgb[0], [0.0, 0.0, 0.0]);
        assert_eq!(lut.rgb[1], [1.0 / 32.0, 0.0, 0.0]);
        assert_eq!(lut.rgb[LUT_SIZE], [0.0, 1.0 / 32.0, 0.0]);
        assert_eq!(*lut.rgb.last().unwrap(), [1.0, 1.0, 1.0]);
        assert_eq!(lut.max_deviation(), 0.0);
        assert!(lut.warn.is_empty());
    }

    #[test]
    fn an_srgb_profile_is_nearly_the_identity() {
        // Little CMS's own sRGB, written out and read back as a file.
        let dir = std::env::temp_dir().join(format!("greycard-ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("srgb.icc");
        let bytes = lcms2::Profile::new_srgb().icc().unwrap();
        std::fs::write(&path, bytes).unwrap();
        let lut = Lut3d::from_icc(&path).unwrap();
        assert!(lut.max_deviation() < 1.0 / 255.0, "{}", lut.max_deviation());
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_wide_output_on_an_srgb_monitor_keeps_the_greys_and_clips_the_greens() {
        let lut = Lut3d::build(Space::Rec2020, &MonitorProfile::Srgb, None).unwrap();
        // White and a mid grey are the same in both.
        let (_, white) = lut.at([1.0; 3]);
        assert!(white.iter().all(|&v| v > 0.99), "{white:?}");
        let (_, grey) = lut.at([0.5; 3]);
        assert!(grey.iter().all(|&v| (v - 0.5).abs() < 0.02), "{grey:?}");
        // Rec.2020's green is outside sRGB: it lands on the edge.
        let (_, green) = lut.at([0.0, 1.0, 0.0]);
        assert!(green[1] > 0.99 && green[0] < 0.01, "{green:?}");
        assert!(lut.warn.is_empty());
    }

    #[test]
    fn a_proof_to_srgb_marks_what_srgb_cannot_hold() {
        let proof = Proof {
            profile: ProofProfile::Space(Space::Srgb),
            intent: ProofIntent::Relative,
            warn: true,
        };
        let lut = Lut3d::build(Space::Rec2020, &MonitorProfile::Srgb, Some(&proof)).unwrap();
        assert_eq!(lut.warn.len(), lut.rgb.len());
        let (grey, _) = lut.at([0.5; 3]);
        assert!(!lut.warn[grey], "a grey is in every gamut");
        let (green, _) = lut.at([0.0, 1.0, 0.0]);
        assert!(lut.warn[green], "Rec.2020's green is outside sRGB");
        let (red, _) = lut.at([1.0, 0.0, 0.0]);
        assert!(lut.warn[red], "and so is its red");
        // The marks agree with the matrix: a grid point whose linear
        // sRGB is well inside the cube is not marked, one well outside
        // is. Near the edge Little CMS has its own tolerance.
        let m = Space::Srgb.matrix();
        let decode = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (mut inside, mut outside) = (0, 0);
        for (i, c) in grid().enumerate() {
            let lin = c.map(decode);
            let s: Vec<f32> = m
                .iter()
                .map(|row| row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2])
                .collect();
            if s.iter().all(|&v| (0.03..=0.97).contains(&v)) {
                assert!(!lut.warn[i], "{c:?} is inside sRGB, {s:?}");
                inside += 1;
            } else if s.iter().any(|&v| !(-0.05..=1.05).contains(&v)) {
                assert!(lut.warn[i], "{c:?} is outside sRGB, {s:?}");
                outside += 1;
            }
        }
        assert!(
            inside > 1000 && outside > 1000,
            "{inside} in, {outside} out"
        );
        // Without the ask, no marks, and the same picture.
        let quiet = Lut3d::build(
            Space::Rec2020,
            &MonitorProfile::Srgb,
            Some(&Proof {
                warn: false,
                ..proof.clone()
            }),
        )
        .unwrap();
        assert!(quiet.warn.is_empty());
        assert_eq!(quiet.rgb, lut.rgb);
    }

    #[test]
    fn a_proof_of_srgb_in_srgb_changes_nothing_much() {
        let proof = Proof {
            profile: ProofProfile::Space(Space::Srgb),
            intent: ProofIntent::Perceptual,
            warn: true,
        };
        let lut = Lut3d::build(Space::Srgb, &MonitorProfile::Srgb, Some(&proof)).unwrap();
        assert!(lut.max_deviation() < 2.0 / 255.0, "{}", lut.max_deviation());
        // The marks are the working space's, and a good part of its
        // grid lies outside sRGB; the greys never do.
        let marked = lut.warn.iter().filter(|&&w| w).count();
        assert!(
            marked > lut.warn.len() / 10,
            "{marked} of {}",
            lut.warn.len()
        );
        for level in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let (grey, _) = lut.at([level; 3]);
            assert!(!lut.warn[grey], "grey {level}");
        }
    }

    #[test]
    fn the_proof_profile_keys_round_trip() {
        for s in Space::ALL {
            let p = ProofProfile::Space(s);
            assert_eq!(ProofProfile::from_key(&p.key()), Some(p));
        }
        let f = ProofProfile::File(PathBuf::from("/tmp/paper.icc"));
        assert_eq!(ProofProfile::from_key(&f.key()), Some(f));
        assert_eq!(ProofProfile::from_key(""), None);
        for i in ProofIntent::ALL {
            assert_eq!(ProofIntent::from_name(i.name()), Some(i));
        }
    }

    #[test]
    fn the_table_for_adobe_rgb_shows_the_files_values() {
        // On a monitor that is Adobe RGB, what the table shows for the
        // finish's value is the value the file holds for it, at the
        // grid's points (between them the table interpolates).
        let lut = Lut3d::build(Space::AdobeRgb, &MonitorProfile::AdobeRgb, None).unwrap();
        let mut worst = 0f32;
        for c in [
            [0.5; 3],
            [0.25, 0.5, 0.75],
            [0.875, 0.625, 0.125],
            [0.0625; 3],
        ] {
            let (_, shown) = lut.at(c);
            for k in 0..3 {
                let want = Space::AdobeRgb.recode(c[k]);
                worst = worst.max((shown[k] - want).abs());
            }
        }
        eprintln!("adobe table against the file's values, worst {worst:e}");
        assert!(worst < 2.0 / 255.0, "{worst}");
    }

    #[test]
    fn an_srgb_monitor_is_the_identity_and_the_wide_ones_pull_the_primaries_in() {
        assert_eq!(
            Lut3d::build(Space::Srgb, &MonitorProfile::Srgb, None)
                .unwrap()
                .max_deviation(),
            0.0
        );
        for wide in [MonitorProfile::AdobeRgb, MonitorProfile::DisplayP3] {
            let lut = Lut3d::build(Space::Srgb, &wide, None).unwrap();
            // A grey is a grey on any monitor; the curves differ a
            // little, so its level may move a little.
            let (_, white) = lut.at([1.0; 3]);
            assert!(white.iter().all(|&v| v > 0.99), "{white:?}");
            let (_, grey) = lut.at([0.5; 3]);
            assert!(grey.iter().all(|&v| (v - 0.5).abs() < 0.02), "{grey:?}");
            assert!((grey[0] - grey[1]).abs() < 0.005 && (grey[1] - grey[2]).abs() < 0.005);
            // sRGB's red and green lie inside both gamuts: shown
            // short of the monitor's own primary, by less of the
            // channel or some of another. (Adobe RGB's red is sRGB's
            // chromaticity exactly, so there it is only dimmer.)
            for (c, own, other) in [([1.0, 0.0, 0.0], 0, 1), ([0.0, 1.0, 0.0], 1, 0)] {
                let (_, shown) = lut.at(c);
                assert!(
                    shown[own] < 0.95 || shown[other] > 0.05,
                    "{wide:?} {c:?} -> {shown:?}"
                );
            }
            assert!(
                lut.max_deviation() > 0.2,
                "{wide:?} {}",
                lut.max_deviation()
            );
        }
    }

    #[test]
    fn the_gamut_mark_is_over_the_working_space_and_srgb_output_still_warns() {
        let proof = Proof {
            profile: ProofProfile::Space(Space::Srgb),
            intent: ProofIntent::Relative,
            warn: true,
        };
        // An sRGB output proofed for sRGB: nothing in the output is out
        // of the proof's gamut, but the working space's green is.
        let lut = Lut3d::build(Space::Srgb, &MonitorProfile::Srgb, Some(&proof)).unwrap();
        assert_eq!(lut.warn.len(), lut.rgb.len());
        let (green, _) = lut.at([0.0, 1.0, 0.0]);
        let (grey, _) = lut.at([0.5; 3]);
        let (soft, _) = lut.at([0.6, 0.5, 0.45]);
        assert!(lut.warn[green], "Rec.2020's green is outside sRGB");
        assert!(
            !lut.warn[grey] && !lut.warn[soft],
            "a grey and a soft color are in it"
        );
        // Without the ask, no marks at all.
        let quiet = Proof {
            warn: false,
            ..proof
        };
        let lut = Lut3d::build(Space::Srgb, &MonitorProfile::Srgb, Some(&quiet)).unwrap();
        assert!(lut.warn.is_empty());
    }

    #[test]
    fn the_monitor_profile_keys_round_trip() {
        for m in MonitorProfile::STANDARD {
            assert_eq!(MonitorProfile::from_key(&m.key()), Some(m));
        }
        let f = MonitorProfile::File(PathBuf::from("/tmp/monitor.icc"));
        assert_eq!(MonitorProfile::from_key(&f.key()), Some(f.clone()));
        assert_eq!(f.name(), "File");
        assert_eq!(MonitorProfile::from_key(""), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("greycard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_cached_profile_is_rewritten_only_when_its_bytes_change() {
        let dir = scratch("monitor-cache");
        let (path, wrote) = cache_profile(&dir, 7, b"first").unwrap();
        assert!(wrote);
        assert_eq!(path, dir.join("7.icc"));
        assert_eq!(
            cache_profile(&dir, 7, b"first").unwrap(),
            (path.clone(), false)
        );
        assert_eq!(
            cache_profile(&dir, 7, b"second").unwrap(),
            (path.clone(), true)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        // Nothing left over from the rename.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_main_display_comes_first_and_each_has_a_name() {
        let dir = scratch("monitor-list");
        let display = |id, main, name: Option<&str>, icc: Option<&[u8]>| Display {
            id,
            main,
            name: name.map(str::to_string),
            icc: icc.map(<[u8]>::to_vec),
        };
        let monitors = monitors_from(
            vec![
                display(3, false, Some("Studio Display"), Some(b"p3")),
                display(1, true, None, Some(b"xdr")),
                display(5, false, None, None),
            ],
            Some(&dir),
        );
        let names: Vec<&str> = monitors.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(names, ["Display 1", "Studio Display", "Display 5"]);
        assert!(monitors[0].primary && !monitors[1].primary);
        assert_eq!(monitors[0].profile, Some(dir.join("1.icc")));
        assert_eq!(std::fs::read(dir.join("3.icc")).unwrap(), b"p3");
        assert_eq!(monitors[2].profile, None);
        // Without a cache, no profile is a path.
        let bare = monitors_from(vec![display(1, true, None, Some(b"xdr"))], None);
        assert_eq!(bare[0].profile, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// Where "System" takes the profile from, for the panel's note.
pub const SYSTEM_SOURCE: &str = if cfg!(target_os = "macos") {
    "ColorSync"
} else {
    "colord"
};

/// A monitor the system's color manager knows, with its current
/// profile.
#[derive(Debug, Clone)]
pub struct Monitor {
    pub model: String,
    /// Only colord and ColorSync say which display is the primary
    /// one, and only their own sort reads this; the field stays
    /// either way so the type is one type.
    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    pub primary: bool,
    pub profile: Option<std::path::PathBuf>,
}

/// The displays the system's color manager knows, primary first,
/// each with the path of its current profile if it has one: colord's
/// on Linux, ColorSync's on macOS. Until the compositor can say which
/// monitor a window is on, the primary is the best guess.
#[cfg(target_os = "linux")]
pub fn system_monitors() -> Result<Vec<Monitor>> {
    colord_monitors()
}

#[cfg(target_os = "macos")]
pub fn system_monitors() -> Result<Vec<Monitor>> {
    Ok(monitors_from(
        colorsync::displays(),
        profile_cache().as_deref(),
    ))
}

/// Windows' WCS is not read yet: no monitor, and the output falls
/// back to sRGB.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn system_monitors() -> Result<Vec<Monitor>> {
    Ok(Vec::new())
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn sort_primary_first(monitors: &mut [Monitor]) {
    monitors.sort_by_key(|m| !m.primary);
}

/// One display as ColorSync reports it, before its profile is a file.
#[cfg(any(target_os = "macos", test))]
struct Display {
    id: u32,
    main: bool,
    name: Option<String>,
    icc: Option<Vec<u8>>,
}

/// Where the displays' profiles are written: beside the thumbnail
/// cache, under `$XDG_CACHE_HOME` when that is set, else the
/// platform's cache.
#[cfg(target_os = "macos")]
fn profile_cache() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::cache_dir)
        .map(|c| c.join("greycard").join("monitors"))
}

/// The displays as monitors, primary first, each display's ICC data
/// written to `<cache>/<id>.icc` so a profile is a path here as it is
/// from colord. Without a cache, or when the write fails, the display
/// is listed without a profile.
#[cfg(any(target_os = "macos", test))]
fn monitors_from(displays: Vec<Display>, cache: Option<&std::path::Path>) -> Vec<Monitor> {
    let mut monitors: Vec<Monitor> = displays
        .into_iter()
        .map(|d| {
            let model = d.name.unwrap_or_else(|| format!("Display {}", d.id));
            let profile = match (d.icc, cache) {
                (Some(icc), Some(dir)) => match cache_profile(dir, d.id, &icc) {
                    Ok((path, _)) => Some(path),
                    Err(e) => {
                        tracing::warn!("ColorSync: {model}'s profile not cached: {e:#}");
                        None
                    }
                },
                _ => None,
            };
            Monitor {
                model,
                primary: d.main,
                profile,
            }
        })
        .collect();
    sort_primary_first(&mut monitors);
    for m in &monitors {
        let path = m
            .profile
            .as_ref()
            .map_or("no profile".to_string(), |p| p.display().to_string());
        let primary = if m.primary { " (primary)" } else { "" };
        tracing::info!("ColorSync: {}{primary}: {path}", m.model);
    }
    monitors
}

/// `icc` as `<dir>/<id>.icc`, written only when the file is missing
/// or holds other bytes, through a temporary file renamed over it so
/// a second editor never reads half a profile. True when written.
#[cfg(any(target_os = "macos", test))]
fn cache_profile(dir: &std::path::Path, id: u32, icc: &[u8]) -> Result<(PathBuf, bool)> {
    let path = dir.join(format!("{id}.icc"));
    if std::fs::read(&path).is_ok_and(|old| old == icc) {
        return Ok((path, false));
    }
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    let tmp = dir.join(format!("{id}.icc.{}.tmp", std::process::id()));
    let written = std::fs::write(&tmp, icc)
        .with_context(|| tmp.display().to_string())
        .and_then(|()| std::fs::rename(&tmp, &path).with_context(|| path.display().to_string()));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map(|()| (path, true))
}

/// The active displays from CoreGraphics, each with its ColorSync
/// profile's ICC data and the name AppKit gives it.
#[cfg(target_os = "macos")]
mod colorsync {
    use std::ptr::NonNull;

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSScreen;
    use objc2_core_foundation::CFRetained;
    use objc2_core_graphics::{
        CGColorSpace, CGDirectDisplayID, CGError, CGGetActiveDisplayList, CGMainDisplayID,
    };
    use objc2_foundation::{NSNumber, ns_string};

    use super::Display;

    // The crate's own binding panics when a display has no color
    // space; this one says so.
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayCopyColorSpace(display: CGDirectDisplayID) -> Option<NonNull<CGColorSpace>>;
    }

    pub(super) fn displays() -> Vec<Display> {
        let mut ids = [0 as CGDirectDisplayID; 16];
        let mut count = 0u32;
        // SAFETY: the list holds `ids.len()` entries and the count is
        // a valid u32.
        let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
        if err != CGError::Success {
            tracing::warn!("ColorSync: no display list (CGError {})", err.0);
            return Vec::new();
        }
        let names = screen_names();
        let main = CGMainDisplayID();
        ids[..count.min(ids.len() as u32) as usize]
            .iter()
            .map(|&id| Display {
                id,
                main: id == main,
                name: names
                    .iter()
                    .find(|(n, _)| *n == id)
                    .map(|(_, name)| name.clone()),
                icc: icc_data(id),
            })
            .collect()
    }

    fn icc_data(id: CGDirectDisplayID) -> Option<Vec<u8>> {
        // SAFETY: a display id from the active list; a null return is
        // handled, and a Copy function's result is ours to release.
        let space =
            unsafe { CGDisplayCopyColorSpace(id) }.map(|p| unsafe { CFRetained::from_raw(p) })?;
        CGColorSpace::icc_data(Some(&space)).map(|d| d.to_vec())
    }

    /// Each screen's display id and localized name; nothing off the
    /// main thread, where AppKit cannot be asked.
    fn screen_names() -> Vec<(CGDirectDisplayID, String)> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        NSScreen::screens(mtm)
            .iter()
            .filter_map(|screen| {
                let description = screen.deviceDescription();
                let number: Retained<AnyObject> =
                    description.objectForKey(ns_string!("NSScreenNumber"))?;
                let number = number.downcast_ref::<NSNumber>()?;
                Some((
                    number.unsignedIntValue(),
                    screen.localizedName().to_string(),
                ))
            })
            .collect()
    }
}

/// The displays colord manages, primary first, each with the path of
/// its current profile if it has one.
#[cfg(target_os = "linux")]
fn colord_monitors() -> Result<Vec<Monitor>> {
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::OwnedObjectPath;
    const SERVICE: &str = "org.freedesktop.ColorManager";
    let conn = Connection::system().context("the system bus")?;
    let manager =
        Proxy::new(&conn, SERVICE, "/org/freedesktop/ColorManager", SERVICE).context("colord")?;
    let devices: Vec<OwnedObjectPath> = manager
        .call("GetDevicesByKind", &("display",))
        .context("asking colord for displays")?;
    let mut monitors = Vec::new();
    for path in devices {
        let device = Proxy::new(&conn, SERVICE, path, "org.freedesktop.ColorManager.Device")?;
        let model: String = device.get_property("Model").unwrap_or_default();
        let metadata: std::collections::HashMap<String, String> =
            device.get_property("Metadata").unwrap_or_default();
        let profiles: Vec<OwnedObjectPath> = device.get_property("Profiles").unwrap_or_default();
        let profile = match profiles.first() {
            Some(p) => {
                let profile = Proxy::new(
                    &conn,
                    SERVICE,
                    p.clone(),
                    "org.freedesktop.ColorManager.Profile",
                )?;
                profile
                    .get_property::<String>("Filename")
                    .ok()
                    .filter(|f| !f.is_empty())
                    .map(std::path::PathBuf::from)
            }
            None => None,
        };
        monitors.push(Monitor {
            model,
            primary: metadata
                .get("OutputPriority")
                .is_some_and(|p| p == "primary"),
            profile,
        });
    }
    sort_primary_first(&mut monitors);
    Ok(monitors)
}
