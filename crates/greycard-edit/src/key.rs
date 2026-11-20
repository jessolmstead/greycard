//! The develop key: a name for the picture an edit makes, for the
//! pictures kept of it (the thumbnails and the larger picture made from
//! the edit, notes on thumbnails from the edit).
//!
//! It is a hash of what makes the picture and of nothing else: the
//! edit itself, less what never reaches a pixel (an adjustment's name,
//! the schema version it was written at); a recipe number for the
//! pipeline, raised when the same edit comes to make another picture;
//! the version of the learned stages each of its Sky, Landscape and
//! People masks is made by ([`mask_pipelines`]), so a mask made again
//! by a newer rule makes the frames that have one again and no other;
//! the contents of every file outside the sidecar that the develop
//! reads by name: the look's table (a `.cube` or a HaldCLUT, the camera
//! match's fitted tables among them, and its variant for the edit's
//! display curve) and a DCP camera profile; and, when the edit corrects
//! the lens by its profile, the caller's name for the lens database it
//! has (this crate does not know the database). The files are read by
//! the caller and handed in as [`Read`]s: greycard-edit-io's `key`
//! reads them from the user's directories. A refit of a look
//! changes the key of every frame that uses it and of no other, and a
//! database fetched or updated changes the key of every frame whose
//! lens it may correct.
//!
//! The frame's own quarter turns (the sidecar's `turn`) are not in it:
//! the pictures kept under a key are kept as the camera's tag turns the
//! frame, with the frame's turns taken back out, and a turn moves the
//! crop and the masks inside the edit (`Edit::turn`), which the key
//! does see. So a frame turned in the strip keeps the pictures of its
//! edit, and they are turned at draw time like the camera's.
//!
//! The sidecar's history, its ids, its meta (a rating, a flag, the
//! keywords) and its exports are not in it, so a rating does not change
//! the key, and an undo and a redo back to the same edit give the key
//! they gave before.

use crate::Edit;
use crate::mask::{LandscapeGroup, Shape};

/// The pipeline's own part of the key: raise it when the develop or
/// the finish comes to make a different picture from the same edit (a
/// change to an op, a default the edit does not store, the shrink that
/// makes the kept pictures), and every picture kept under the old
/// number is never asked for again.
///
/// 2: a tint blends toward its hue rather than turning it, 2026-10-10.
/// 3: highlights by the frame's own range and whites pivoting at the
/// span's foot, 2026-10-13.
/// 4: a baseline per body (`baseline::for_body`), which the edit does
/// not store, 2026-10-17.
/// 5: a DNG's own warps and bad-pixel lists, 2026-11-01.
/// 6: the mosaic's noise line drawn past its bins of texture, which
/// the automatic dual demosaic threshold and the hot-pixel repair
/// read by default, 2026-11-15.
pub const DEVELOP_RECIPE: u32 = 6;

/// The capture sharpen's guide, by version: taken into the key of an
/// edit whose sharpen reads one ([`Edit::sharpen_reads_guide`]), so
/// the pictures kept of such an edit are made again when it changes,
/// and those of every other edit are kept. 1: what is flat is read off
/// the picture before Texture, Clarity, and Structure, 2026-11-12; an
/// edit with all three at zero makes the picture it made, to the bit.
pub const SHARPEN_GUIDE: &str = "sharpen-guide-1";

/// The Midtones bell's shape, by version: taken into the key of an edit
/// that sets Midtones ([`Edit::sets_midtones`]), so the pictures kept
/// of such an edit are made again when it changes, and those of every
/// other edit are kept. 1: full half a stop under the highlights span's
/// foot, reaching 1.75 stops either way, 2026-11-12.
pub const MIDTONES_SHAPE: &str = "midtones-1";

/// The whites down, by version: taken into the key of an edit with
/// Whites under zero, the picture's or a mask's
/// ([`Edit::sets_whites_down`]), so the pictures kept of such an edit
/// are made again when it changes, and those of every other edit are
/// kept: Whites at zero and up makes the picture it made, to the bit.
/// Keyed on the edit alone, because a key does not read pixels. A
/// Whites-down edit changes its picture wherever the frame reaches the
/// band under white the roll makes its room in, lights or none, and a
/// frame with nothing in the band is the bit it was: a grey 0.12
/// showed at 254 comes out 0.13, 0.48, 1.8, 5, and 10.9 levels darker
/// at Whites -0.25, -0.5, -1, -2, and -3.5 (Channels), and the band
/// starts where 0.12 showed 253, 252, 250, 246, and 237. 3: the
/// white point and a roll over white after it (issue 132), 2026-11-16,
/// past the pictures trial builds kept under 1 and 2.
pub const WHITES_SHAPE: &str = "whites-3";

/// The linear gradient's ramp, by version: taken into the key of an
/// edit with a linear gradient in some mask
/// ([`Edit::has_linear_gradient`]), so the pictures kept of such an
/// edit are made again when it changes, and those of every other edit
/// are kept. 1: Hermite's step, issue 182, 2026-11-18.
pub const LINEAR_RAMP: &str = "linear-ramp-1";

/// An aspect over no crop, by version: taken into the key of an edit
/// whose aspect frames the picture with no crop stored
/// ([`crate::geometry::Geometry::frames_by_aspect`]), so the pictures
/// kept of such an edit are made again and those of every other edit
/// are kept. 1: framed by the aspect as the viewport frames it, where
/// before the pictures were the whole frame, 2026-11-13.
pub const ASPECT_FRAME: &str = "aspect-frame-1";

/// The learned highlight fill, by its implementation's version: taken
/// into the key of an edit whose fill names a version this build has
/// ([`Edit::highlight_fill`]), so the pictures kept of such an edit are
/// made again when the op changes under one name, and those of every
/// other edit are kept. The version's name is in the edit's own JSON
/// already; this is the code that runs it. An edit naming a version
/// this build lacks develops without the fill and does not take it, so
/// a newer build that has that version keys it apart from the picture
/// kept here. 1: the first, 2026-11-19.
pub const HIGHLIGHT_FILL: &str = "highlight-fill-1";

/// The highlight fill's versions this build has, by the names an edit
/// writes: `greycard_ai::highlight::VERSIONS`' names, which this crate
/// cannot see; the engine's tests hold the two equal.
pub const HIGHLIGHT_FILLS: &[&str] = &["hl-1"];

/// The learned masks' stages, by version: raise one when its raster
/// comes to be another from the same shape. greycard-ui's mask cache
/// keys its rasters by these, and the develop key of an edit with such
/// a mask takes them in, so the pictures kept of it are made again.
///
/// The Sky's: the prior, the gate, SAM's outline and the color-line
/// matte. 2: the gate's core over the kept queries, 2026-10-17. 3: the
/// sure sky held up over a diluted map, and its pockets, 2026-10-17.
///
/// Not raised for the Sky's wide raster (`brush::raster_width`, twice
/// as wide on a frame past three of its pixels a texel), which every
/// frame under the rule does not take, to the bit. The frame's size is
/// not in the key, which is made from the sidecar alone (the index
/// makes it without a decode); the mask cache keeps a wide raster in a
/// slot of its own, so a frame past the rule makes its sky again, and
/// what the open frame, an export and every picture made from now on
/// read is the wide one. A picture kept already of such a frame stays
/// until the frame is opened, whose develop makes its own in its
/// place, or its edit changes. Read at that picture's 2048, the two
/// rasters differ by under a level on a 45 MP frame as shot; on a
/// 100 MP one upright by a seventh of a level on average and by up to
/// half the mask in a gap between twigs.
pub const SKY_PIPELINE: &str = "sky-3";
/// The landscape groups' gates and edges.
pub const LANDSCAPE_PIPELINE: &str = "landscape-2";
/// A People mask's routes and the people found.
pub const PART_PIPELINE: &str = "part-2";
/// A whole person's edge, Subject's once a Subject model is in.
pub const PERSON_EDGE: &str = "person-edge-1";

/// The learned stages `shape`'s raster is made by: Foliage is cut by
/// the found sky, so takes the sky's too; every People mask takes the
/// whole person's edge, which only a whole person reads, so a change
/// to that edge remakes the pictures of the other routes once as well.
pub fn mask_pipelines(shape: &Shape) -> &'static [&'static str] {
    match shape {
        Shape::Sky { .. } => &[SKY_PIPELINE],
        Shape::Landscape {
            group: LandscapeGroup::Foliage,
        } => &[LANDSCAPE_PIPELINE, SKY_PIPELINE],
        Shape::Landscape { .. } => &[LANDSCAPE_PIPELINE],
        Shape::Part { .. } => &[PART_PIPELINE, PERSON_EDGE],
        _ => &[],
    }
}

/// The key of an edit's picture on a frame whose camera framed less of
/// the picture than the raw keeps (`camera`, the index's and the
/// decode's [`greycard_core::raw::CameraFrame`]): `key`, the edit's own
/// ([`Edit::develop_key_with`]), with the frame taken in where it frames
/// the edit ([`crate::geometry::framed_by_camera`]: no crop stored,
/// `uncropped`, and not a ratio, `by_ratio`, over a centered frame,
/// which frames as with none). Every other key is `key` itself, so the
/// frames of every camera that frames nothing, every edit with a crop,
/// and a ratio over a centered frame keep their pictures.
pub fn framed_key(
    key: u64,
    uncropped: bool,
    by_ratio: bool,
    camera: Option<&greycard_core::raw::CameraFrame>,
) -> u64 {
    let Some(camera) = camera.filter(|c| {
        crate::geometry::framed_by_camera(
            uncropped,
            by_ratio,
            Some(crate::geometry::Crop::of(c.fractions())),
        )
    }) else {
        return key;
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"greycard camera frame\0");
    hasher.update(&key.to_le_bytes());
    hasher.update(camera.to_text().as_bytes());
    let hash = hasher.finalize();
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("eight bytes"))
}

/// The key of an edit's picture on a frame that may decode with light
/// over its white (`headroom`, by
/// [`greycard_core::decode::may_have_headroom`] of its file): `key`,
/// the edit's own ([`Edit::develop_key_with`]), with
/// [`greycard_core::decode::HEADROOM`] taken in. The key is made
/// without a decode, so every such file's pictures are made again when
/// the table changes, and those of every other file keep `key`. Taken
/// before [`framed_key`].
pub fn headroom_key(key: u64, headroom: bool) -> u64 {
    if !headroom {
        return key;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"greycard headroom\0");
    hasher.update(&key.to_le_bytes());
    hasher.update(greycard_core::decode::HEADROOM.as_bytes());
    let hash = hasher.finalize();
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("eight bytes"))
}

/// The key of an edit's picture on a frame that may carry a DNG
/// `LinearizationTable` (`linearization`, by
/// [`greycard_core::decode::may_have_linearization`] of its file):
/// `key` with [`greycard_core::decode::LINEARIZATION`] taken in, as
/// [`headroom_key`] takes the headroom in, so only such files'
/// pictures are made again when how the table is read changes. Taken
/// before [`framed_key`].
pub fn linearization_key(key: u64, linearization: bool) -> u64 {
    if !linearization {
        return key;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"greycard linearization\0");
    hasher.update(&key.to_le_bytes());
    hasher.update(greycard_core::decode::LINEARIZATION.as_bytes());
    let hash = hasher.finalize();
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("eight bytes"))
}

/// [`headroom_key`] and [`linearization_key`] of `key`, each where the
/// file at `path` may need it: the decode's part of a picture's key.
pub fn decode_key(key: u64, path: &std::path::Path) -> u64 {
    let key = headroom_key(key, greycard_core::decode::may_have_headroom(path));
    linearization_key(key, greycard_core::decode::may_have_linearization(path))
}

/// A file the develop of an edit reads outside the sidecar, as the key
/// takes it: its file name, and BLAKE3 of its contents, `None` for a
/// file that is not there or will not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub name: String,
    pub contents: Option<[u8; 32]>,
}

impl Edit {
    /// The key of the picture this edit makes, with `read` the files
    /// its develop reads by name (greycard-edit-io's `key` reads them
    /// from the user's directories). `lenses` names the lens database
    /// the caller develops with (where it was found and when it was
    /// made), `None` for none; it is in the key only while the edit
    /// corrects the lens by its profile. See the module.
    pub fn develop_key_with(&self, read: &[Read], lenses: Option<&str>) -> u64 {
        self.develop_key_by(read, lenses, |shape| {
            mask_pipelines(shape)
                .iter()
                .map(|v| v.to_string())
                .collect()
        })
    }

    /// [`Edit::develop_key_with`] with the masks' stages as
    /// `pipelines` names them: for the tests, which raise one.
    fn develop_key_by(
        &self,
        read: &[Read],
        lenses: Option<&str>,
        pipelines: impl Fn(&Shape) -> Vec<String>,
    ) -> u64 {
        // What reaches no pixel is taken out: the version the edit was
        // written at and whether it waits on its frame (a migration that
        // changes nothing changes no picture), an adjustment's name, and
        // the light a mask's
        // switched-off white balance keeps for when it goes on again.
        let mut edit = self.clone();
        edit.version = 0;
        edit.frame_pending = false;
        for a in &mut edit.adjustments {
            a.name.clear();
            a.look.white_balance = a.look.white_balance.acting();
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"greycard develop key\0");
        hasher.update(&DEVELOP_RECIPE.to_le_bytes());
        // Serde writes a struct's fields in their order and a list in
        // its, and nothing in an edit is a map, so equal edits write
        // equal bytes.
        let json = serde_json::to_vec(&edit).expect("an edit serializes");
        hasher.update(&(json.len() as u64).to_le_bytes());
        hasher.update(&json);
        // The learned masks' stages, each once, in order: only an edit
        // with such a mask takes them in.
        let stages: std::collections::BTreeSet<String> = self
            .adjustments
            .iter()
            .flat_map(|a| &a.mask.components)
            .flat_map(|c| pipelines(&c.shape))
            .collect();
        for stage in stages {
            hasher.update(b"mask stages\0");
            hasher.update(&(stage.len() as u64).to_le_bytes());
            hasher.update(stage.as_bytes());
        }
        if self.sharpen_reads_guide() {
            hasher.update(b"sharpen guide\0");
            hasher.update(&(SHARPEN_GUIDE.len() as u64).to_le_bytes());
            hasher.update(SHARPEN_GUIDE.as_bytes());
        }
        if self.sets_midtones() {
            hasher.update(b"midtones shape\0");
            hasher.update(&(MIDTONES_SHAPE.len() as u64).to_le_bytes());
            hasher.update(MIDTONES_SHAPE.as_bytes());
        }
        if self.sets_whites_down() {
            hasher.update(b"whites shape\0");
            hasher.update(&(WHITES_SHAPE.len() as u64).to_le_bytes());
            hasher.update(WHITES_SHAPE.as_bytes());
        }
        if self.has_linear_gradient() {
            hasher.update(b"linear ramp\0");
            hasher.update(&(LINEAR_RAMP.len() as u64).to_le_bytes());
            hasher.update(LINEAR_RAMP.as_bytes());
        }
        if self.geometry.frames_by_aspect() {
            hasher.update(b"aspect frame\0");
            hasher.update(&(ASPECT_FRAME.len() as u64).to_le_bytes());
            hasher.update(ASPECT_FRAME.as_bytes());
        }
        if self.highlight_fill().is_some() {
            hasher.update(b"highlight fill\0");
            hasher.update(&(HIGHLIGHT_FILL.len() as u64).to_le_bytes());
            hasher.update(HIGHLIGHT_FILL.as_bytes());
        }
        for Read { name, contents } in read {
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            match contents {
                Some(hash) => {
                    hasher.update(&[1]);
                    hasher.update(hash);
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
        // The database the profile corrections come from, while there are
        // any: one fetched, or updated, is another picture.
        if self.lens.enabled && self.lens.profile {
            let lenses = lenses.unwrap_or_default();
            hasher.update(b"lenses\0");
            hasher.update(&(lenses.len() as u64).to_le_bytes());
            hasher.update(lenses.as_bytes());
        }
        let hash = hasher.finalize();
        u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("eight bytes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sidecar;

    fn key(edit: &Edit, lenses: Option<&str>) -> u64 {
        edit.develop_key_with(&[], lenses)
    }

    /// A mask's Refine edge is another picture: the key moves with it,
    /// and back with it.
    #[test]
    fn a_masks_refine_edge_changes_the_key() {
        let mut edit = Edit::default();
        edit.adjustments.push(crate::Adjustment {
            id: 1,
            mask: crate::Mask {
                components: vec![crate::mask::Component {
                    shape: Shape::Subject {},
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        });
        let plain = key(&edit, None);
        edit.adjustments[0].mask.refine = 0.3;
        let refined = key(&edit, None);
        assert_ne!(plain, refined);
        edit.adjustments[0].mask.refine = 0.6;
        assert_ne!(key(&edit, None), refined);
        edit.adjustments[0].mask.refine = 0.0;
        assert_eq!(key(&edit, None), plain);
    }

    /// The sharpen's guide is in the key of an edit whose sharpen reads
    /// one, and of no other: Detail's Dehaze alone, or Texture with no
    /// sharpen to guide, keeps the key it had.
    #[test]
    fn the_sharpen_guide_is_keyed_only_where_the_sharpen_reads_one() {
        let mut edit = Edit::default();
        edit.detail.dehaze = 0.3;
        assert!(!edit.sharpen_reads_guide());
        edit.detail.texture = 0.5;
        assert!(edit.sharpen_reads_guide());
        edit.detail.texture = 0.0;
        edit.detail.clarity = -0.2;
        assert!(edit.sharpen_reads_guide());
        edit.detail.enabled = false;
        assert!(!edit.sharpen_reads_guide(), "the section off");
        edit.detail.enabled = true;
        edit.sharpen.enabled = false;
        assert!(!edit.sharpen_reads_guide(), "nothing to guide");
    }

    /// The Midtones bell's shape is in the key of an edit that sets
    /// Midtones, the picture's own or a mask's, and of no other.
    #[test]
    fn the_midtones_shape_is_keyed_only_where_midtones_is_set() {
        let mut edit = Edit::default();
        edit.light.tone.shadows = 0.5;
        assert!(!edit.sets_midtones());
        edit.light.tone.midtones = -0.25;
        assert!(edit.sets_midtones());
        edit.light.tone.midtones = 0.0;
        edit.adjustments.push(crate::Adjustment::default());
        assert!(!edit.sets_midtones());
        edit.adjustments[0].look.light.tone.midtones = 1.0;
        assert!(edit.sets_midtones(), "a mask's");
    }

    /// The whites' shape is in the key of an edit with Whites under
    /// zero, the picture's own or a mask's, and of no other: Whites up,
    /// Highlights alone, and a mask's Whites at zero leave the key as it
    /// was. Held against the key made without the shape, so the shape
    /// itself is what keys a Whites-down edit, not its Whites.
    #[test]
    fn the_whites_shape_is_keyed_only_where_whites_is_down() {
        let mut edit = Edit::default();
        edit.light.tone.whites = 0.5;
        edit.light.tone.shadows = 0.5;
        let up = key(&edit, None);
        assert_eq!(up, key_before_the_aspect_frame(&edit), "Whites up");
        edit.light.tone.whites = -0.25;
        assert_ne!(
            key(&edit, None),
            key_before_the_aspect_frame(&edit),
            "Whites down carries the shape"
        );
        edit.light.tone.whites = 0.5;
        assert!(!edit.sets_whites_down());
        edit.light.tone.highlights = -1.0;
        assert!(!edit.sets_whites_down(), "Highlights is master's");
        edit.light.tone.highlights = 0.0;
        edit.light.tone.whites = -0.25;
        assert!(edit.sets_whites_down());
        assert_ne!(key(&edit, None), up);
        edit.light.tone.whites = 0.5;
        edit.adjustments.push(crate::Adjustment::default());
        assert!(!edit.sets_whites_down());
        edit.adjustments[0].look.light.tone.whites = -1.0;
        assert!(edit.sets_whites_down(), "a mask's whites");
        edit.adjustments.clear();
        assert_eq!(key(&edit, None), up);
    }

    /// The linear ramp is in the key of an edit with a linear gradient
    /// in some mask, and of no other: a radial alone keeps the key it
    /// had, so the pictures kept of it stay.
    #[test]
    fn the_linear_ramp_is_keyed_only_where_a_mask_holds_a_linear_gradient() {
        use crate::mask::{Component, Shape};
        let kept = |edit: &Edit| key(edit, None) == key_before_the_aspect_frame(edit);
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        assert!(!edit.has_linear_gradient());
        let mut a = crate::Adjustment::default();
        a.look.light.exposure = 1.0;
        a.mask.components.push(Component {
            shape: Shape::Radial {
                center: [0.5, 0.5],
                radius: [0.2, 0.2],
                angle: 0.0,
                feather: 0.5,
            },
            ..Default::default()
        });
        edit.adjustments.push(a);
        assert!(!edit.has_linear_gradient());
        assert!(kept(&edit), "a radial alone");
        edit.adjustments[0].mask.components.push(Component {
            shape: Shape::Linear {
                from: [0.0, 0.0],
                to: [0.3, 0.2],
            },
            ..Default::default()
        });
        assert!(edit.has_linear_gradient());
        assert!(!kept(&edit), "a linear gradient carries the ramp");
        edit.adjustments[0].mask.components.remove(0);
        assert!(!kept(&edit), "a linear gradient alone");
    }

    /// The highlight fill off keeps every key as it was; a version this
    /// build has is another picture and carries [`HIGHLIGHT_FILL`]; a
    /// version it lacks develops without the fill but is in the edit's
    /// JSON, so its key is its own, made without the tag.
    #[test]
    fn the_highlight_fill_is_keyed_only_where_this_build_has_its_version() {
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        assert_eq!(edit.highlight_fill, None);
        let off = key(&edit, None);
        assert_eq!(off, key_before_the_aspect_frame(&edit), "off, as before");
        edit.highlight_fill = Some("hl-1".into());
        assert_eq!(edit.highlight_fill(), Some("hl-1"));
        let on = key(&edit, None);
        assert_ne!(on, off);
        assert_ne!(
            on,
            key_before_the_aspect_frame(&edit),
            "hl-1 carries the fill's version"
        );
        edit.highlight_fill = Some("hl-9".into());
        assert_eq!(edit.highlight_fill(), None, "a version this build lacks");
        let unknown = key(&edit, None);
        assert_ne!(unknown, off, "the name is in the edit's JSON");
        assert_ne!(unknown, on);
        assert_eq!(
            unknown,
            key_before_the_aspect_frame(&edit),
            "and the fill's version is not"
        );
        edit.highlight_fill = None;
        assert_eq!(key(&edit, None), off, "and off again");
    }

    /// The key as it was made before [`ASPECT_FRAME`], for an edit with
    /// no mask, no file read, no guide, and no Midtones: the recipe, the
    /// edit, and the lens database while it corrects. The default
    /// edit's key, equal to it, says it is made as the key is.
    fn key_before_the_aspect_frame(edit: &Edit) -> u64 {
        let mut stripped = edit.clone();
        stripped.version = 0;
        stripped.frame_pending = false;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"greycard develop key\0");
        hasher.update(&DEVELOP_RECIPE.to_le_bytes());
        let json = serde_json::to_vec(&stripped).expect("an edit serializes");
        hasher.update(&(json.len() as u64).to_le_bytes());
        hasher.update(&json);
        if edit.lens.enabled && edit.lens.profile {
            hasher.update(b"lenses\0");
            hasher.update(&0u64.to_le_bytes());
        }
        let bytes = hasher.finalize();
        u64::from_le_bytes(bytes.as_bytes()[..8].try_into().unwrap())
    }

    /// An aspect over no crop, whose kept pictures were the whole frame,
    /// is keyed anew so they are made again; the default edit, a crop
    /// stored, Original as the frame stands, and a tone alone keep the
    /// keys they had.
    #[test]
    fn the_aspect_frame_is_keyed_only_where_an_aspect_frames_with_no_crop() {
        let kept = |edit: &Edit| key(edit, None) == key_before_the_aspect_frame(edit);
        let mut edit = Edit::default();
        assert!(kept(&edit), "the default edit");
        edit.light.exposure = 0.5;
        assert!(kept(&edit), "a tone alone");
        edit.geometry.aspect = crate::geometry::Aspect::Original;
        assert!(kept(&edit), "Original, the whole plane");
        edit.geometry.aspect = crate::geometry::Aspect::Ratio { w: 3.0, h: 2.0 };
        assert!(!kept(&edit), "3:2 over no crop");
        edit.geometry.crop = Some(crate::geometry::Crop {
            x: 0.0,
            y: 0.1,
            w: 1.0,
            h: 0.8,
        });
        assert!(kept(&edit), "a crop stored");
        edit.geometry.crop = None;
        edit.geometry.aspect = crate::geometry::Aspect::Original;
        edit.geometry.portrait = true;
        assert!(!kept(&edit), "Original stood on end");
    }

    #[test]
    fn an_undo_and_a_redo_back_to_the_same_picture_give_the_same_key() {
        let mut sidecar = Sidecar::default();
        let before = key(&sidecar.current, None);
        let first = sidecar.current.clone();
        sidecar.current.light.exposure += 1.0;
        let brightened = key(&sidecar.current, None);
        assert_ne!(before, brightened, "a brightening is another picture");
        // Undo: the edit as it was, from the history's copy.
        sidecar.current = first.clone();
        assert_eq!(key(&sidecar.current, None), before);
        // Redo, by setting the same value again rather than by a copy.
        sidecar.current.light.exposure += 1.0;
        assert_eq!(key(&sidecar.current, None), brightened);
        // Through the sidecar's own file and back.
        let json = serde_json::to_string(&sidecar).unwrap();
        let read: Sidecar = serde_json::from_str(&json).unwrap();
        assert_eq!(key(&read.current, None), brightened);
    }

    #[test]
    fn a_rating_a_name_and_the_turn_change_no_key_and_the_crop_does() {
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = 0.7;
        let k = key(&sidecar.current, None);
        sidecar.meta.rating = 4;
        sidecar.meta.keywords.push("kept".into());
        assert_eq!(
            key(&sidecar.current, None),
            k,
            "a rating is not the picture"
        );
        sidecar.current.version = 1;
        assert_eq!(key(&sidecar.current, None), k, "nor is the schema version");
        sidecar.current.frame_pending = true;
        assert_eq!(key(&sidecar.current, None), k, "nor the wait on the frame");
        sidecar.current.frame_pending = false;
        sidecar.current.adjustments.push(crate::Adjustment {
            id: 3,
            name: "Sky".into(),
            ..Default::default()
        });
        let with = key(&sidecar.current, None);
        sidecar.current.adjustments[0].name = "The sky".into();
        assert_eq!(
            key(&sidecar.current, None),
            with,
            "a mask's name is not the picture"
        );
        sidecar.current.adjustments[0].enabled = false;
        assert_ne!(key(&sidecar.current, None), with);
        sidecar.current.adjustments.clear();
        assert_eq!(key(&sidecar.current, None), k);

        // The frame's turn is not the picture kept: that is the camera's
        // way up, and turned at draw time.
        sidecar.turn = 1;
        assert_eq!(key(&sidecar.current, None), k, "the frame's turn is not");
        sidecar.turn = 0;
        sidecar.current.geometry.crop = Some(crate::geometry::Crop {
            x: 0.1,
            y: 0.0,
            w: 0.9,
            h: 1.0,
        });
        assert_ne!(key(&sidecar.current, None), k, "the crop is");
    }

    /// The lens database is in the key while the edit corrects the lens
    /// by its profile, and only then.
    #[test]
    fn the_lens_database_is_in_the_key_while_the_profile_corrects() {
        let mut edit = Edit::default();
        assert!(edit.lens.enabled && edit.lens.profile);
        let none = key(&edit, None);
        let fetched = key(&edit, Some("/cache/lensfun/version_2 1760000000"));
        assert_ne!(none, fetched, "a database fetched is another picture");
        assert_ne!(
            fetched,
            key(&edit, Some("/cache/lensfun/version_2 1770000000"))
        );
        edit.lens.profile = false;
        assert_eq!(key(&edit, None), key(&edit, Some("anything")));
        edit.lens.profile = true;
        edit.lens.enabled = false;
        assert_eq!(key(&edit, None), key(&edit, Some("anything")));
    }

    /// A mask's white balance switched off is another picture; the
    /// light it keeps for the switch reaches no pixel and is not in the
    /// key, so two frames off at the global share one whatever they
    /// keep.
    #[test]
    fn a_masks_kept_white_is_not_in_the_key() {
        use crate::LocalWhite;
        let mut edit = Edit::default();
        edit.adjustments.push(crate::Adjustment {
            id: 1,
            ..Default::default()
        });
        let none = key(&edit, None);
        edit.adjustments[0].look.white_balance = LocalWhite::Absolute {
            temperature: 3200.0,
            tint: 0.002,
        };
        let on = key(&edit, None);
        assert_ne!(on, none, "a mask's own white is another picture");
        edit.adjustments[0].look.white_balance = LocalWhite::Kept {
            temperature: 3200.0,
            tint: 0.002,
        };
        assert_eq!(key(&edit, None), none, "switched off, the global's");
        edit.adjustments[0].look.white_balance = LocalWhite::Kept {
            temperature: 8000.0,
            tint: -0.01,
        };
        assert_eq!(key(&edit, None), none, "whatever it keeps");
        edit.adjustments[0].look.white_balance = LocalWhite::Absolute {
            temperature: 3200.0,
            tint: 0.002,
        };
        assert_eq!(key(&edit, None), on, "and back on, the same picture");
    }

    /// A learned mask's stages raised changes the key of an edit with
    /// such a mask and of no other: a Sky's and a Foliage's when the
    /// sky's stages move, a Water's not.
    #[test]
    fn a_learned_mask_s_stages_raised_change_the_keys_of_its_edits_alone() {
        use crate::mask::{Component, LandscapeGroup, Shape};
        let with = |shape: Shape| {
            let mut edit = Edit::default();
            let mut adjustment = crate::Adjustment::default();
            adjustment.mask.components.push(Component {
                shape,
                ..Component::default()
            });
            edit.adjustments.push(adjustment);
            edit
        };
        let raised = |shape: &Shape| -> Vec<String> {
            mask_pipelines(shape)
                .iter()
                .map(|&v| if v == SKY_PIPELINE { "sky-raised" } else { v }.to_string())
                .collect()
        };
        let both = |edit: &Edit| {
            (
                edit.develop_key_with(&[], None),
                edit.develop_key_by(&[], None, raised),
            )
        };
        let (a, b) = both(&with(Shape::Sky { picks: Vec::new() }));
        assert_ne!(a, b, "a Sky");
        let (a, b) = both(&with(Shape::Landscape {
            group: LandscapeGroup::Foliage,
        }));
        assert_ne!(a, b, "a Foliage, cut by the sky");
        let (a, b) = both(&with(Shape::Landscape {
            group: LandscapeGroup::Water,
        }));
        assert_eq!(a, b, "a Water");
        let (a, b) = both(&Edit::default());
        assert_eq!(a, b, "no mask");
        // And the stages are in the key at all: a Sky's key is not the
        // one it would have with no stages.
        assert_ne!(
            with(Shape::Sky { picks: Vec::new() }).develop_key_with(&[], None),
            with(Shape::Sky { picks: Vec::new() }).develop_key_by(&[], None, |_| Vec::new()),
        );
    }

    /// The camera's frame takes part in the key of an edit with no
    /// crop, which it frames, and in no other: an edit with a crop, and
    /// a frame the camera framed nothing of, keep their keys.
    #[test]
    fn the_cameras_frame_is_in_the_key_of_an_edit_it_frames() {
        use greycard_core::raw::{CameraFrame, Orientation, Rect};
        let frame = CameraFrame {
            area: Rect {
                x: 8,
                y: 8,
                width: 5184,
                height: 3888,
            },
            frame: Rect {
                x: 8,
                y: 224,
                width: 5184,
                height: 3456,
            },
            orientation: Orientation::Normal,
        };
        let square = CameraFrame {
            frame: Rect {
                x: 656,
                y: 8,
                width: 3888,
                height: 3888,
            },
            ..frame
        };
        let key = key(&Edit::default(), None);
        assert_eq!(framed_key(key, true, false, None), key);
        assert_eq!(
            framed_key(key, false, false, Some(&frame)),
            key,
            "a crop of its own"
        );
        let framed = framed_key(key, true, false, Some(&frame));
        assert_ne!(framed, key);
        assert_ne!(framed_key(key, true, false, Some(&square)), framed);
        assert_eq!(framed_key(key, true, false, Some(&frame)), framed);
        // A ratio over no crop frames as with no camera frame where the
        // frame is centered, and keeps its key; one off center does not.
        assert_eq!(framed_key(key, true, true, Some(&frame)), key);
        let off = CameraFrame {
            frame: Rect {
                y: 8,
                ..frame.frame
            },
            ..frame
        };
        assert_ne!(framed_key(key, true, true, Some(&off)), key);
    }

    /// The headroom's version is in the key of a file that may have
    /// headroom, and of no other: every other file keeps its pictures.
    #[test]
    fn the_headroom_is_keyed_only_where_a_file_may_have_it() {
        let key = key(&Edit::default(), None);
        assert_eq!(headroom_key(key, false), key);
        let cr3 = headroom_key(key, true);
        assert_ne!(cr3, key);
        assert_eq!(headroom_key(key, true), cr3, "the same for the same key");
        assert_ne!(headroom_key(key ^ 1, true), cr3);
        use greycard_core::decode::may_have_headroom;
        use std::path::Path;
        assert!(may_have_headroom(Path::new("a/IMG_0001.CR3")));
        assert!(may_have_headroom(Path::new("b.cr3")));
        for other in ["c.CR2", "d.dng", "e.NEF", "f.cr3.gcd", "cr3"] {
            assert!(!may_have_headroom(Path::new(other)), "{other}");
        }
    }

    /// The linearization's version is in the key of a DNG's pictures
    /// and of no other file's; a CR3's takes the headroom alone.
    #[test]
    fn the_linearization_is_keyed_only_for_a_dng() {
        use greycard_core::decode::may_have_linearization;
        use std::path::Path;
        let key = key(&Edit::default(), None);
        assert_eq!(linearization_key(key, false), key);
        let dng = linearization_key(key, true);
        assert_ne!(dng, key);
        assert_ne!(dng, headroom_key(key, true));
        assert_eq!(
            linearization_key(key, true),
            dng,
            "the same for the same key"
        );
        assert_ne!(linearization_key(key ^ 1, true), dng);
        assert!(may_have_linearization(Path::new("a/L1000001.DNG")));
        assert!(may_have_linearization(Path::new("b.dng")));
        for other in ["c.CR3", "d.RAF", "e.NEF", "f.dng.gcd", "dng"] {
            assert!(!may_have_linearization(Path::new(other)), "{other}");
        }
        assert_eq!(decode_key(key, Path::new("x.DNG")), dng);
        assert_eq!(decode_key(key, Path::new("x.cr3")), headroom_key(key, true));
        assert_eq!(decode_key(key, Path::new("x.RAF")), key);
        assert_eq!(decode_key(key, Path::new("x.nef")), key);
    }
}
