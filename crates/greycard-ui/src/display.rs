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
                ToneCurve::new(563.0 / 256.0),
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
        let from = Profile::new_icc_context(&ctx, &output.icc()?)
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
        std::fs::remove_dir_all(&dir).unwrap();
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
}

/// A monitor colord knows, with its current profile.
#[derive(Debug, Clone)]
pub struct Monitor {
    pub model: String,
    /// Only colord says which display is the primary one, and only
    /// its own sort reads this; the field stays either way so the
    /// type is one type.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub primary: bool,
    pub profile: Option<std::path::PathBuf>,
}

/// colord is Linux's; elsewhere the editor knows no monitor and the
/// output falls back to sRGB, until each platform's own color
/// manager is read (ColorSync on macOS, WCS on Windows).
#[cfg(not(target_os = "linux"))]
pub fn colord_monitors() -> Result<Vec<Monitor>> {
    Ok(Vec::new())
}

/// The displays colord manages, primary first, each with the path of
/// its current profile if it has one. Until the compositor can say
/// which monitor a window is on, the primary is the best guess.
#[cfg(target_os = "linux")]
pub fn colord_monitors() -> Result<Vec<Monitor>> {
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
    monitors.sort_by_key(|m| !m.primary);
    Ok(monitors)
}
