//! A preset as it reaches one frame, with no window: whether its camera
//! profile fits the body that took the frame, and the preset laid as
//! one named step over a sidecar. The panel's sync and the start ask
//! these on the window's thread, the import on its own.

use std::path::Path;

use greycard_edit::camera::{self, ProfileChoice};
use greycard_edit::preset::Section;
use greycard_edit::{Preset, Sidecar};

/// Whether a camera profile may go to a frame of `body` (make and
/// model). A profile the directory has not got, or one that names no
/// camera, cannot be checked and goes, as it would in a preset; one
/// made for a camera goes only to that camera's frames, and a frame
/// that cannot say what took it does not get it.
pub fn profile_fits(entry: Option<&camera::Entry>, body: Option<camera::Body>) -> bool {
    let Some(entry) = entry else {
        return true;
    };
    if entry
        .unique_camera_model
        .as_deref()
        .is_none_or(|u| u.trim().is_empty())
    {
        return true;
    }
    body.is_some_and(|body| entry.fits(body))
}

/// A probed file's camera, as a profile's fit reads it.
pub fn probed_body(p: &greycard_core::decode::Probe) -> camera::Body<'_> {
    camera::Body::new(&p.make, &p.model, &p.other_names)
}

/// `panel::sync::preset_for_body` with no state: `body` says what took
/// the frame (make and model), asked only when the preset names a
/// profile. The import runs this on its own thread, where there is no
/// `State`.
pub fn preset_fitted(
    preset: &Preset,
    profiles: &[camera::Entry],
    body: impl FnOnce() -> Option<greycard_core::decode::Probe>,
) -> (Preset, bool) {
    let named = match &preset.edit.camera.profile {
        ProfileChoice::Named(name) if preset.sections.contains(&Section::Camera) => {
            profiles.iter().find(|e| &e.name == name)
        }
        _ => None,
    };
    let Some(entry) = named else {
        return (preset.clone(), false);
    };
    let body = body();
    if profile_fits(Some(entry), body.as_ref().map(probed_body)) {
        (preset.clone(), false)
    } else {
        let mut without = preset.clone();
        without.sections.retain(|s| *s != Section::Camera);
        (without, true)
    }
}

/// `preset` as it reaches or leaves `file`: itself on a raw, and
/// without the display curve on a picture (a JPEG, a TIFF). A picture
/// shows no switch for the curve, and its own is the default, so a
/// curve laid on one could not be seen or set back (yet a fitted look
/// would still go by it), and one taken from one is no choice anybody
/// made.
pub fn curve_fitted(preset: &Preset, file: &Path) -> Preset {
    let mut out = preset.clone();
    if greycard_core::picture::is_picture_path(file) {
        out.sections.retain(|s| *s != Section::DisplayCurve);
    }
    out
}

/// `preset` as it reaches `file`: [`curve_fitted`], and its white
/// balance as the frame's kind takes one ([`white_fitted`]), a raw's
/// As shot read from the file only when an offset lands on it.
pub fn frame_fitted(preset: &Preset, file: &Path, camera: &camera::Camera) -> Preset {
    let preset = curve_fitted(preset, file);
    white_fitted(&preset, file, || as_shot_at(file, camera))
}

/// `preset` with its white balance as `file` takes it: an offset onto a
/// picture as it is, onto a raw as the light it names at the raw's As
/// shot (`as_shot`); a kelvin light onto a raw as it is. Where the frame
/// cannot take it, a light onto a picture or an offset onto a raw that
/// will not say its As shot, the section is left out and the frame
/// keeps its own, said in the log.
pub fn white_fitted(
    preset: &Preset,
    file: &Path,
    as_shot: impl FnOnce() -> Option<greycard_core::TempTint>,
) -> Preset {
    if !preset.sections.contains(&Section::WhiteBalance) {
        return preset.clone();
    }
    let picture = greycard_core::picture::is_picture_path(file);
    let mut out = preset.clone();
    match greycard_edit::white::landing(&preset.edit.white_balance, picture, as_shot) {
        Ok(wb) => out.edit.white_balance = wb,
        Err(why) => {
            tracing::info!(
                "{}: white balance left as it was: {why}",
                file.file_name().unwrap_or_default().to_string_lossy()
            );
            out.sections.retain(|s| *s != Section::WhiteBalance);
        }
    }
    out
}

/// The light the raw at `path` recorded, as the develop reads it
/// through `camera`'s profile (a DCP's, or the raw's own): the kelvin
/// and Duv its As shot means. Read from the raw without a sample;
/// `None` for a picture, or a raw that will not say.
pub fn as_shot_at(path: &Path, camera: &camera::Camera) -> Option<greycard_core::TempTint> {
    if greycard_core::picture::is_picture_path(path) {
        return None;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let white = greycard_core::decode::white_path(path)
        .inspect_err(|e| tracing::warn!("{name}: its as-shot white: {e}"))
        .ok()?;
    as_shot_through(&white, || {
        camera
            .profile(&greycard_edit_io::Stores::user())
            .map(|p| p.camera.clone())
    })
}

/// The light `white` recorded, read through the profile `chosen` gives
/// (a DCP's) or, with none, the raw's own. A monochrome sensor's is
/// read through its own whatever is chosen, as its develop and the
/// panel read it (`color::resolve_white_balance`), so a fitted white
/// is the one the panel shows.
pub fn as_shot_through(
    white: &greycard_core::decode::CameraWhite,
    chosen: impl FnOnce() -> Option<greycard_core::CameraProfile>,
) -> Option<greycard_core::TempTint> {
    let profile = match chosen().filter(|_| !white.monochrome) {
        Some(p) => p,
        None => white.profile().ok()?,
    };
    greycard_core::color::temp_tint_of_gains(&profile, white.as_shot_coefficients?).ok()
}

/// `--preset`: lay `preset` over `file`'s edit as one step named for
/// it, and write the sidecar where `placement` says (`None`: the run
/// writes no sidecars). Written here and not left to the quit's
/// `save_edit`, which finds the panel equal to the step already
/// recorded and so writes nothing: the step would be lost on close.
///
/// The preset goes on the line of `version` (the file's own for none);
/// a line that is not there takes nothing. A raw's line still waiting on
/// its ISO's learned-denoiser blend (never developed,
/// `on_disk::line_never_developed`) is given it first, as
/// `panel::sync::lay_over_targets` gives a target: the step makes the
/// edit no longer the default, and once written the next launch would
/// take that to mean the blend was seeded already. A preset that
/// changes nothing records and writes nothing. True when the sidecar
/// was written.
pub fn preset_at_start(
    sidecar: &mut Sidecar,
    version: Option<&str>,
    file: &Path,
    preset: &Preset,
    placement: Option<greycard_edit::Placement>,
) -> bool {
    let Some(line) = sidecar.line_mut(version) else {
        return false;
    };
    let camera = if preset.sections.contains(&Section::Camera) {
        preset.edit.camera.clone()
    } else {
        line.current.camera.clone()
    };
    let preset = &frame_fitted(preset, file, &camera);
    let applied = preset.applied(&line.current);
    if applied == line.current {
        return false;
    }
    let mut over = line.current.clone();
    if greycard_edit_io::on_disk::line_never_developed(line)
        && !greycard_core::picture::is_picture_path(file)
        && let Ok(p) = greycard_core::decode::probe_path(file)
    {
        over.noise.learned_strength = greycard_edit::Noise::blend_for_iso(p.iso);
        line.current.noise.learned_strength = over.noise.learned_strength;
    }
    let label = greycard_edit::history::preset_label(&preset.name);
    if !line.record_as(preset.applied(&over), Some(label)) {
        return false;
    }
    match placement.map(|p| sidecar.save_in(file, p)) {
        Some(Ok(())) => true,
        Some(Err(e)) => {
            tracing::warn!("{}: sidecar not saved: {e}", file.display());
            false
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::preset::Choice;

    #[test]
    fn a_picture_neither_takes_nor_gives_a_display_curve() {
        let preset = Preset::from_choice(
            "p",
            &greycard_edit::Edit::default(),
            &Choice::whole(&[Section::Light, Section::DisplayCurve]),
        );
        for raw in ["/a/IMG_0001.CR3", "/a/PXL_0001.dng"] {
            assert_eq!(
                curve_fitted(&preset, Path::new(raw)).sections,
                preset.sections
            );
        }
        for picture in ["/a/IMG_0001.JPG", "/a/scan.tif"] {
            assert_eq!(
                curve_fitted(&preset, Path::new(picture)).sections,
                [Section::Light]
            );
        }
    }

    /// A monochrome raw's As shot, as a fit reads it, is the one the
    /// panel shows, whatever DCP is stored for its body; a color raw's
    /// is still read through the DCP.
    #[test]
    fn a_monochrome_raw_s_as_shot_is_the_panel_s_whatever_the_dcp() {
        use greycard_core::color::{
            as_shot_temp_tint, profile_from_calibrations, profile_from_frame,
        };
        let dir = crate::testing::scratch_dir("mono-as-shot");
        let mono = dir.join("mono.dng");
        crate::testing::monochrome_dng(&mono, 32, 24);
        let color = dir.join("color.dng");
        crate::testing::linear_dng(&color, 32, 24, "Test", "Cam");
        let read = |path: &Path| {
            (
                greycard_core::decode::white_path(path).unwrap(),
                greycard_core::decode::decode_path(path).unwrap(),
            )
        };
        let ((mono_white, mono_frame), (color_white, color_frame)) = (read(&mono), read(&color));
        crate::testing::remove_dir_retry(&dir);
        let dcp = profile_from_calibrations(&[greycard_core::raw::Calibration {
            illuminant: 17,
            color_matrix: vec![0.9, -0.3, -0.1, -0.4, 1.2, 0.2, -0.05, 0.13, 0.5],
            forward_matrix: None,
        }])
        .unwrap();

        let panel =
            as_shot_temp_tint(&mono_frame, &profile_from_frame(&mono_frame).unwrap()).unwrap();
        assert!((panel.cct - 6504.0).abs() < 10.0);
        for chosen in [None, Some(dcp.clone())] {
            assert_eq!(as_shot_through(&mono_white, || chosen), Some(panel));
        }
        let own =
            as_shot_temp_tint(&color_frame, &profile_from_frame(&color_frame).unwrap()).unwrap();
        assert_eq!(as_shot_through(&color_white, || None), Some(own));
        let through = as_shot_through(&color_white, || Some(dcp)).unwrap();
        assert!((through.cct - own.cct).abs() > 10.0, "{through:?} {own:?}");
    }

    /// A preset's white balance as each frame takes it: a light onto a
    /// picture is left out and the picture keeps its own; an offset onto
    /// a raw is the light it names at the raw's As shot, or left out
    /// when the raw will not say; onto a picture it goes as it is.
    #[test]
    fn a_white_balance_lands_as_the_frame_takes_one() {
        use greycard_edit::WhiteBalance;
        let with = |wb: WhiteBalance| {
            let edit = greycard_edit::Edit {
                white_balance: wb,
                ..Default::default()
            };
            Preset::from_choice(
                "p",
                &edit,
                &Choice::whole(&[Section::Light, Section::WhiteBalance]),
            )
        };
        let light = with(WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.0,
        });
        let offset = with(WhiteBalance::Offset {
            mired: 25.0,
            tint: 0.0,
        });
        let picture = Path::new("/a/scan.jpg");
        let raw = Path::new("/a/IMG_0001.CR3");
        let fitted = white_fitted(&light, picture, || None);
        assert_eq!(fitted.sections, [Section::Light]);
        assert_eq!(white_fitted(&light, raw, || None), light);
        assert_eq!(white_fitted(&offset, picture, || None), offset);
        let shot = greycard_core::TempTint::new(5000.0, 0.001);
        let onto_raw = white_fitted(&offset, raw, || Some(shot));
        let WhiteBalance::Custom { temperature, tint } = onto_raw.edit.white_balance else {
            panic!("{:?}", onto_raw.edit.white_balance)
        };
        assert!((1e6 / 5000.0 - 1e6 / temperature - 25.0).abs() < 1e-9);
        assert_eq!(tint, 0.001);
        assert_eq!(
            white_fitted(&offset, raw, || None).sections,
            [Section::Light]
        );
    }
}
