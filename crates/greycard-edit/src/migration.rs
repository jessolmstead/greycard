//! The schema's migrations: a stored edit brought up to [`VERSION`],
//! version by version, and the step that waits on the frame's shape.

use crate::{Edit, Error, Result, VERSION, baseline, geometry};

impl Edit {
    /// Whether this edit is still part-migrated: version 4's step
    /// for [`geometry::Aspect::Original`] wants the frame's shape,
    /// which the sidecar does not carry, so [`migrate`] marks the edit
    /// [`Edit::frame_pending`] and this says so (as it does for an edit
    /// made at version 3 in hand). An edit that answers yes must have
    /// [`Self::migrate_with_frame`] called on it before anything reads
    /// its crop's aspect — and it is safe to save meanwhile, since it
    /// writes back with the mark.
    pub fn needs_frame(&self) -> bool {
        self.frame_pending || self.version < 4
    }

    /// Finish version 4's step now the frame's shape is known.
    ///
    /// `source_portrait` is whether the frame's *developed* picture
    /// stands on end — the camera's tag and the frame's own turn
    /// applied, this edit's quarter turns not — which is what
    /// `Stance::shown_size` reports and what the viewport calls the
    /// source size. The plane the crop is measured in is that turned
    /// by the edit's own quarters.
    ///
    /// On a landscape plane the flag already meant what it means
    /// now, so it is left alone. On an upright plane it did not: a
    /// ticked flag was the only way to ask for the frame's own
    /// shape, and an unticked one gave a landscape crop of an
    /// upright frame, which is the bug. Both now mean the frame's
    /// own shape, so an upright plane ends flag-false either way.
    /// That is `portrait AND NOT plane_is_portrait`, which preserves
    /// what every stored edit rendered as except the one spelling
    /// whose rendering was the bug.
    ///
    /// A no-op on an edit that does not need it, so it is safe to
    /// call on every edit in hand.
    pub fn migrate_with_frame(&mut self, source_portrait: bool) {
        if !self.needs_frame() {
            return;
        }
        if self.geometry.aspect == geometry::Aspect::Original {
            let plane_portrait = source_portrait != (self.geometry.turns % 2 == 1);
            if plane_portrait {
                self.geometry.portrait = false;
            }
        }
        self.frame_pending = false;
        self.version = VERSION;
    }
}

/// Bring a stored edit up to [`VERSION`]. A missing version is read
/// as 1. Version 2 gave the NOISE section a switch of its own, so
/// version 1's `noise.enabled`, which was the profiled denoiser's,
/// becomes `noise.profiled`; the section's switch is on.
///
/// Version 3 is the tone equalizer and the white point. No field moved
/// and none is renamed, so the migration is the identity, but the bump
/// is not ceremony: `light.tone.highlights`, `.shadows` and `.whites`
/// changed meaning. The first two were stops of shift read from the
/// pixel's own luminance, over ramps placed at 0..3 and -3..0 stops
/// over mid grey; they are read from a smoothed luminance now — a
/// region's, not a pixel's — over 0..2.5 and -3..0. Whites was a third
/// such shift over 2..5 stops and is a white point pivoted at mid grey,
/// a global power on the top of the scale. A sidecar written before
/// this build opens with its numbers unchanged and its picture does
/// move: most of all where whites was set, which used to move the clip
/// by 7 percent of a stop at minus one and moves it by 71 now.
///
/// That is the deliberate choice, and the alternative was considered.
/// Scaling the old numbers to reproduce the old picture would need a
/// whites of about a fourteenth of what is stored, which is not a
/// setting anybody chose and would carry forward the very thing the
/// roadmap called "whites do basically nothing". The stored number is
/// what the edit asked for; this is the first build that does it.
///
/// §98 moved the two ramps within version 3, highlights to -1..2.5
/// and shadows to -3.5..0, and the sliders to ±2: where the same
/// shift reaches, not what the number is, and nothing shipped in
/// between, so no bump.
///
/// Version 4 changed what [`geometry::Aspect::Original`] means. It
/// used to stand the plane up as landscape and read which way up the
/// crop went off `geometry.portrait`, which starts false, so
/// "Original" on a frame shot on end cropped it landscape; it is the
/// plane's own ratio now, and the flag turns that. The conversion
/// clears the flag on an upright plane and leaves it on a landscape
/// one, which keeps every stored edit rendering as it did but for
/// the one spelling whose rendering was the bug; and the plane is
/// the frame's developed picture turned by the edit's own quarter
/// turns, which a sidecar does not hold.
///
/// So this step cannot be done here. An edit that needs it is marked
/// [`Edit::frame_pending`] and says so through [`Edit::needs_frame`];
/// whoever knows the frame's shape finishes it with
/// [`Edit::migrate_with_frame`], and until then the edit reads,
/// renders and writes back with the mark, so nothing is lost by a
/// build that only listed a folder. Until version 8 the mark was the
/// version itself, left at 3; that ran every later step again at the
/// next read, which version 8's step cannot take, so a build from now
/// writes the mark and its own version.
///
/// Version 5 is the sidecar's versions ([`Sidecar::versions`]).
/// Nothing in an edit moved or changed meaning, so the step is the
/// identity; the bump is there so that a build from before versions,
/// which would read version 0 and drop the rest at its next save,
/// refuses the sidecar whole instead.
///
/// Version 6 is the baseline per body ([`baseline`]): the exposure's
/// zero is the body's own baseline, where it was 0.8 stops for every
/// body. An edit written before that develops anything keeps the
/// number it was made against, as [`Light::baseline`], so its picture
/// is the one it was; an edit that develops nothing is the default and
/// takes its body's. A build from before would drop the field and
/// brighten or darken the picture, so it refuses the sidecar instead.
///
/// Version 8 is one brush row: a brush's Subtract takes from the mask
/// under it, where it took from the brush's own paint and nothing
/// else ([`brush`]). An edit written before keeps its picture: on a
/// brush joined by anything but a plain Subtract, its Subtract strokes
/// become [`Op::Lift`], which is what they did; a brush joined by
/// Subtract and not inverted (a "− Brush") becomes a brush joined by
/// Add whose Add strokes are Subtract and whose Subtract strokes are
/// Lift, so it cuts what its paint cut, by as much. The first day's
/// `erase: true` on a stroke is a Subtract before either. A build from
/// before would read a Subtract as a lift, so it refuses the sidecar.
///
/// Version 9 is a Fill of several strokes ([`Patch::more`]). Nothing
/// moves: an edit from before has none, and a repair of one stroke
/// writes as it did. A build from before would drop the strokes after
/// the first and fill less than was painted, so it refuses the
/// sidecar.
///
/// Version 10 is the turn a state was placed under
/// ([`Edit::placed_under`], §724): a turn of the frame changes the
/// sidecar's turn and no state, each state shown under it from the turn
/// it was placed under. Every state written before it was turned in
/// place to the sidecar's turn, so it is placed under that turn; the
/// step needs the sidecar's turn and is `migrate_line`'s, not this. A
/// build from before would take a state placed under another turn for
/// one placed under the frame's, and show it a turn off, so it refuses
/// the sidecar.
///
/// [`brush`]: crate::brush
/// [`Op::Lift`]: crate::brush::Op::Lift
/// [`Patch::more`]: crate::retouch::Patch::more
///
/// Two shapes serde would take are refused before any step, since
/// neither writes back as it reads (both found by fuzzing). An edit
/// that is not an object: serde takes a list of a struct's fields in
/// their order too, so `[0]` would read as a version 0 edit that no
/// step here had touched, and be migrated only when read again. And a
/// number past the range of an `f32`: it reads as infinity, which
/// serde_json writes as `null`, and the next read refuses that.
///
/// [`Sidecar::versions`]: crate::Sidecar::versions
/// [`Light::baseline`]: crate::Light::baseline
pub fn migrate(mut value: serde_json::Value) -> Result<serde_json::Value> {
    if !value.is_object() {
        return Err(refused("an edit is an object"));
    }
    if let Some((path, n)) = past_f32(&value) {
        return Err(refused(&format!(
            "{path} is {n:e}, past the range of any setting"
        )));
    }
    // A missing version is 1; one that is there and is not a schema
    // version at all is refused, rather than read as 1 (`7.0`, `-1`,
    // `"9"`) or wrapped (`4294967297` was 1).
    let version = match value.get("version") {
        None => 1,
        Some(v) => v
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| refused(&format!("a version is a whole number, not {v}")))?,
    };
    if version > VERSION {
        return Err(Error::Newer(version));
    }
    if let Some(obj) = value.as_object_mut() {
        // Version 3 changed what the three tone shifts mean, not where
        // they are kept, so it has nothing to move; the doc above says
        // why the numbers are left standing.
        if version < 2
            && let Some(noise) = obj.get_mut("noise").and_then(|n| n.as_object_mut())
            && let Some(profiled) = noise.remove("enabled")
        {
            noise.insert("profiled".into(), profiled);
        }
        // The dehaze was a top-level `dehaze: {amount}` for a few
        // days of master before it joined the DETAIL section; a
        // sidecar from then keeps its amount. Within version 3: it
        // never shipped.
        if let Some(amount) = obj
            .remove("dehaze")
            .and_then(|d| d.get("amount").and_then(|a| a.as_f64()))
        {
            let detail = obj
                .entry("detail")
                .or_insert_with(|| serde_json::Value::Object(Default::default()));
            if let Some(detail) = detail.as_object_mut() {
                detail.insert("dehaze".into(), amount.into());
            }
        }
        // Version 6 gave each body a baseline of its own
        // (`baseline::for_body`). An edit written before was made
        // against one number for every body and keeps it, so a picture
        // someone developed does not move; one that develops nothing
        // (a frame only rated, say) is the default and takes its
        // body's, as a frame with no sidecar does.
        if version < 6 && !develops_nothing(obj) {
            let light = obj
                .entry("light")
                .or_insert_with(|| serde_json::Value::Object(Default::default()));
            if let Some(light) = light.as_object_mut()
                && !light.contains_key("baseline")
            {
                light.insert("baseline".into(), f64::from(baseline::BEFORE_BODIES).into());
            }
        }
        // Version 7 gave the learned blend a chroma strength of its
        // own, full by default. An edit written before blended the
        // chroma by the one strength it had, and keeps that blend
        // where its learned tier is on; where it is off the blend
        // does not run, and the field at its default is left out.
        if version < 7
            && let Some(noise) = obj.get_mut("noise").and_then(|n| n.as_object_mut())
            && noise
                .get("learned")
                .and_then(|l| l.as_str())
                .is_some_and(|l| l != "off")
            && !noise.contains_key("learned_chroma_strength")
            && let Some(blend) = noise.get("learned_strength").cloned()
        {
            noise.insert("learned_chroma_strength".into(), blend);
        }
        if version < 8 {
            one_brush_row(obj);
        }
        // Version 4's step needs the frame's shape, which is not in
        // here: an edit that wants it is marked `frame_pending` until
        // someone who knows the frame calls `Edit::migrate_with_frame`.
        // The mark, not the version, says so: an edit written back at 3
        // would have every later step run on it again at its next read,
        // and version 8's is not one that can. Only an
        // `Original` aspect is affected; every other edit is brought
        // up by its version number alone. Version 5's step (the
        // sidecar's versions) and version 9's (a Fill's further
        // strokes) move nothing in an edit.
        let waiting = version < 4
            && obj
                .get("geometry")
                .and_then(|g| g.get("aspect"))
                .and_then(|a| a.get("kind"))
                .and_then(|k| k.as_str())
                == Some("original");
        if waiting {
            obj.insert("frame_pending".into(), true.into());
        }
        obj.insert("version".into(), VERSION.into());
    }
    Ok(value)
}

/// Version 8's step on every brush in the edit's masks (the doc of
/// [`migrate`]).
fn one_brush_row(obj: &mut serde_json::Map<String, serde_json::Value>) {
    use serde_json::Value;
    let Some(adjustments) = obj.get_mut("adjustments").and_then(Value::as_array_mut) else {
        return;
    };
    let components = adjustments
        .iter_mut()
        .filter_map(|a| a.get_mut("mask")?.get_mut("components")?.as_array_mut())
        .flatten()
        .filter_map(Value::as_object_mut);
    for c in components {
        let is_brush = c
            .get("shape")
            .and_then(|s| s.get("kind"))
            .and_then(Value::as_str)
            == Some("brush");
        if !is_brush {
            continue;
        }
        let minus = c.get("mode").and_then(Value::as_str) == Some("subtract")
            && !c.get("invert").and_then(Value::as_bool).unwrap_or(false);
        if minus {
            c.insert("mode".into(), "add".into());
        }
        let Some(strokes) = c
            .get_mut("shape")
            .and_then(|s| s.get_mut("strokes"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for stroke in strokes.iter_mut().filter_map(Value::as_object_mut) {
            let erase = stroke
                .remove("erase")
                .and_then(|e| e.as_bool())
                .unwrap_or(false);
            let op = match stroke.get("op").and_then(Value::as_str).unwrap_or("add") {
                "add" if erase => "subtract",
                op => op,
            };
            let op = match (op, minus) {
                ("add", true) => "subtract",
                ("subtract", _) => "lift",
                (op, _) => op,
            }
            .to_owned();
            stroke.insert("op".into(), op.into());
        }
    }
}

/// A stored file's shape refused, in the words of a JSON error.
pub(crate) fn refused(what: &str) -> Error {
    Error::Json(<serde_json::Error as serde::de::Error>::custom(what))
}

/// The first number in `value` that an `f32` cannot hold, and where it
/// is (`detail.texture`, `mixer.saturation[7]`). Every float in an
/// edit is an `f32` or holds a value that fits one, and no integer in
/// it comes near. The path is made on the way back out, so a walk that
/// finds nothing makes none.
fn past_f32(value: &serde_json::Value) -> Option<(String, f64)> {
    // `head` before the rest of the path: `.key` unless it is empty or
    // goes on with an index.
    let under = |head: String, rest: String| {
        if rest.is_empty() || rest.starts_with('[') {
            head + &rest
        } else {
            format!("{head}.{rest}")
        }
    };
    match value {
        serde_json::Value::Number(n) => n
            .as_f64()
            .filter(|&n| n.is_finite() && (n as f32).is_infinite())
            .map(|n| (String::new(), n)),
        serde_json::Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(i, v)| past_f32(v).map(|(rest, n)| (under(format!("[{i}]"), rest), n))),
        serde_json::Value::Object(map) => map
            .iter()
            .find_map(|(k, v)| past_f32(v).map(|(rest, n)| (under(k.clone(), rest), n))),
        _ => None,
    }
}

/// The key a line's read finds on an edit a build before version 10
/// wrote, put there by [`migrate_line`] and taken out before the edit is
/// read (`line::ReadEdit`): such a state with no id is given the one
/// that build gave it ([`crate::sync::filled_id`]). Never written; no
/// field of an edit can be called so.
pub(crate) const BEFORE_PLACED_MARK: &str = "#before_placed";

/// The key a line's read finds when [`migrate_line`] left states out
/// that could not be read (`Line::unread`). Never written.
pub(crate) const UNREAD_MARK: &str = "#unread";

/// Version 10's step on one stored edit of a sidecar whose turn is
/// `turn`: an edit a build before it wrote is placed under that turn,
/// and marked as that build's ([`BEFORE_PLACED_MARK`]). One that is not
/// an object, or whose version is not a number, is left for [`migrate`]
/// to refuse.
fn placed_before_ten(edit: &mut serde_json::Value, turn: u8) {
    let Some(obj) = edit.as_object_mut() else {
        return;
    };
    let version = match obj.get("version") {
        None => 1,
        Some(v) => match v.as_u64() {
            Some(v) => v,
            None => return,
        },
    };
    if version >= 10 {
        return;
    }
    obj.insert(BEFORE_PLACED_MARK.into(), true.into());
    if turn != 0 && !obj.contains_key("placed_under") {
        obj.insert("placed_under".into(), turn.into());
    }
}

/// Whether a stored edit, its earlier steps already brought up, is a
/// raw's or a picture's default: nothing in it but the version and the
/// learned denoiser's blend, which the first open seeds from the ISO
/// and which moves no brightness. One that will not read is not.
fn develops_nothing(obj: &serde_json::Map<String, serde_json::Value>) -> bool {
    let Ok(mut edit) = serde_json::from_value::<Edit>(serde_json::Value::Object(obj.clone()))
    else {
        return false;
    };
    edit.version = VERSION;
    // Where it was placed is no development (version 10's step has
    // put it in already when the frame was turned).
    edit.placed_under = 0;
    // A picture's default as an edit from before the bodies held it:
    // the one with its lens corrections on.
    let [_, picture_then] = Edit::picture_defaults();
    [Edit::default(), picture_then].into_iter().any(|d| {
        edit.noise.learned_strength = d.noise.learned_strength;
        edit == d
    })
}

/// Bring every edit of a line's object up to the current schema, in
/// place: the current one, the history and the snapshots. A history or
/// a snapshot list that is not a list is left out, as it always was.
///
/// `turn` is the sidecar's turn as read: version 10's step, which needs
/// it and so is not [`migrate`]'s. Every state a build before version
/// 10 wrote was turned in place to the sidecar's turn at each turn of
/// the frame, so each is placed under it ([`Edit::placed_under`],
/// §724). Nothing moves, and every state shows as it showed.
///
/// `budget` is what the file's differences may still make whole
/// ([`crate::sparse::BUDGET`]), shared by its lines. `newer_ok` when the
/// file's floors let this build read states past its schema: such a
/// state is read as it is, unmigrated ([`crate::Floors`]).
/// The schema version an edit says, none when it says none it could.
pub(crate) fn version_of(edit: &serde_json::Value) -> Option<u32> {
    edit.get("version")?.as_u64()?.try_into().ok()
}

pub(crate) fn migrate_line(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    turn: u8,
    budget: &mut usize,
    newer_ok: bool,
) -> Result<()> {
    let migrated = |mut edit: serde_json::Value| {
        if newer_ok && version_of(&edit).is_some_and(|v| v > VERSION) {
            return Ok(edit);
        }
        placed_before_ten(&mut edit, turn);
        migrate(edit)
    };
    // A difference only in the history, against an earlier entry: the
    // current state and the snapshots are written whole (§724).
    let whole = |edit: &serde_json::Value| edit.get("delta").is_none();
    if let Some(current) = obj.remove("current") {
        if !whole(&current) {
            return Err(refused("the current state is written whole"));
        }
        obj.insert("current".into(), migrated(current)?);
    }
    if let Some(serde_json::Value::Array(history)) = obj.remove("history") {
        let (history, lost) = crate::sparse::whole_history(history, budget);
        let history: Result<Vec<_>> = history.into_iter().map(migrated).collect();
        obj.insert("history".into(), serde_json::Value::Array(history?));
        if lost > 0 {
            obj.insert(UNREAD_MARK.into(), lost.into());
        }
    }
    if let Some(serde_json::Value::Array(mut snapshots)) = obj.remove("snapshots") {
        for snapshot in snapshots.iter_mut() {
            // As for an edit (see `migrate`): a list in its place would
            // carry an edit no step had touched.
            if !snapshot.is_object() {
                return Err(refused("a snapshot is an object"));
            }
            if let Some(edit) = snapshot.get_mut("edit") {
                if !whole(edit) {
                    return Err(refused("a snapshot is written whole"));
                }
                *edit = migrated(edit.take())?;
            }
        }
        obj.insert("snapshots".into(), serde_json::Value::Array(snapshots));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Preset, Sidecar};

    /// Version 4 changed what `Original` means, so every sidecar
    /// written before it has to go on rendering as it did. The old
    /// build stood the plane up as landscape and read the way up off
    /// `portrait`; the new one reads the plane's own ratio. The
    /// migration is `portrait AND NOT plane_is_portrait` — an
    /// upright plane ends flag-false, a landscape one keeps the flag
    /// it had — and the plane is the developed frame turned by the
    /// edit's own quarters, so it needs the frame and cannot be done
    /// in `migrate`.
    ///
    /// The one row that must *not* come back the same is the bug:
    /// an upright frame with the flag false cropped landscape, and
    /// now crops the whole frame. That row is the only one where
    /// this differs from `portrait XOR plane_is_portrait`, which
    /// would have preserved the bug along with everything else.
    #[test]
    fn a_version_three_original_keeps_its_shape_once_the_frame_is_known() {
        use geometry::Aspect;

        // The landscape source the editor develops, and the same
        // frame shot on end.
        const LAND: (f32, f32) = (6000.0, 4000.0);
        const UP: (f32, f32) = (4000.0, 6000.0);

        // What version 3 would have cropped: the plane stood up as
        // landscape, then turned by the flag.
        fn as_version_three(turns: u8, portrait: bool, source: (f32, f32)) -> f32 {
            let (w, h) = if turns % 2 == 1 {
                (source.1, source.0)
            } else {
                source
            };
            let landscape = if w / h >= 1.0 { w / h } else { h / w };
            if portrait { 1.0 / landscape } else { landscape }
        }

        let stored = |turns: u8, portrait: bool| {
            format!(
                r#"{{"version":3,"geometry":{{"turns":{turns},"portrait":{portrait},
                   "aspect":{{"kind":"original"}}}}}}"#
            )
        };

        for (name, turns, portrait, source, old_is_right) in [
            ("landscape, flag false", 0u8, false, LAND, true),
            ("landscape, flag true", 0, true, LAND, true),
            ("upright, flag false", 0, false, UP, false),
            ("upright, flag true", 0, true, UP, true),
            // What the old build itself wrote for "pick Original,
            // then Turn left" on a landscape frame: it flipped the
            // flag on every quarter turn. The common case.
            ("landscape turned, flag true", 1, true, LAND, true),
        ] {
            let edit = Edit::from_json(&stored(turns, portrait)).unwrap();
            // It reads, and it says it is not finished with.
            assert!(edit.needs_frame(), "{name}");
            assert!(edit.frame_pending, "{name}");
            // Saved back meanwhile it keeps the mark, so a build that
            // only listed the folder loses nothing, and reads the same.
            let saved = edit.to_json();
            assert!(saved.contains("\"frame_pending\": true"), "{name}");
            assert_eq!(Edit::from_json(&saved).unwrap(), edit, "{name}");

            let mut edit = edit;
            edit.migrate_with_frame(source.1 > source.0);
            assert!(!edit.needs_frame(), "{name}");
            assert_eq!(edit.version, VERSION, "{name}");

            let (pw, ph) = edit.geometry.plane_size(source.0, source.1);
            let now = edit
                .geometry
                .aspect
                .ratio(pw, ph, edit.geometry.portrait)
                .unwrap();
            let then = as_version_three(turns, portrait, source);
            if old_is_right {
                assert!((now - then).abs() < 1e-6, "{name}: {then} became {now}");
            } else {
                // The bug: it cropped landscape and must now be the
                // whole upright frame.
                assert!((then - 1.5).abs() < 1e-6, "{name}: {then}");
                assert!((now - pw / ph).abs() < 1e-6, "{name}: {now}");
                assert!(now < 1.0, "{name}: {now} is not upright");
            }
        }

        // Nothing but `Original` is touched, and a free or named
        // aspect comes up to version 4 in `migrate` alone.
        for aspect in [r#"{"kind":"free"}"#, r#"{"kind":"ratio","w":3.0,"h":2.0}"#] {
            let json =
                format!(r#"{{"version":3,"geometry":{{"portrait":true,"aspect":{aspect}}}}}"#);
            let edit = Edit::from_json(&json).unwrap();
            assert!(!edit.needs_frame(), "{aspect}");
            assert_eq!(edit.version, VERSION, "{aspect}");
            assert!(edit.geometry.portrait, "{aspect}");
        }
        // And an edit this build wrote is never migrated twice: the
        // flag would flip back.
        let mut fresh = Edit {
            geometry: geometry::Geometry {
                aspect: Aspect::Original,
                portrait: true,
                ..geometry::Geometry::default()
            },
            ..Edit::default()
        };
        assert!(!fresh.needs_frame());
        fresh.migrate_with_frame(true);
        assert!(fresh.geometry.portrait, "a version 4 edit is left alone");
    }

    /// A sidecar migrates every state it holds, not only the current
    /// one, since undo, redo and a snapshot each put one of the
    /// others back on the panel; in the copies shown, so the file keeps
    /// what it was written with and the step is the same each read.
    #[test]
    fn a_sidecar_migrates_all_its_states_once_and_only_once() {
        let json = r#"{
            "current": {"version":3,"geometry":{"aspect":{"kind":"original"}}},
            "history": [{"version":3,"geometry":{"aspect":{"kind":"original"}}}],
            "snapshots": [{"name":"a","taken":0,
                "edit":{"version":3,"geometry":{"aspect":{"kind":"original"}}}}]
        }"#;
        let mut sidecar: Sidecar = crate::sidecar::plain(json).unwrap();
        assert!(sidecar.needs_frame());
        // Written back before the frame is known, it is version 3
        // still, and reads back wanting the same step.
        let written = serde_json::to_string(&sidecar).unwrap();
        let again: Sidecar = crate::sidecar::plain(&written).unwrap();
        assert!(again.needs_frame());

        // The frame stands on end: every state's flag turns, in the
        // copy shown (§724).
        sidecar.frame_known(2.0 / 3.0);
        assert!(!sidecar.needs_frame());
        for edit in [
            &sidecar.current,
            &sidecar.history[0].edit,
            &sidecar.snapshots[0].edit,
        ] {
            assert_eq!(edit.version, VERSION);
            assert!(!edit.frame_pending);
            assert!(!edit.geometry.portrait, "an upright plane ends flag-false");
        }
        // The states as written keep what they hold: written back, the
        // file is the one read but for the shape now known beside them,
        // so a read of it shows what the first did with nothing to
        // measure; the file read before it waits for the frame again
        // and, shown by it, shows the same.
        let mut now: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&sidecar).unwrap()).unwrap();
        let shape = now.as_object_mut().unwrap().remove("shape").unwrap();
        assert_eq!(
            serde_json::from_value::<crate::Measured>(shape).unwrap(),
            crate::Measured {
                turn: 0,
                aspect: 2.0 / 3.0
            }
        );
        assert_eq!(
            now,
            serde_json::from_str::<serde_json::Value>(&written).unwrap()
        );
        let measured = Sidecar::from_json(
            &serde_json::to_string(&sidecar).unwrap(),
            &crate::Edit::default(),
        )
        .unwrap();
        assert!(!measured.needs_frame());
        assert_eq!(measured.current.geometry, sidecar.current.geometry);
        let mut again: Sidecar = crate::sidecar::plain(&written).unwrap();
        assert!(again.needs_frame());
        again.frame_known(2.0 / 3.0);
        assert_eq!(again.current, sidecar.current);
        assert_eq!(again.history[0].edit, sidecar.history[0].edit);
        assert_eq!(again.snapshots[0].edit, sidecar.snapshots[0].edit);
        assert_eq!(again, sidecar);
    }

    #[test]
    fn a_version_one_noise_switch_is_the_profiled_denoisers() {
        let old = r#"{"version": 1, "noise": {"enabled": true, "strength": 2.0}}"#;
        let e = Edit::from_json(old).unwrap();
        assert!(e.noise.enabled, "the section is on");
        assert!(
            e.noise.profiled,
            "and the profiled pass is what it asked for"
        );
        assert_eq!(e.noise.strength, 2.0);
        assert!(e.settings(&crate::NoStores).denoise.is_some());
        // Written back, it is version 2 and reads the same.
        let again = Edit::from_json(&e.to_json()).unwrap();
        assert_eq!(again, e);
        // A version 2 file's `enabled` is the section's.
        let new = r#"{"version": 2, "noise": {"enabled": false, "profiled": true}}"#;
        let e = Edit::from_json(new).unwrap();
        assert!(!e.noise.enabled && e.noise.profiled);
        assert!(e.settings(&crate::NoStores).denoise.is_none());
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let err = Edit::from_json(r#"{"version": 99}"#).unwrap_err();
        assert!(matches!(err, Error::Newer(99)));
    }

    #[test]
    fn a_sidecar_without_a_dehaze_reads_as_zero() {
        let edit = Edit::from_json(r#"{"version": 3, "light": {"exposure": 1.0}}"#).unwrap();
        assert_eq!(edit.detail.dehaze, 0.0);
        let edit = Edit::from_json(r#"{"version": 3, "detail": {"texture": 0.2, "clarity": 0.1}}"#)
            .unwrap();
        assert_eq!(edit.detail.dehaze, 0.0);
        assert_eq!(edit.detail.texture, 0.2);
    }

    /// A sidecar from the days the dehaze sat at the top level of the
    /// edit keeps its amount, in the Detail section now, whether or
    /// not the sidecar had a Detail section of its own.
    #[test]
    fn a_top_level_dehaze_moves_into_the_detail_section() {
        let edit = Edit::from_json(r#"{"version": 3, "dehaze": {"amount": 0.5}}"#).unwrap();
        assert_eq!(edit.detail.dehaze, 0.5);
        assert!(edit.detail.enabled);
        let edit = Edit::from_json(
            r#"{"version": 3, "dehaze": {"amount": -0.25}, "detail": {"texture": 0.3}}"#,
        )
        .unwrap();
        assert_eq!(edit.detail.dehaze, -0.25);
        assert_eq!(edit.detail.texture, 0.3);
        // And it is written where it now lives, not where it was.
        let json = edit.to_json();
        let tree: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(tree.get("dehaze").is_none(), "{json}");
        assert_eq!(tree["detail"]["dehaze"], -0.25);
        assert_eq!(Edit::from_json(&json).unwrap(), edit);
    }

    /// Found by fuzzing: serde reads a struct from a list of its fields
    /// too, so `[0]` was a version 0 edit no migration had touched,
    /// which wrote back as one the next read migrated. An edit, a
    /// snapshot, a version's line, and a preset that are not objects
    /// are refused.
    #[test]
    fn fuzz_an_edit_written_as_a_list_is_refused() {
        assert!(Edit::from_json("[0]").is_err());
        assert!(Sidecar::from_json(r#"{"current": [0]}"#, &crate::Edit::default()).is_err());
        assert!(Sidecar::from_json(r#"{"history": [[0]]}"#, &crate::Edit::default()).is_err());
        assert!(
            Sidecar::from_json(r#"{"snapshots": [["s", 0, {}]]}"#, &crate::Edit::default())
                .is_err()
        );
        assert!(
            Sidecar::from_json(
                r#"{"versions": [{"id": "a", "line": [[0]]}]}"#,
                &crate::Edit::default()
            )
            .is_err()
        );
        assert!(Preset::from_json(r#"[6, "p", ["light"], [], "replace", [0]]"#).is_err());
        // The same as objects read.
        assert!(Edit::from_json(r#"{"version": 0}"#).is_ok());
        assert!(
            Sidecar::from_json(
                r#"{"snapshots": [{"name": "s", "taken": 0, "edit": {}}]}"#,
                &crate::Edit::default()
            )
            .is_ok()
        );
        // A version entry with no id is left out, as the versions'
        // reader leaves it, whatever its line is.
        for junk in [
            r#"[[0]]"#,
            r#"{"line": [[0]]}"#,
            r#"{"id": " ", "line": [[0]]}"#,
        ] {
            let sidecar = Sidecar::from_json(
                &format!(r#"{{"versions": [{junk}]}}"#),
                &crate::Edit::default(),
            )
            .unwrap_or_else(|e| panic!("{junk}: {e}"));
            assert!(sidecar.versions.is_empty(), "{junk}");
        }
    }

    /// A missing schema version is version 1. One that is there and is
    /// not a whole number a `u32` holds is refused, where it was read as
    /// version 1, or wrapped round to it.
    #[test]
    fn a_version_that_is_not_a_whole_number_is_refused() {
        for v in ["4294967297", "7.0", "-1", "\"9\"", "null", "true"] {
            let err = Edit::from_json(&format!(r#"{{"version": {v}}}"#))
                .expect_err(v)
                .to_string();
            assert!(err.contains("a version is a whole number"), "{v}: {err}");
        }
        let missing = Edit::from_json(r#"{"noise": {"enabled": true}}"#).unwrap();
        assert!(missing.noise.profiled, "read as version 1 and migrated");
    }

    /// Found by fuzzing: a number past an `f32`'s range read as
    /// infinity, which serde_json writes as `null`, so the sidecar
    /// saved after it would not load. It is refused when read, in an
    /// edit, a sidecar's history, and a preset, with where it is and
    /// what was being read; the largest `f32` still reads, and writes
    /// back as it was.
    #[test]
    fn fuzz_a_number_past_an_f32_is_refused() {
        let edit = r#"{"version": 6, "detail": {"texture": 1e71}}"#;
        assert_eq!(
            Edit::from_json(edit).unwrap_err().to_string(),
            "sidecar: detail.texture is 1e71, past the range of any setting"
        );
        assert!(
            Sidecar::from_json(
                &format!(r#"{{"history": [{{}}, {edit}]}}"#),
                &crate::Edit::default()
            )
            .is_err()
        );
        let preset = r#"{"name": "p", "sections": ["mixer"],
            "edit": {"mixer": {"saturation": [0, 0, 0, 0, 0, 0, 0, -2.2e53]}}}"#;
        assert_eq!(
            Preset::from_json(preset).unwrap_err().to_string(),
            "preset: mixer.saturation[7] is -2.2e53, past the range of any setting"
        );
        let edge =
            Edit::from_json(r#"{"version": 6, "detail": {"texture": 3.4028235e38}}"#).unwrap();
        assert_eq!(edge.detail.texture, f32::MAX);
        assert_eq!(Edit::from_json(&edge.to_json()).unwrap(), edge);
    }
}

#[cfg(test)]
mod baselines {
    use super::*;
    use crate::{Preset, Section, Sidecar, preset};

    /// An edit as a build before the baselines per body wrote it.
    fn written_before(edit: &serde_json::Value) -> Edit {
        let mut v = edit.clone();
        v["version"] = 5.into();
        Edit::from_json(&v.to_string()).unwrap()
    }

    /// An edit from before the chroma blend had one strength for both:
    /// with a learned tier on it keeps that blend for the chroma, so
    /// it develops as it did; with the tier off the chroma is full,
    /// the default, and the file is unchanged by it.
    #[test]
    fn an_edit_written_before_the_chroma_blend_keeps_its_blend() {
        let v = |learned: &str| {
            let mut v =
                serde_json::json!({"noise": {"learned": learned, "learned_strength": 0.35}});
            v["version"] = 6.into();
            Edit::from_json(&v.to_string()).unwrap()
        };
        let on = v("best");
        assert_eq!(on.noise.learned_strength, 0.35);
        assert_eq!(on.noise.learned_chroma_strength, 0.35);
        assert_eq!(on.version, VERSION);
        assert_eq!(Edit::from_json(&on.to_json()).unwrap(), on);
        let off = v("off");
        assert_eq!(off.noise.learned_chroma_strength, 1.0);
        let json: serde_json::Value = serde_json::from_str(&off.to_json()).unwrap();
        assert!(json["noise"].get("learned_chroma_strength").is_none());
        // Written at 7 with the field left out, it is full.
        let mut v = serde_json::json!({"noise": {"learned": "best", "learned_strength": 0.35}});
        v["version"] = 7.into();
        let fresh = Edit::from_json(&v.to_string()).unwrap();
        assert_eq!(fresh.noise.learned_chroma_strength, 1.0);
    }

    #[test]
    fn an_edit_written_before_keeps_the_zero_it_was_made_against() {
        let edit = written_before(&serde_json::json!({"light": {"exposure": 0.3}}));
        assert_eq!(edit.light.exposure, 0.3);
        assert_eq!(edit.light.baseline, Some(baseline::BEFORE_BODIES));
        assert_eq!(edit.version, VERSION);
        // With no light in it at all, but a turn: the same.
        let edit = written_before(&serde_json::json!({"geometry": {"turns": 1}}));
        assert_eq!(edit.light.baseline, Some(baseline::BEFORE_BODIES));
        assert_eq!(edit.light.exposure, 0.0);
        // Read and written again, it is kept, and read again it is
        // what was written.
        let again = Edit::from_json(&edit.to_json()).unwrap();
        assert_eq!(again, edit);
    }

    #[test]
    fn an_edit_written_before_that_develops_nothing_takes_its_body_s() {
        assert_eq!(written_before(&serde_json::json!({})).light.baseline, None);
        // The learned blend the first open seeds is not a develop.
        let seeded = written_before(&serde_json::json!({"noise": {"learned_strength": 0.6}}));
        assert_eq!(seeded.light.baseline, None);
        assert_eq!(seeded.noise.learned_strength, 0.6);
        // A picture's default then had its lens corrections on.
        let picture = serde_json::to_value(&Edit::picture_defaults()[1]).unwrap();
        assert_eq!(written_before(&picture).light.baseline, None);
    }

    #[test]
    fn an_edit_written_now_has_no_zero_of_its_own_and_one_kept_is_not_moved() {
        let mut e = Edit::default();
        e.light.exposure = 0.3;
        assert!(!e.to_json().contains("baseline"));
        assert_eq!(Edit::from_json(&e.to_json()).unwrap().light.baseline, None);
        let kept = serde_json::json!({"version": 5, "light": {"exposure": 0.3, "baseline": 0.5}});
        assert_eq!(
            Edit::from_json(&kept.to_string()).unwrap().light.baseline,
            Some(0.5)
        );
    }

    #[test]
    fn a_sidecar_written_before_keeps_it_on_every_state() {
        let json = serde_json::json!({
            "version": 5,
            "current": {"version": 5, "light": {"exposure": 0.5}},
            "history": [{"version": 5}, {"version": 5, "light": {"exposure": 0.2}}],
        });
        let s = Sidecar::from_json(&json.to_string(), &crate::Edit::default()).unwrap();
        assert_eq!(s.line.current.light.baseline, Some(baseline::BEFORE_BODIES));
        assert_eq!(s.line.history.len(), 2);
        assert_eq!(s.line.history[0].edit.light.baseline, None);
        assert_eq!(
            s.line.history[1].edit.light.baseline,
            Some(baseline::BEFORE_BODIES)
        );
    }

    #[test]
    fn the_panel_s_light_leaves_the_zero_alone_and_the_switch_keeps_it() {
        let mut e = Edit::default();
        e.light.baseline = Some(0.8);
        let mut look = Edit::default().look();
        look.light.exposure = 1.0;
        e.set_look(look);
        assert_eq!(e.light.exposure, 1.0);
        assert_eq!(e.light.baseline, Some(0.8));
        e.light.enabled = false;
        assert_eq!(e.light.effective().baseline, Some(0.8));
        assert_eq!(e.light.effective().exposure, 0.0);
    }

    #[test]
    fn a_preset_carries_no_zero_and_one_that_resets_the_exposure_lands_on_the_body_s() {
        let mut old = Edit::default();
        old.light.exposure = 0.4;
        old.light.baseline = Some(baseline::BEFORE_BODIES);
        let reset = Preset::from_edit("Reset light", &Edit::default(), &[Section::Light]);
        assert_eq!(reset.edit.light.baseline, None);
        let laid = reset.applied(&old);
        assert_eq!(laid.light.exposure, 0.0);
        assert_eq!(
            laid.light.baseline, None,
            "the frame's own zero, its body's"
        );
        // Made from an older edit, or read from a file made before.
        let from_old = Preset::from_edit("Bright", &old, &[Section::Light]);
        assert_eq!(from_old.edit.light.baseline, None);
        assert_eq!(from_old.edit.light.exposure, 0.4);
        let file = serde_json::json!({
            "version": 5, "name": "Bright", "sections": ["light"],
            "edit": {"light": {"exposure": 0.4}},
        });
        let read = Preset::from_json(&file.to_string()).unwrap();
        assert_eq!(read.edit.light.baseline, None);
        assert_eq!(read.edit.light.exposure, 0.4);
        // Between frames the section carries the zero its numbers were
        // set against.
        let mut onto = Edit::default();
        Section::Light.copy(&old, &mut onto);
        assert_eq!(onto.light.baseline, Some(baseline::BEFORE_BODIES));
    }

    #[test]
    fn between_frames_the_zero_goes_with_the_exposure_and_is_no_setting_to_match() {
        use preset::{Choice, Part};
        let mut old = Edit::default();
        old.light.exposure = 0.3;
        old.light.baseline = Some(baseline::BEFORE_BODIES);
        let exposure = Choice {
            sections: Vec::new(),
            parts: vec![Part::LightExposure],
            masks: None,
            masks_land: Default::default(),
        };
        for choice in [Choice::whole(&[Section::Light]), exposure] {
            let p = Preset::between_frames(&old, &choice);
            assert_eq!(p.edit.light.baseline, old.light.baseline);
            let laid = p.applied(&Edit::default());
            assert_eq!(laid.light.exposure, 0.3);
            assert_eq!(laid.light.baseline, old.light.baseline);
            // Saved, the same choice carries none, and lays none.
            let saved = Preset::from_choice("x", &old, &choice);
            assert_eq!(saved.edit.light.baseline, None);
            assert_eq!(saved.applied(&old).light.baseline, None);
            // A preset whose sliders an older edit matches is applied.
            assert!(saved.is_applied(&old), "{choice:?}");
        }
        // Without the light, nothing of it goes.
        let color = Preset::between_frames(&old, &Choice::whole(&[Section::Color]));
        assert_eq!(color.edit.light.baseline, None);
    }
}
