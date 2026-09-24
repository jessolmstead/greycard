//! The export sheet's choices as the panel names them, and export
//! presets: a set of those choices kept by name.
//!
//! An export preset is not a develop preset (`greycard_edit::preset`):
//! it says how a picture is written, not how it looks, so it lives in
//! the settings file beside the sheet's last choices rather than in a
//! sidecar or the preset directory.

use crate::export::{self, Metadata, OnExists, Sharpen, Space};
use crate::watermark::{Kind, Mark, Position};
use serde::{Deserialize, Serialize};

/// The watermark's kinds by the sheet's names.
pub const MARK_OFF: &str = "Off";
pub const MARK_TEXT: &str = "Text";
pub const MARK_IMAGE: &str = "Image";

/// Everything the export sheet asks, by the names it shows. The field
/// names in the file are the ones the settings have always used for
/// the sheet, `export_` and all, so a settings file from before
/// presets still reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sheet {
    #[serde(rename = "export_format")]
    pub format: String,
    #[serde(rename = "export_quality")]
    pub quality: f32,
    #[serde(rename = "export_size")]
    pub size: String,
    /// The long edge typed for a custom size, as typed.
    #[serde(rename = "export_custom")]
    pub custom: String,
    #[serde(rename = "export_space")]
    pub space: String,
    #[serde(rename = "export_embed")]
    pub embed: bool,
    #[serde(rename = "export_sharpen")]
    pub sharpen: String,
    /// What an export does when a file of that name is already there:
    /// Increment, Overwrite or Skip.
    #[serde(rename = "export_on_exists")]
    pub on_exists: String,
    /// All, No edit or None.
    #[serde(rename = "export_metadata")]
    pub metadata: String,
    /// Off, Text or Image.
    #[serde(rename = "export_mark")]
    pub mark: String,
    #[serde(rename = "export_mark_text")]
    pub mark_text: String,
    /// The PNG's path.
    #[serde(rename = "export_mark_image")]
    pub mark_image: String,
    /// White or Black, for text.
    #[serde(rename = "export_mark_color")]
    pub mark_color: String,
    /// One of the nine by `watermark::Position::name`.
    #[serde(rename = "export_mark_position")]
    pub mark_position: String,
    /// The mark's width, percent of the export's long edge.
    #[serde(rename = "export_mark_size")]
    pub mark_size: f32,
    /// The margin, percent of the export's long edge.
    #[serde(rename = "export_mark_margin")]
    pub mark_margin: f32,
    /// Percent.
    #[serde(rename = "export_mark_opacity")]
    pub mark_opacity: f32,
}

impl Default for Sheet {
    fn default() -> Self {
        Self {
            format: export::Format::Jpeg.name().into(),
            quality: 92.0,
            size: "Full".into(),
            custom: "1600".into(),
            space: Space::Srgb.name().into(),
            embed: true,
            sharpen: Sharpen::default().name().into(),
            on_exists: OnExists::default().name().into(),
            metadata: Metadata::default().name().into(),
            mark: MARK_OFF.into(),
            mark_text: String::new(),
            mark_image: String::new(),
            mark_color: "White".into(),
            mark_position: Position::default().name().into(),
            mark_size: 15.0,
            mark_margin: 2.0,
            mark_opacity: 60.0,
        }
    }
}

impl Sheet {
    /// The export these choices ask for. A name from another version
    /// is the default's. A text or image mark with nothing to draw is
    /// still asked for, so the export refuses it rather than write the
    /// picture unmarked (`Mark::check`).
    pub fn settings(&self) -> export::Settings {
        let defaults = export::Settings::default();
        export::Settings {
            format: export::Format::from_name(&self.format).unwrap_or(defaults.format),
            quality: self.quality.round().clamp(1.0, 100.0) as u8,
            long_edge: export::long_edge(&self.size, &self.custom),
            space: Space::from_name(&self.space).unwrap_or(defaults.space),
            embed_profile: self.embed,
            sharpen: Sharpen::from_name(&self.sharpen).unwrap_or(defaults.sharpen),
            metadata: Metadata::from_name(&self.metadata).unwrap_or_default(),
            watermark: self.watermark(),
        }
    }

    /// The sheet's answer to a file of that name being there already.
    pub fn on_exists(&self) -> OnExists {
        OnExists::from_name(&self.on_exists).unwrap_or_default()
    }

    fn watermark(&self) -> Option<Mark> {
        let kind = match self.mark.as_str() {
            MARK_TEXT => Kind::Text {
                text: self.mark_text.trim().to_string(),
                white: self.mark_color != "Black",
            },
            MARK_IMAGE => Kind::Image {
                path: self.mark_image.trim().into(),
            },
            _ => return None,
        };
        Some(Mark {
            kind,
            position: Position::from_name(&self.mark_position).unwrap_or_default(),
            size: self.mark_size.clamp(0.5, 100.0) / 100.0,
            margin: self.mark_margin.clamp(0.0, 50.0) / 100.0,
            opacity: self.mark_opacity.clamp(0.0, 100.0) / 100.0,
        })
    }

    /// The choices that make a difference, and only those: a custom
    /// edge under a named size, a quality for a lossless format and a
    /// watermark's settings with the watermark off change nothing, and
    /// the sliders are compared at the step they show.
    fn normalized(&self) -> Sheet {
        let step = |v: f32| (v * 10.0).round() / 10.0;
        let mut s = self.clone();
        s.quality = s.quality.round();
        if s.format != export::Format::Jpeg.name() {
            s.quality = 0.0;
        }
        if s.size != export::CUSTOM {
            s.custom.clear();
        }
        s.custom = s.custom.trim().to_string();
        s.mark_size = step(s.mark_size);
        s.mark_margin = step(s.mark_margin);
        s.mark_opacity = step(s.mark_opacity);
        match s.mark.as_str() {
            MARK_TEXT => s.mark_image.clear(),
            MARK_IMAGE => {
                s.mark_text.clear();
                s.mark_color.clear();
            }
            _ => {
                let off = Sheet::default();
                s.mark_text.clear();
                s.mark_image.clear();
                s.mark_color = off.mark_color;
                s.mark_position = off.mark_position;
                s.mark_size = off.mark_size;
                s.mark_margin = off.mark_margin;
                s.mark_opacity = off.mark_opacity;
            }
        }
        s
    }

    /// Whether the two write the same file.
    pub fn same(&self, other: &Sheet) -> bool {
        self.normalized() == other.normalized()
    }
}

/// A sheet kept by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    #[serde(flatten)]
    pub sheet: Sheet,
}

/// The picker's entry for no preset. No preset can take the name, in
/// any case.
pub const NO_PRESET: &str = "None";

/// Whether a name is the picker's own and not a preset's.
pub fn reserved(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case(NO_PRESET)
}

/// The preset of that name: exactly, else the one preset whose name
/// matches ignoring case, as a name typed on the command line may be.
/// Two that match only ignoring case are an error, not a guess; the
/// picker's None is never a preset.
pub fn lookup<'a>(presets: &'a [ExportPreset], name: &str) -> anyhow::Result<&'a ExportPreset> {
    let name = name.trim();
    let usable = || presets.iter().filter(|p| !reserved(&p.name));
    if let Some(p) = usable().find(|p| p.name == name) {
        return Ok(p);
    }
    let matches: Vec<&ExportPreset> = usable()
        .filter(|p| p.name.eq_ignore_ascii_case(name))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => {
            let known: Vec<&str> = usable().map(|p| p.name.as_str()).collect();
            anyhow::bail!(
                "no export preset {name:?}; the settings have {}",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            )
        }
        many => {
            let names: Vec<&str> = many.iter().map(|p| p.name.as_str()).collect();
            anyhow::bail!(
                "export preset {name:?} could be any of {}; give the name as written",
                names.join(", ")
            )
        }
    }
}

/// `lookup` for the sheet, which only needs to know.
pub fn find<'a>(presets: &'a [ExportPreset], name: &str) -> Option<&'a ExportPreset> {
    lookup(presets, name).ok()
}

/// Whether saving as `name` would replace a preset: one of that name
/// in any case.
pub fn exists(presets: &[ExportPreset], name: &str) -> bool {
    let name = name.trim();
    presets.iter().any(|p| p.name.eq_ignore_ascii_case(name))
}

/// Keep `sheet` as `name`: in the place of one of that name in any
/// case, which takes the name as now typed, else at the end. The name
/// is trimmed; an empty one, or the picker's None, keeps nothing.
pub fn save_as(presets: &mut Vec<ExportPreset>, name: &str, sheet: &Sheet) -> bool {
    let name = name.trim();
    if name.is_empty() || reserved(name) {
        return false;
    }
    let preset = ExportPreset {
        name: name.to_string(),
        sheet: sheet.clone(),
    };
    match presets
        .iter()
        .position(|p| p.name.eq_ignore_ascii_case(name))
    {
        Some(i) => {
            presets[i] = preset;
            // Any other left over in another case from before goes.
            let mut k = 0;
            presets.retain(|p| {
                k += 1;
                k - 1 == i || !p.name.eq_ignore_ascii_case(name)
            });
        }
        None => presets.push(preset),
    }
    true
}

/// Forget the preset of that name; whether there was one.
pub fn delete(presets: &mut Vec<ExportPreset>, name: &str) -> bool {
    let before = presets.len();
    presets.retain(|p| p.name != name);
    presets.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web() -> Sheet {
        Sheet {
            format: "JPEG".into(),
            quality: 85.0,
            size: "2048".into(),
            space: "sRGB".into(),
            metadata: "No edit".into(),
            mark: MARK_TEXT.into(),
            mark_text: "© J. O.".into(),
            mark_position: "Bottom right".into(),
            mark_size: 20.0,
            mark_opacity: 50.0,
            ..Sheet::default()
        }
    }

    #[test]
    fn a_sheet_is_the_export_it_names() {
        let s = web().settings();
        assert_eq!(s.format, export::Format::Jpeg);
        assert_eq!(s.quality, 85);
        assert_eq!(s.long_edge, Some(2048));
        assert_eq!(s.metadata, Metadata::NoEdit);
        let mark = s.watermark.expect("a mark");
        assert_eq!(
            mark.kind,
            Kind::Text {
                text: "© J. O.".into(),
                white: true
            }
        );
        assert_eq!(mark.position, Position::BottomRight);
        assert!((mark.size - 0.2).abs() < 1e-6 && (mark.opacity - 0.5).abs() < 1e-6);
        // A mark with nothing to draw is still asked for, and refused.
        let empty = Sheet {
            mark_text: "  ".into(),
            ..web()
        };
        let asked = empty.settings().watermark.expect("still a mark");
        assert!(asked.check().is_err());
        let no_png = Sheet {
            mark: MARK_IMAGE.into(),
            mark_image: String::new(),
            ..web()
        };
        let err = no_png.settings().watermark.unwrap().check().unwrap_err();
        assert!(err.to_string().contains("PNG"), "{err}");
        // Off is no mark.
        assert!(
            Sheet {
                mark: MARK_OFF.into(),
                ..web()
            }
            .settings()
            .watermark
            .is_none()
        );
        let image = Sheet {
            mark: MARK_IMAGE.into(),
            mark_image: "/x/logo.png".into(),
            ..web()
        };
        assert_eq!(
            image.settings().watermark.unwrap().kind,
            Kind::Image {
                path: "/x/logo.png".into()
            }
        );
        // Names from another version fall back.
        let odd = Sheet {
            format: "WebP".into(),
            metadata: "Some".into(),
            ..Sheet::default()
        };
        assert_eq!(odd.settings(), export::Settings::default());
    }

    #[test]
    fn only_a_change_that_writes_another_file_is_an_edit() {
        let a = web();
        assert!(a.same(&a.clone()));
        // A custom edge under a named size, the slider a hair off.
        let b = Sheet {
            custom: "999".into(),
            quality: 85.2,
            ..a.clone()
        };
        assert!(a.same(&b));
        // The image path with a text mark, and every mark field with
        // no mark.
        assert!(a.same(&Sheet {
            mark_image: "/elsewhere.png".into(),
            ..a.clone()
        }));
        let off = Sheet {
            mark: MARK_OFF.into(),
            ..a.clone()
        };
        assert!(off.same(&Sheet {
            mark_text: "other".into(),
            mark_size: 3.0,
            ..off.clone()
        }));
        // What does change the file.
        assert!(!a.same(&Sheet {
            quality: 86.0,
            ..a.clone()
        }));
        assert!(!a.same(&Sheet {
            mark_text: "other".into(),
            ..a.clone()
        }));
        assert!(!a.same(&Sheet {
            mark_position: "Top left".into(),
            ..a.clone()
        }));
        assert!(!a.same(&off));
    }

    #[test]
    fn presets_save_replace_and_delete_by_name() {
        let mut list = Vec::new();
        assert!(!save_as(&mut list, "  ", &web()));
        assert!(save_as(&mut list, " Web ", &web()));
        assert!(save_as(&mut list, "Print", &Sheet::default()));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Web");
        // Saved again under the same name: in its place.
        let smaller = Sheet {
            size: "1024".into(),
            ..web()
        };
        assert!(save_as(&mut list, "Web", &smaller));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].sheet, smaller);
        assert_eq!(
            find(&list, "web").map(|p| &p.name),
            Some(&"Web".to_string())
        );
        assert!(find(&list, "Nope").is_none());
        // Another case is the same preset: replaced, renamed.
        assert!(exists(&list, "WEB"));
        assert!(save_as(&mut list, "WEB", &web()));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "WEB");
        assert!(delete(&mut list, "WEB"));
        assert!(!delete(&mut list, "WEB"));
        assert_eq!(list.len(), 1);
        // The picker's None is no name, in any case.
        for none in ["None", "none", " NONE "] {
            assert!(!save_as(&mut list, none, &web()));
        }
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn a_name_is_found_once_or_not_at_all() {
        let named = |n: &str| ExportPreset {
            name: n.into(),
            sheet: Sheet::default(),
        };
        // An older file with a preset called none, and two that differ
        // only in case.
        let list = vec![named("none"), named("Web"), named("web"), named("Print")];
        assert!(find(&list, "None").is_none());
        assert!(find(&list, "none").is_none());
        assert_eq!(find(&list, "Web").unwrap().name, "Web");
        assert_eq!(find(&list, "web").unwrap().name, "web");
        assert_eq!(find(&list, "print").unwrap().name, "Print");
        let err = lookup(&list, "WEB").unwrap_err().to_string();
        assert!(err.contains("Web, web"), "{err}");
        let err = lookup(&list, "Nope").unwrap_err().to_string();
        assert!(
            err.contains("Web, web, Print") && !err.contains("none,"),
            "{err}"
        );
    }
}
