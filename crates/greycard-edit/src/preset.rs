//! Presets: a named part of an edit kept in a file of its own, to be
//! laid over any picture's edit.
//!
//! A preset says which [`Section`]s it carries and holds a whole
//! [`Edit`] with those sections set, the rest at their defaults; laid
//! over an edit, the carried sections replace the picture's and the
//! rest is left alone. The file is JSON, `.gcp` beside the sidecar's
//! `.gcd`, under the user's configuration directory; the edit inside
//! is migrated as a sidecar's is, so a preset outlives a schema.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Edit, Error, Result, VERSION, migrate};

/// The parts of an edit a preset can carry. Not the geometry nor the
/// retouch, which are one picture's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Section {
    Light,
    WhiteBalance,
    Noise,
    /// Local contrast: Texture and Clarity.
    Detail,
    Sharpen,
    Curves,
    Mixer,
    Color,
    #[serde(rename = "black-and-white")]
    BlackWhite,
    Grading,
    Tint,
    /// The look table the picture finishes through.
    Look,
    Lens,
    Vignette,
    Grain,
    Demosaic,
    /// The camera profile the develop starts from.
    Camera,
    Adjustments,
}

impl Section {
    /// In the panel's order.
    pub const ALL: &[Section] = &[
        Section::WhiteBalance,
        Section::Light,
        Section::Color,
        Section::Curves,
        Section::Mixer,
        Section::BlackWhite,
        Section::Grading,
        Section::Tint,
        Section::Look,
        Section::Noise,
        Section::Lens,
        Section::Detail,
        Section::Sharpen,
        Section::Vignette,
        Section::Grain,
        Section::Demosaic,
        Section::Camera,
        Section::Adjustments,
    ];

    /// The serialized name.
    pub fn name(self) -> &'static str {
        match self {
            Section::Light => "light",
            Section::WhiteBalance => "white-balance",
            Section::Noise => "noise",
            Section::Detail => "detail",
            Section::Sharpen => "sharpen",
            Section::Curves => "curves",
            Section::Mixer => "mixer",
            Section::Color => "color",
            Section::BlackWhite => "black-and-white",
            Section::Grading => "grading",
            Section::Tint => "tint",
            Section::Look => "look",
            Section::Lens => "lens",
            Section::Vignette => "vignette",
            Section::Grain => "grain",
            Section::Demosaic => "demosaic",
            Section::Camera => "camera",
            Section::Adjustments => "adjustments",
        }
    }

    /// The panel's word for it.
    pub fn title(self) -> &'static str {
        match self {
            Section::Light => "Light",
            Section::WhiteBalance => "White balance",
            Section::Noise => "Noise",
            Section::Detail => "Detail",
            Section::Sharpen => "Sharpen",
            Section::Curves => "Curves",
            Section::Mixer => "Color mixer",
            Section::Color => "Color",
            Section::BlackWhite => "Black and white",
            Section::Grading => "Color grading",
            Section::Tint => "Color tint",
            Section::Look => "Look",
            Section::Lens => "Lens",
            Section::Vignette => "Vignette",
            Section::Grain => "Grain",
            Section::Demosaic => "Demosaic",
            Section::Camera => "Camera profile",
            Section::Adjustments => "Adjustments",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.name() == name)
    }

    /// Whether a preset saved without a say carries it: the look and
    /// the finish, not what is one picture's own (its white, its lens,
    /// its demosaic) nor its masks.
    pub fn by_default(self) -> bool {
        !matches!(
            self,
            Section::WhiteBalance
                | Section::Lens
                | Section::Demosaic
                | Section::Camera
                | Section::Adjustments
        )
    }

    /// Lay this section of `from` over `onto`.
    pub fn copy(self, from: &Edit, onto: &mut Edit) {
        match self {
            Section::Light => onto.light = from.light,
            Section::WhiteBalance => onto.white_balance = from.white_balance.clone(),
            // The profiled denoiser only: `learned` and
            // `learned_strength` are the file's own, defaulted from
            // its ISO (`Noise::blend_for_iso`), not a look to carry
            // from picture to picture, so a preset never touches them.
            Section::Noise => {
                onto.noise.enabled = from.noise.enabled;
                onto.noise.profiled = from.noise.profiled;
                onto.noise.strength = from.noise.strength;
            }
            Section::Detail => onto.detail = from.detail,
            Section::Sharpen => onto.sharpen = from.sharpen,
            Section::Curves => onto.curves = from.curves.clone(),
            Section::Mixer => onto.mixer = from.mixer,
            Section::Color => onto.color = from.color,
            Section::BlackWhite => onto.bw = from.bw,
            Section::Grading => onto.grading = from.grading,
            Section::Tint => onto.tint = from.tint,
            Section::Look => onto.look_lut = from.look_lut.clone(),
            Section::Lens => onto.lens = from.lens,
            Section::Vignette => onto.vignette = from.vignette,
            Section::Grain => onto.grain = from.grain,
            Section::Demosaic => onto.demosaic = from.demosaic,
            Section::Camera => onto.camera = from.camera.clone(),
            Section::Adjustments => {
                // The preset's masks in place of the picture's, with
                // ids of their own.
                onto.adjustments = from
                    .adjustments
                    .iter()
                    .enumerate()
                    .map(|(i, a)| crate::Adjustment {
                        id: i as u64 + 1,
                        ..a.clone()
                    })
                    .collect();
            }
        }
    }

    /// Whether this section reads the same in both edits.
    pub fn same(self, a: &Edit, b: &Edit) -> bool {
        match self {
            Section::Light => a.light == b.light,
            Section::WhiteBalance => a.white_balance == b.white_balance,
            // As `copy`: the learned tier and its blend are not part
            // of what a preset carries or compares.
            Section::Noise => {
                a.noise.enabled == b.noise.enabled
                    && a.noise.profiled == b.noise.profiled
                    && a.noise.strength == b.noise.strength
            }
            Section::Detail => a.detail == b.detail,
            Section::Sharpen => a.sharpen == b.sharpen,
            Section::Curves => a.curves == b.curves,
            Section::Mixer => a.mixer == b.mixer,
            Section::Color => a.color == b.color,
            Section::BlackWhite => a.bw == b.bw,
            Section::Grading => a.grading == b.grading,
            Section::Tint => a.tint == b.tint,
            Section::Look => a.look_lut == b.look_lut,
            Section::Lens => a.lens == b.lens,
            Section::Vignette => a.vignette == b.vignette,
            Section::Grain => a.grain == b.grain,
            Section::Demosaic => a.demosaic == b.demosaic,
            Section::Camera => a.camera == b.camera,
            Section::Adjustments => a.adjustments == b.adjustments,
        }
    }
}

/// A preset file's extension.
pub const EXTENSION: &str = "gcp";
/// The file in a store that lists the shipped presets put there.
const SEEDED: &str = "seeded";

/// A named part of an edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    /// The schema version of the edit inside.
    pub version: u32,
    pub name: String,
    /// What it carries, in [`Section::ALL`]'s order, each once.
    pub sections: Vec<Section>,
    /// The carried sections' settings; the rest at their defaults.
    pub edit: Edit,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            version: VERSION,
            name: String::new(),
            sections: Vec::new(),
            edit: Edit::default(),
        }
    }
}

impl Preset {
    /// The chosen sections of `edit`, under a name. What is not
    /// chosen is not written.
    pub fn from_edit(name: &str, edit: &Edit, sections: &[Section]) -> Self {
        let sections: Vec<Section> = Section::ALL
            .iter()
            .copied()
            .filter(|s| sections.contains(s))
            .collect();
        let mut kept = Edit::default();
        for s in &sections {
            s.copy(edit, &mut kept);
        }
        Self {
            version: VERSION,
            name: name.trim().to_string(),
            sections,
            edit: kept,
        }
    }

    /// Lay the carried sections over `onto`.
    pub fn apply(&self, onto: &mut Edit) {
        for s in &self.sections {
            s.copy(&self.edit, onto);
        }
    }

    /// `edit` with the preset laid over it.
    pub fn applied(&self, edit: &Edit) -> Edit {
        let mut out = edit.clone();
        self.apply(&mut out);
        out
    }

    /// Whether `edit` already has every carried section as the
    /// preset has it.
    pub fn is_applied(&self, edit: &Edit) -> bool {
        self.sections.iter().all(|s| s.same(&self.edit, edit))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a preset serializes")
    }

    /// Read a preset, the edit inside migrated to this build's schema.
    pub fn from_json(json: &str) -> Result<Self> {
        let mut value: serde_json::Value = serde_json::from_str(json)?;
        if let Some(obj) = value.as_object_mut() {
            let version = obj.get("version").cloned();
            if let Some(mut edit) = obj.remove("edit") {
                // The edit's own version is the file's.
                if let (Some(v), Some(e)) = (version, edit.as_object_mut())
                    && !e.contains_key("version")
                {
                    e.insert("version".into(), v);
                }
                obj.insert("edit".into(), migrate(edit)?);
            }
            obj.insert("version".into(), VERSION.into());
        }
        let mut preset: Preset = serde_json::from_value(value)?;
        preset.sections = Section::ALL
            .iter()
            .copied()
            .filter(|s| preset.sections.contains(s))
            .collect();
        Ok(preset)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut preset = Self::from_json(&std::fs::read_to_string(path)?)?;
        if preset.name.is_empty() {
            preset.name = stem_name(path);
        }
        Ok(preset)
    }

    /// Write the preset, whole, through a temporary file.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Read a preset file of either kind: Lightroom's `.xmp`, or this
/// engine's; the name from inside, or the file's stem.
pub fn import(path: &Path) -> Result<crate::lightroom::Imported> {
    let text = std::fs::read_to_string(path)?;
    if path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("xmp"))
    {
        return crate::lightroom::read(&text, &stem_name(path));
    }
    let mut preset = Preset::from_json(&text)?;
    if preset.name.is_empty() {
        preset.name = stem_name(path);
    }
    Ok(crate::lightroom::Imported {
        preset,
        unmapped: Vec::new(),
    })
}

/// A file's stem as a name.
fn stem_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Preset".into())
}

/// A preset's file name from its name: the letters and digits, in
/// lower case, runs of anything else a dash.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.extend(c.to_lowercase());
        } else {
            dash = true;
        }
    }
    if out.is_empty() {
        out.push_str("preset");
    }
    out
}

/// A preset as found in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub preset: Preset,
}

/// Where the presets live: a directory of `.gcp` files.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// `$XDG_CONFIG_HOME/greycard/presets` when that is set, else
    /// `greycard/presets` under the platform's configuration
    /// directory: `~/.config` on Linux, `~/Library/Application
    /// Support` on macOS, `%APPDATA%` on Windows.
    pub fn user() -> Option<Self> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(dirs::config_dir)?;
        Some(Self::at(base.join("greycard").join("presets")))
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where a preset of this name is kept.
    pub fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.{EXTENSION}", slug(name)))
    }

    /// Every preset in the store, by name; a file that will not read
    /// is passed over with a word on stderr.
    pub fn list(&self) -> Vec<Entry> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Entry> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == EXTENSION))
            .filter_map(|path| match Preset::load(&path) {
                Ok(preset) => Some(Entry { path, preset }),
                Err(e) => {
                    log::warn!("preset {}: {e}", path.display());
                    None
                }
            })
            .collect();
        out.sort_by(|a, b| {
            a.preset
                .name
                .to_lowercase()
                .cmp(&b.preset.name.to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
        });
        out
    }

    /// The preset called `name`, case aside; or, for a path that is a
    /// file, that file.
    pub fn find(&self, name: &str) -> Option<Entry> {
        let path = Path::new(name);
        if path.is_file() {
            return Preset::load(path).ok().map(|preset| Entry {
                path: path.to_path_buf(),
                preset,
            });
        }
        let wanted = name.trim().to_lowercase();
        self.list()
            .into_iter()
            .find(|e| e.preset.name.to_lowercase() == wanted)
    }

    /// Write `preset` under its name, over one of the same name.
    pub fn save(&self, preset: &Preset) -> Result<PathBuf> {
        if preset.name.trim().is_empty() {
            return Err(Error::Preset("a preset needs a name".into()));
        }
        // A file that holds the name already, whatever it is called.
        let path = self
            .list()
            .into_iter()
            .find(|e| e.preset.name.eq_ignore_ascii_case(preset.name.trim()))
            .map(|e| e.path)
            .unwrap_or_else(|| self.path_for(&preset.name));
        preset.save_to(&path)?;
        Ok(path)
    }

    pub fn remove(&self, entry: &Entry) -> Result<()> {
        Ok(std::fs::remove_file(&entry.path)?)
    }

    /// Put the film presets this build ships in the store, each one
    /// once.
    ///
    /// A store that was there before a preset shipped gets it too (a
    /// user with presets of their own is the common case, not the
    /// exception), so the record of what has been seeded is not the
    /// directory's existence but a `seeded` file in it listing the
    /// stems put there so far. A stem on that list is never written
    /// again: one edited stays edited and one binned stays binned.
    /// Nothing is overwritten, and a failure is a word in the log, not
    /// an error — a preset that could not be written is not a reason
    /// for the editor not to start.
    pub fn seed(&self) {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            log::warn!("presets {}: {e}", self.dir.display());
            return;
        }
        let record = self.dir.join(SEEDED);
        let mut seeded: Vec<String> = std::fs::read_to_string(&record)
            .map(|t| {
                t.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let mut changed = false;
        for (stem, text) in SHIPPED {
            if seeded.iter().any(|s| s == stem) {
                continue;
            }
            let path = self.dir.join(format!("{stem}.{EXTENSION}"));
            if path.exists() {
                // Something of that name is there already: the
                // user's, whatever it holds.
            } else if let Err(e) = std::fs::write(&path, text) {
                log::warn!("preset {}: {e}", path.display());
                continue;
            }
            seeded.push((*stem).to_string());
            changed = true;
        }
        if changed {
            let text = seeded.join("\n") + "\n";
            if let Err(e) = std::fs::write(&record, text) {
                log::warn!("presets {}: {e}", record.display());
            }
        }
    }
}

/// The film presets this build ships, by file stem.
///
/// Every one of them is built from the parametric controls the editor
/// already has — the tone shifts, the parametric curve, the mixer, the
/// grading wheels, the black and white, the grain — so none needs a
/// LUT file to exist and every number in them stays a slider the user
/// can move. Honest names, not the makers' trademarks (notes §78):
///
/// - **Muted Slide**, a transparency look: saturation out of the
///   picture, a hard shoulder on the highlights, crushed blacks and
///   cyan shadows.
/// - **Warm Negative**, a color negative scanned and left warm: soft
///   contrast, a matte black from the point curve, a warm shadow and
///   highlight wheel, fine grain.
/// - **Red-Filter Mono**, a panchromatic black and white through a
///   red filter: a dark sky, light skin, coarse grain.
pub const SHIPPED: [(&str, &str); 3] = [
    ("muted-slide", include_str!("../presets/muted-slide.gcp")),
    (
        "red-filter-mono",
        include_str!("../presets/red-filter-mono.gcp"),
    ),
    (
        "warm-negative",
        include_str!("../presets/warm-negative.gcp"),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Learned, WhiteBalance};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("greycard-preset-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Every shipped preset reads, says what it carries, and changes
    /// the picture the way its name claims. They are checked here
    /// rather than trusted because they are hand-written JSON: a
    /// misspelt key is silently a default.
    #[test]
    fn the_shipped_presets_are_what_they_say() {
        let by_name = |stem: &str| {
            let (_, text) = SHIPPED.iter().find(|(s, _)| *s == stem).expect("shipped");
            Preset::from_json(text).expect("it reads")
        };

        let slide = by_name("muted-slide");
        assert_eq!(slide.name, "Muted Slide");
        assert!(slide.sections.contains(&Section::Color));
        // It carries nothing it does not list, and lists nothing it
        // does not change.
        for preset in SHIPPED.map(|(_, t)| Preset::from_json(t).unwrap()) {
            let bare = Edit::default();
            for s in Section::ALL {
                let changed = !s.same(&preset.edit, &bare);
                assert_eq!(
                    changed,
                    preset.sections.contains(s),
                    "{}: {} is {}",
                    preset.name,
                    s.name(),
                    if changed {
                        "set but not listed"
                    } else {
                        "listed but not set"
                    }
                );
            }
            // None of them is a LUT: a film preset here is the
            // parametric controls, so it needs no file to exist.
            assert!(preset.edit.look_lut.lut.is_none(), "{}", preset.name);
        }

        // No preset pulls the black point up into the picture. A
        // negative `blacks` is a clip, not a shade — -0.12 puts it at
        // 41 of 255 and once took half a frame to pure black — and
        // where the black sits is a per-picture decision a shipped
        // preset has no business making. A positive one opens the
        // shadows and clips nothing; a faded negative's matte black is
        // the point curve's, as Warm Negative has it.
        //
        // What each preset does to a picture beyond that is measured
        // on the render, in `greycard-ui`'s `shipped_presets`: the
        // signs of the sliders are not the thing, and reading them was
        // what let a "Muted" preset raise saturation.
        for preset in SHIPPED.map(|(_, t)| Preset::from_json(t).unwrap()) {
            assert!(
                preset.edit.light.tone.blacks >= 0.0,
                "{} pulls the black point up, to {}",
                preset.name,
                preset.edit.light.tone.blacks
            );
        }

        let negative = by_name("warm-negative");
        assert_eq!(negative.name, "Warm Negative");
        assert!(!negative.edit.grain.is_off());

        // Red-Filter Mono is mono, and through a red filter: the one
        // thing here that its name states outright.
        let mono = by_name("red-filter-mono");
        assert_eq!(mono.name, "Red-Filter Mono");
        assert!(mono.edit.bw.enabled);
        assert_eq!(mono.edit.bw.filter(), Some(crate::bw::Filter::Red));
        assert!(!mono.edit.grain.is_off());
    }

    /// Every shipped preset is seeded once, into a new store and into
    /// one the user already had, and a binned one stays binned.
    #[test]
    fn the_shipped_presets_are_seeded_once_each() {
        let d = dir("seed");
        let store = Store::at(d.clone());
        store.seed();
        let names: Vec<String> = store.list().into_iter().map(|e| e.preset.name).collect();
        assert_eq!(names.len(), SHIPPED.len());
        assert!(names.contains(&"Muted Slide".to_string()));

        // Bin one and seed again: it is on the record, so it is not
        // put back.
        let gone = store.find("Muted Slide").unwrap();
        store.remove(&gone).unwrap();
        store.seed();
        assert_eq!(store.list().len(), SHIPPED.len() - 1);

        // A store that existed before anything shipped, with a preset
        // of the user's own in it, gets the shipped ones beside it;
        // and a user's file under a shipped name is left alone.
        let d2 = dir("seed-existing");
        let store2 = Store::at(d2.clone());
        let mut edit = Edit::default();
        edit.light.exposure = 0.3;
        store2
            .save(&Preset::from_edit("Mine", &edit, &[Section::Light]))
            .unwrap();
        std::fs::write(
            store2.dir().join("warm-negative.gcp"),
            r#"{"name": "Warm Negative", "sections": ["light"], "edit": {"light": {"exposure": 1.0}}}"#,
        )
        .unwrap();
        store2.seed();
        let names: Vec<String> = store2.list().into_iter().map(|e| e.preset.name).collect();
        assert_eq!(names.len(), SHIPPED.len() + 1);
        assert!(names.contains(&"Mine".to_string()));
        let theirs = store2.find("Warm Negative").unwrap();
        assert_eq!(
            theirs.preset.edit.light.exposure, 1.0,
            "the user's file stands"
        );
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn a_preset_carries_its_sections_and_no_more() {
        let mut edit = Edit::default();
        edit.light.exposure = 1.5;
        edit.vignette.amount = -1.0;
        edit.white_balance = WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.0,
        };
        edit.noise.learned = Learned::Best;
        let preset = Preset::from_edit(
            " Warm ",
            &edit,
            &[Section::Vignette, Section::Light, Section::Light],
        );
        assert_eq!(preset.name, "Warm");
        // In the panel's order, each once.
        assert_eq!(preset.sections, vec![Section::Light, Section::Vignette]);
        assert_eq!(preset.edit.light.exposure, 1.5);
        assert_eq!(preset.edit.vignette.amount, -1.0);
        assert_eq!(
            preset.edit.white_balance,
            WhiteBalance::AsShot,
            "not chosen"
        );
        assert_eq!(preset.edit.noise, crate::Noise::default());

        // Laid over another edit, the sections replace and the rest stays.
        let mut other = Edit::default();
        other.light.exposure = -2.0;
        other.grain.amount = 0.3;
        other.white_balance = WhiteBalance::Custom {
            temperature: 6500.0,
            tint: 0.01,
        };
        assert!(!preset.is_applied(&other));
        let out = preset.applied(&other);
        assert_eq!(out.light.exposure, 1.5);
        assert_eq!(out.vignette.amount, -1.0);
        assert_eq!(out.grain.amount, 0.3);
        assert!(
            matches!(out.white_balance, WhiteBalance::Custom { temperature, .. } if temperature == 6500.0)
        );
        assert!(preset.is_applied(&out));

        // The JSON says what is carried, and reads back.
        let json = preset.to_json();
        assert!(json.contains("\"vignette\""), "{json}");
        assert_eq!(Preset::from_json(&json).unwrap(), preset);
    }

    #[test]
    fn the_noise_section_carries_the_profiled_pass_not_the_learned_blend() {
        // A picture edited with the learned denoiser on, at whatever
        // blend its ISO gave it.
        let mut edit = Edit::default();
        edit.noise.profiled = true;
        edit.noise.strength = 1.8;
        edit.noise.learned = Learned::Best;
        edit.noise.learned_strength = 0.35;
        let preset = Preset::from_edit("Denoise", &edit, &[Section::Noise]);
        // The profiled pass comes across; the learned tier and its
        // blend do not, since they are the file's own (its ISO), not
        // the preset's.
        assert!(preset.edit.noise.profiled);
        assert_eq!(preset.edit.noise.strength, 1.8);
        assert_eq!(preset.edit.noise.learned, crate::Noise::default().learned);
        assert_eq!(
            preset.edit.noise.learned_strength,
            crate::Noise::default().learned_strength
        );

        // Laid over a file whose blend already follows its own ISO,
        // with its own tier chosen, applying the preset leaves both
        // alone.
        let mut other = Edit::default();
        other.noise.learned = Learned::Balanced;
        other.noise.learned_strength = crate::Noise::blend_for_iso(Some(100));
        let out = preset.applied(&other);
        assert!(out.noise.profiled);
        assert_eq!(out.noise.strength, 1.8);
        assert_eq!(out.noise.learned, Learned::Balanced, "the file's own tier");
        assert_eq!(
            out.noise.learned_strength,
            crate::Noise::blend_for_iso(Some(100)),
            "the file's own ISO blend, not stamped by the preset"
        );

        // The save sheet's "changed from default" check agrees: a
        // fresh file's ISO-driven blend alone does not make the
        // Noise section look worth saving.
        let mut iso_only = Edit::default();
        iso_only.noise.learned_strength = crate::Noise::blend_for_iso(Some(100));
        assert!(Section::Noise.same(&iso_only, &Edit::default()));
    }

    #[test]
    fn adjustments_come_with_fresh_ids() {
        let mut edit = Edit::default();
        for id in [7, 9] {
            edit.adjustments.push(crate::Adjustment {
                id,
                name: format!("Mask {id}"),
                ..Default::default()
            });
        }
        // A mask with a shape switched off carries the switch: the
        // preset lays down the mask as it was edited, not as it would
        // read with every shape on.
        edit.adjustments[0].mask.components = vec![
            crate::mask::Component::default(),
            crate::mask::Component {
                mode: crate::mask::Mode::Subtract,
                enabled: false,
                ..Default::default()
            },
        ];
        let preset = Preset::from_edit("Masks", &edit, &[Section::Adjustments]);
        let mut onto = Edit::default();
        onto.adjustments.push(crate::Adjustment {
            id: 3,
            name: "Old".into(),
            ..Default::default()
        });
        preset.apply(&mut onto);
        let ids: Vec<u64> = onto.adjustments.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(onto.adjustments[1].name, "Mask 9");
        assert_eq!(onto.next_id(), 3);
        let carried = &onto.adjustments[0].mask.components;
        assert!(carried[0].enabled);
        assert!(!carried[1].enabled);
    }

    #[test]
    fn an_old_presets_edit_is_migrated() {
        // Version 1's noise.enabled was the profiled denoiser's.
        let old = r#"{"version": 1, "name": "Clean", "sections": ["noise"],
                      "edit": {"noise": {"enabled": true, "strength": 2.0}}}"#;
        let p = Preset::from_json(old).unwrap();
        assert_eq!(p.version, VERSION);
        assert!(p.edit.noise.profiled);
        assert_eq!(p.edit.noise.strength, 2.0);
        // Unknown sections are dropped, and unknown fields ignored.
        let odd = r#"{"name": "X", "sections": ["light"], "edit": {}, "color": 3}"#;
        assert_eq!(
            Preset::from_json(odd).unwrap().sections,
            vec![Section::Light]
        );
        assert!(Preset::from_json(r#"{"version": 99, "edit": {"version": 99}}"#).is_err());
    }

    #[test]
    fn section_names_round_trip() {
        for s in Section::ALL {
            assert_eq!(Section::from_name(s.name()), Some(*s));
            assert_eq!(
                serde_json::to_string(s).unwrap(),
                format!("\"{}\"", s.name())
            );
        }
        let on: Vec<_> = Section::ALL.iter().filter(|s| s.by_default()).collect();
        assert_eq!(on.len(), 13);
        // A film preset is a look and a strength, so the look comes
        // across by default as the rest of the look does.
        assert!(on.contains(&&Section::Look));
    }

    #[test]
    fn import_reads_either_kind() {
        let d = dir("import");
        std::fs::create_dir_all(&d).unwrap();
        let xmp = d.join("Faded.XMP");
        std::fs::write(&xmp, r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+1.0" crs:Dehaze="+20"/></rdf:RDF></x:xmpmeta>"#).unwrap();
        let got = import(&xmp).unwrap();
        assert_eq!(got.preset.name, "Faded");
        assert_eq!(got.preset.edit.light.exposure, 1.0);
        assert!((got.preset.edit.detail.dehaze - 0.2).abs() < 1e-6);
        assert_eq!(got.preset.sections, vec![Section::Light, Section::Detail]);
        assert!(got.unmapped.is_empty());
        let gcp = d.join("Mine.gcp");
        std::fs::write(
            &gcp,
            r#"{"sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        let got = import(&gcp).unwrap();
        assert_eq!(got.preset.name, "Mine");
        assert_eq!(got.preset.edit.grain.amount, 0.2);
        assert!(got.unmapped.is_empty());
        assert!(import(&d.join("none.gcp")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn slugs_are_file_names() {
        assert_eq!(slug("Warm Film, v2"), "warm-film-v2");
        assert_eq!(slug("  --  "), "preset");
        assert_eq!(slug("Été/été"), "été-été");
    }

    #[test]
    fn the_store_lists_saves_finds_and_removes() {
        let store = Store::at(dir("store"));
        assert!(store.list().is_empty(), "no directory is no presets");
        let mut edit = Edit::default();
        edit.light.exposure = 0.3;
        let warm = Preset::from_edit("Warm", &edit, &[Section::Light]);
        let path = store.save(&warm).unwrap();
        assert!(path.ends_with("warm.gcp"), "{path:?}");
        edit.grain.amount = 0.5;
        let grainy = Preset::from_edit("a grainy one", &edit, &[Section::Grain]);
        store.save(&grainy).unwrap();
        // A file of a name of its own, with no name inside.
        std::fs::write(
            store.dir().join("odd-name.gcp"),
            r#"{"sections": ["grain"], "edit": {"grain": {"amount": 0.1}}}"#,
        )
        .unwrap();
        std::fs::write(store.dir().join("junk.gcp"), "not json").unwrap();
        std::fs::write(store.dir().join("notes.txt"), "nor a preset").unwrap();
        let names: Vec<_> = store.list().into_iter().map(|e| e.preset.name).collect();
        assert_eq!(names, vec!["a grainy one", "odd-name", "Warm"]);

        assert_eq!(store.find("warm").unwrap().preset, warm);
        assert!(store.find("cool").is_none());
        assert_eq!(store.find(path.to_str().unwrap()).unwrap().preset, warm);
        // Saved again under the name, the same file is written.
        let warmer = Preset::from_edit("WARM", &edit, &[Section::Light, Section::Grain]);
        assert_eq!(store.save(&warmer).unwrap(), path);
        assert_eq!(store.list().len(), 3);
        assert_eq!(store.find("Warm").unwrap().preset.sections.len(), 2);
        assert!(store.save(&Preset::default()).is_err(), "a name is needed");

        let entry = store.find("odd-name").unwrap();
        store.remove(&entry).unwrap();
        assert_eq!(store.list().len(), 2);
        std::fs::remove_dir_all(store.dir()).unwrap();
    }
}
