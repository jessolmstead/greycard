//! A state as the file writes it, and as its id is made from (§724):
//! the edit's JSON with every field equal to its container's default
//! left out. A reader fills what is left out with serde's defaults,
//! which are the containers' own (the golden default holds them still:
//! a default never changes within a schema version). The rules:
//!
//! - `version` and `process` are always written: a missing one reads
//!   as 1. A default that changes with the build is no default to
//!   leave a field out against.
//! - Rules are by path in the edit, never by a key's name.
//! - A tagged enum's object (a white balance, a mask's local white, a
//!   crop's aspect, a shape), and a struct with no default to strip
//!   against (a crop, a pick, a person), leaves out each key it reads
//!   the same without, read by its type ([`read_at`]): its tag and its
//!   variant's needed fields stay, a field with a default goes when
//!   at it. So a field added later at its default changes no id here
//!   either.
//! - A list is written whole, each element of a list of structs left
//!   with what differs from its own type's default (an adjustment, a
//!   mask's component, a brush stroke, a repair patch and its further
//!   strokes), or by reading it as above (a pick).
//! - Keys are sorted, and an id's text is written by our own writer
//!   ([`canonical`]), whatever `serde_json`'s map keeps.
//!
//! Also the reader for a later build's differences: a history entry
//! written as `{"delta": …}` against an earlier entry, made whole here
//! before anything reads it ([`whole_history`]). No build writes one
//! yet; the reader is strict, so a delta it does not wholly understand
//! loses that state, never reads as a guess.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::{Adjustment, Edit, brush, mask, retouch};

/// The defaults a state is written against.
struct Defaults {
    edit: Value,
    adjustment: Value,
    component: Value,
    stroke: Value,
    patch: Value,
    patch_stroke: Value,
}

static DEFAULTS: LazyLock<Defaults> = LazyLock::new(|| Defaults {
    edit: full_value(&Edit::default()),
    adjustment: full_value(&Adjustment::default()),
    component: full_value(&mask::Component::default()),
    stroke: full_value(&brush::Stroke::default()),
    patch: full_value(&retouch::Patch::default()),
    patch_stroke: full_value(&retouch::Stroke::default()),
});

/// Where a component's shape is.
const SHAPE: &str = "adjustments[].mask.components[].shape";

impl Defaults {
    /// The default of an element of the list at `path`, a list of
    /// structs with a default: none for any other list.
    fn element(&self, path: &str) -> Option<&Value> {
        match path {
            "adjustments" => Some(&self.adjustment),
            "adjustments[].mask.components" => Some(&self.component),
            "adjustments[].mask.components[].shape.strokes" => Some(&self.stroke),
            "retouch.patches" => Some(&self.patch),
            "retouch.patches[].more" => Some(&self.patch_stroke),
            _ => None,
        }
    }
}

/// How a value of the type at a path reads, as its full value: none
/// when it does not read.
type Read = fn(&Value) -> Option<Value>;

fn read_as<T: DeserializeOwned + Serialize>(v: &Value) -> Option<Value> {
    T::deserialize(v).ok().map(|t| full_value(&t))
}

fn read_crop(v: &Value) -> Option<Value> {
    crate::geometry::crop_or_none(v)
        .ok()?
        .map(|c| full_value(&c))
}

/// The value at `path` whose keys are left out by reading it
/// ([`strip_read`]): a tagged enum, whose fields' defaults are its
/// variant's, or a struct whose default is none (an `Option`'s, or a
/// list's element with no default of its own). Every other object is
/// stripped against its default in its container's.
fn read_at(path: &str) -> Option<Read> {
    Some(match path {
        "white_balance" => read_as::<crate::WhiteBalance>,
        "geometry.aspect" => read_as::<crate::geometry::Aspect>,
        "geometry.crop" => read_crop,
        "adjustments[].look.white_balance" => read_as::<crate::white::LocalWhite>,
        SHAPE => read_as::<mask::Shape>,
        "adjustments[].mask.components[].shape.picks[]" => read_as::<mask::Pick>,
        "adjustments[].mask.components[].shape.person" => read_as::<mask::Person>,
        "adjustments[].mask.components[].shape.person.signature" => read_as::<mask::Signature>,
        _ => return None,
    })
}

/// Every path with a rule of its own: for the test that each is one a
/// state can have.
#[cfg(test)]
pub(crate) const RULED: [&str; 13] = [
    "adjustments",
    "adjustments[].mask.components",
    "adjustments[].mask.components[].shape.strokes",
    "retouch.patches",
    "retouch.patches[].more",
    "white_balance",
    "geometry.aspect",
    "geometry.crop",
    "adjustments[].look.white_balance",
    SHAPE,
    "adjustments[].mask.components[].shape.picks[]",
    "adjustments[].mask.components[].shape.person",
    "adjustments[].mask.components[].shape.person.signature",
];

/// `t` as its compact JSON reads back: every number as printed, never
/// widened (a value made by `to_value` holds an f32 as the f64 it is,
/// and prints its every digit).
pub(crate) fn full_value<T: Serialize>(t: &T) -> Value {
    let text = serde_json::to_string(t).expect("a state serializes");
    serde_json::from_str(&text).expect("a state's JSON reads back")
}

/// The state `edit`, as written: what differs from the default.
pub(crate) fn written_value(edit: &Edit) -> Value {
    let mut v = full_value(edit);
    #[cfg(test)]
    if WHOLE.get() {
        return v;
    }
    strip(&mut v, Some(&DEFAULTS.edit), &mut String::new());
    v
}

/// [`written_value`] as compact text, keys in order ([`canonical`]):
/// what a state's id is made from.
pub(crate) fn written_text(edit: &Edit) -> String {
    let mut out = String::new();
    canonical(&written_value(edit), &mut out);
    out
}

/// `v` as compact JSON, every object's keys in byte order whatever
/// order `serde_json`'s map holds them in: what an id hashes, so no
/// feature a dependency turns on (`preserve_order`) changes an id. A
/// number prints as `serde_json` prints it, the shortest text that
/// reads back as it (`ryu`); `numbers_print_as_written` holds that.
pub(crate) fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Array(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            let mut keys: Vec<(&String, &Value)> = o.iter().collect();
            keys.sort_unstable_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (k, x)) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("a key prints"));
                out.push(':');
                canonical(x, out);
            }
            out.push('}');
        }
        _ => out.push_str(&v.to_string()),
    }
}

#[cfg(test)]
thread_local! {
    /// States written whole, as before §724 left defaults out: for the
    /// tests that hold the two to one reading.
    pub(crate) static WHOLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A state's written value, kept with the edit it was made from. A
/// state is never rewritten, so a save makes again only the states
/// that are new; one changed in place since is made again, since the
/// edit kept is asked first.
#[derive(Default)]
pub(crate) struct Cache(Mutex<Option<Arc<(Edit, Value)>>>);

impl Cache {
    /// `edit`'s written value ([`written_value`]), made once.
    pub(crate) fn written(&self, edit: &Edit) -> Arc<(Edit, Value)> {
        #[cfg(test)]
        if WHOLE.get() {
            return Arc::new((edit.clone(), written_value(edit)));
        }
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(made) = slot.as_ref()
            && made.0 == *edit
        {
            return made.clone();
        }
        let made = Arc::new((edit.clone(), written_value(edit)));
        *slot = Some(made.clone());
        made
    }
}

impl Clone for Cache {
    fn clone(&self) -> Self {
        let slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        Cache(Mutex::new(slot.clone()))
    }
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cache")
    }
}

/// Every key of `v` equal to its default in `d` left out, recursively;
/// `path` is where `v` is in the edit, a list's element as `[]`, and
/// says which rule an object or a list takes ([`Defaults::element`],
/// [`read_at`]). `version` and `process` are always written: a missing
/// one reads as 1.
fn strip(v: &mut Value, d: Option<&Value>, path: &mut String) {
    match v {
        Value::Array(items) => {
            let e = DEFAULTS.element(path);
            let len = path.len();
            path.push_str("[]");
            for x in items {
                strip(x, e, path);
            }
            path.truncate(len);
        }
        Value::Object(o) => {
            let read = read_at(path);
            let d = if read.is_some() {
                None
            } else {
                d.and_then(Value::as_object)
            };
            if let Some(d) = d {
                let root = path.is_empty();
                o.retain(|k, x| {
                    (root && (k == "version" || k == "process")) || d.get(k) != Some(x)
                });
            }
            for (k, x) in o.iter_mut() {
                let len = path.len();
                if len > 0 {
                    path.push('.');
                }
                path.push_str(k);
                strip(x, d.and_then(|d| d.get(k)), path);
                path.truncate(len);
            }
            if let Some(read) = read {
                strip_read(o, read);
            }
        }
        _ => {}
    }
}

/// Each key of `o` left out that the value reads the same without, as
/// its type reads it: one with a default its variant or its struct
/// fills, never a tag or a field that must be there. Exact by
/// construction, each removal read back; nothing is left out of a
/// value that does not read.
fn strip_read(o: &mut Map<String, Value>, read: Read) {
    let mut at = Value::Object(std::mem::take(o));
    if let Some(want) = read(&at) {
        strip_read_at(&mut at, "", &want, read);
    }
    *o = match at {
        Value::Object(m) => m,
        _ => Map::new(),
    };
}

/// [`strip_read`] at the object `pointer` names in `at`, and in each
/// object under it that stays: a key goes when `at` reads as `want`
/// without it.
fn strip_read_at(at: &mut Value, pointer: &str, want: &Value, read: Read) {
    let keys: Vec<String> = match at.pointer(pointer) {
        Some(Value::Object(o)) => o.keys().cloned().collect(),
        _ => return,
    };
    for k in keys {
        let Some(Value::Object(o)) = at.pointer_mut(pointer) else {
            return;
        };
        let Some(x) = o.remove(&k) else {
            continue;
        };
        if read(at).as_ref() == Some(want) {
            continue;
        }
        let nested = x.is_object();
        if let Some(Value::Object(o)) = at.pointer_mut(pointer) {
            o.insert(k.clone(), x);
        }
        if nested {
            let escaped = k.replace('~', "~0").replace('/', "~1");
            strip_read_at(at, &format!("{pointer}/{escaped}"), want, read);
        }
    }
}

/// The keys a history entry written as a difference may carry beside
/// its `delta`.
const DELTA_KEYS: [&str; 6] = ["delta", "base", "id", "parent", "step", "exported"];

/// A line's history as written, each entry written as a difference
/// made whole against its base: the entry `base` names, else its
/// parent, which must be an earlier entry of the history, itself whole
/// or made whole. One that cannot be made whole (its base missing, or
/// later, or itself unreadable; a difference this reader does not
/// wholly understand) is left out, and so is every one made from it:
/// how many, the second of the pair. Nothing else is touched.
///
/// Each base is found by its id in a map, so a history reads in time
/// linear in its length. What is made whole is held to `budget` bytes,
/// shared by every line of the file ([`BUDGET`]): a difference past it
/// is lost too, so a file of a few small differences on one large
/// state cannot ask for more memory than that. Nesting is held by the
/// parse (serde_json's 128 levels), and an element index by its list.
pub(crate) fn whole_history(history: Vec<Value>, budget: &mut usize) -> (Vec<Value>, usize) {
    let mut made: Vec<Value> = Vec::with_capacity(history.len());
    // Each entry's size as [`size_of`] counts it, a difference's as its
    // base's and its own: so a difference is weighed before its base
    // is copied.
    let mut sizes: Vec<usize> = Vec::with_capacity(history.len());
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut lost = 0;
    for entry in history {
        let whole = if entry.get("delta").is_none() {
            let size = size_of(&entry);
            Some((entry, size))
        } else if *budget == 0 {
            // Spent: every later difference is lost, unread.
            None
        } else {
            made_whole(&entry, &made, &sizes, &at, budget)
        };
        match whole {
            Some((whole, size)) => {
                if let Some(id) = whole.get("id").and_then(Value::as_str) {
                    at.entry(id.to_string()).or_insert(made.len());
                }
                made.push(whole);
                sizes.push(size);
            }
            None => lost += 1,
        }
    }
    if lost > 0 {
        log::warn!("{lost} states written as differences could not be read, and are left out");
    }
    (made, lost)
}

/// What the differences of one file may make whole, in bytes as
/// [`size_of`] counts them: some fifty times the largest file a build
/// writes today.
pub(crate) const BUDGET: usize = 64 << 20;

/// About how many bytes `v` holds: its strings and keys, and eight for
/// every other value.
fn size_of(v: &Value) -> usize {
    match v {
        Value::String(s) => s.len() + 8,
        Value::Array(a) => a.iter().map(size_of).sum::<usize>() + 8,
        Value::Object(o) => o.iter().map(|(k, x)| k.len() + size_of(x)).sum::<usize>() + 8,
        _ => 8,
    }
}

/// The entry `entry` made whole against its base among `made`, found
/// by id in `at` (the first entry with that id), with its size: none
/// when it cannot be, or when its base and it would pass what is left
/// of `budget`, which is then spent.
fn made_whole(
    entry: &Value,
    made: &[Value],
    sizes: &[usize],
    at: &HashMap<String, usize>,
    budget: &mut usize,
) -> Option<(Value, usize)> {
    let o = entry.as_object()?;
    if o.keys().any(|k| !DELTA_KEYS.contains(&k.as_str())) {
        return None;
    }
    let base_id = match o.get("base") {
        Some(b) => b.as_str()?,
        None => o.get("parent")?.as_str()?,
    };
    let i = *at.get(base_id)?;
    // What making it costs: the base's copy and the difference's merge.
    let cost = sizes[i].saturating_add(size_of(entry));
    if cost > *budget {
        *budget = 0;
        return None;
    }
    *budget -= cost;
    let base = &made[i];
    let mut whole = base.clone();
    let w = whole.as_object_mut()?;
    for k in DELTA_KEYS {
        w.remove(k);
    }
    let delta = o.get("delta")?.as_object()?;
    merge(w, delta)?;
    for k in ["id", "parent", "step", "exported"] {
        if let Some(v) = o.get(k) {
            w.insert(k.into(), v.clone());
        }
    }
    // Its own size, for what is made from it: walked, as the copy was.
    let size = size_of(&whole);
    Some((whole, size))
}

/// `delta` merged into the object `target`: key by key, `$del` taking
/// keys out; None for anything else beginning with `$`.
fn merge(target: &mut Map<String, Value>, delta: &Map<String, Value>) -> Option<()> {
    for (k, d) in delta {
        if k == "$del" {
            for key in d.as_array()? {
                target.remove(key.as_str()?);
            }
            continue;
        }
        if k.starts_with('$') {
            return None;
        }
        let slot = target.entry(k.clone()).or_insert(Value::Null);
        apply(slot, d)?;
    }
    Some(())
}

/// The difference `d` applied to the value `target`: a list's
/// `{"$+": [..]}` appends, `{"$at": {"3": d}}` changes element 3, an
/// object merges into an object (a missing one being empty), and any
/// other value takes the place of what was there.
fn apply(target: &mut Value, d: &Value) -> Option<()> {
    let Some(o) = d.as_object() else {
        if has_dollar(d) {
            return None;
        }
        *target = d.clone();
        return Some(());
    };
    if o.len() == 1
        && let Some(add) = o.get("$+")
    {
        let list = target.as_array_mut()?;
        let add = add.as_array()?;
        if add.iter().any(has_dollar) {
            return None;
        }
        list.extend(add.iter().cloned());
        return Some(());
    }
    if o.len() == 1
        && let Some(at) = o.get("$at")
    {
        let list = target.as_array_mut()?;
        for (i, d) in at.as_object()? {
            let i: usize = i.parse().ok()?;
            apply(list.get_mut(i)?, d)?;
        }
        return Some(());
    }
    if target.is_null() {
        *target = Value::Object(Map::new());
    }
    merge(target.as_object_mut()?, o)
}

/// Whether a key in `v` begins with `$`: a difference's own words,
/// which a value put in whole cannot carry.
fn has_dollar(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.iter().any(|(k, x)| k.starts_with('$') || has_dollar(x)),
        Value::Array(a) => a.iter().any(has_dollar),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
