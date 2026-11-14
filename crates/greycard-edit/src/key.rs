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
pub const DEVELOP_RECIPE: u32 = 5;

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

/// An aspect over no crop, by version: taken into the key of an edit
/// whose aspect frames the picture with no crop stored
/// ([`crate::geometry::Geometry::frames_by_aspect`]), so the pictures
/// kept of such an edit are made again and those of every other edit
/// are kept. 1: framed by the aspect as the viewport frames it, where
/// before the pictures were the whole frame, 2026-11-13.
pub const ASPECT_FRAME: &str = "aspect-frame-1";

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
        if self.geometry.frames_by_aspect() {
            hasher.update(b"aspect frame\0");
            hasher.update(&(ASPECT_FRAME.len() as u64).to_le_bytes());
            hasher.update(ASPECT_FRAME.as_bytes());
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
}
