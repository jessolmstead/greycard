//! What a join of any two copies must hold, by [`join`]'s own word:
//! what the `join_json` fuzz target asserts of two sidecars it reads,
//! and what the tests here assert of pairs made by hand. Behind the
//! `testing` feature, which the fuzz targets turn on, so it never
//! reaches a release build.

use std::collections::HashSet;

use super::join;
use crate::{Line, Sidecar};

/// Every invariant below of `a` and `b`, each joined both ways, with
/// itself, and twice: a panic names the one that broke.
///
/// - No line id held twice in a join (§724): a join places each line
///   once, by its id. Nor a version id twice, nor an id on a line
///   nobody developed, which writes none. Each copy as read and given
///   its ids, as a read from a file is, holds the same.
/// - The same join each time, and the same from either side: two
///   machines joining the same pair make the same sidecar whichever
///   side each calls local.
/// - Each copy's original line held in the join, wherever it sits,
///   unless the other copy has seen that copy's last revision.
/// - No tombstone for what the join holds: a version or a snapshot.
/// - The versions in the order made.
/// - A copy joined with itself holds what it held: every line, each
///   with the same states, the same meta, and the same turn; and that
///   join joined with itself is the same again.
/// - No state Clear history took out on either copy ([`Line::cleared`])
///   in the join, but a copy's first state or its current one; each
///   copy's current state still in the join, unless the other has seen
///   that copy's last revision; and the tombstones take nothing else: the
///   join with them and without has the same snapshots, first state,
///   and current edit on every line.
///
/// [`Line::cleared`]: crate::Line::cleared
pub fn check(a: &Sidecar, b: &Sidecar) {
    holds_each_once("a", &filled(a));
    holds_each_once("b", &filled(b));
    let ab = join(a, b);
    let ba = join(b, a);
    for (why, j) in [("a with b", &ab), ("b with a", &ba)] {
        joined_right(why, j);
        nothing_cleared_comes_back(why, a, b, j);
        for (input, s, other) in [("a", a, b), ("b", b, a)] {
            let seen = s.revisions.last().is_some_and(|r| other.knows(&r.hash));
            if !seen {
                currents_kept(why, input, s, other, j);
            }
        }
        for (input, s, other) in [("a", a, b), ("b", b, a)] {
            // A copy the other has seen whole is in it as it chose to
            // keep it: what it lacks it dropped on purpose.
            let seen = s.revisions.last().is_some_and(|r| other.knows(&r.hash));
            let mut s = s.clone();
            s.fill_line_ids();
            let top = &s.line.line_id;
            assert!(
                seen || top.is_empty() || j.lines().any(|l| l.line_id == *top),
                "{why}: {input}'s original line {top} lost"
            );
        }
    }
    assert!(
        written(&ab) == written(&join(a, b)),
        "a with b joined twice differs"
    );
    tombstones_take_only_history(a, b, &ab);
    assert!(written(&ab) == written(&ba), "a with b is not b with a");
    for (why, s) in [("a with itself", a), ("b with itself", b)] {
        let j = join(s, s);
        joined_right(why, &j);
        let s = written(&filled(s));
        for line in s.lines() {
            let held = j
                .lines()
                .find(|l| l.line_id == line.line_id)
                .unwrap_or_else(|| panic!("{why}: the line {} lost", line.line_id));
            assert!(
                super::state_ids(held) == super::state_ids(line),
                "{why}: the line {} changed its states",
                line.line_id
            );
        }
        assert!(j.meta == s.meta, "{why}: the meta changed");
        assert!(j.turn == s.turn, "{why}: the turn changed");
        // What the join tidies of a file no build writes (the versions
        // put in the order made, a record held twice taken once) it has
        // tidied once: the join with itself again is the same.
        let again = written(&join(&j, &j));
        assert!(
            again == written(&j),
            "{why}: joined with itself again changed"
        );
    }
}

/// No state of the join's lines that either copy's line of that id
/// cleared, but a first state or a current one of either copy's.
fn nothing_cleared_comes_back(why: &str, a: &Sidecar, b: &Sidecar, j: &Sidecar) {
    let (a, b) = (filled(a), filled(b));
    for line in j.lines().filter(|l| !l.line_id.is_empty()) {
        let inputs: Vec<&Line> = [&a, &b]
            .into_iter()
            .flat_map(|s| s.lines())
            .filter(|l| l.line_id == line.line_id)
            .collect();
        let gone: HashSet<&str> = inputs
            .iter()
            .flat_map(|l| l.cleared.keys().map(String::as_str))
            .collect();
        if gone.is_empty() {
            continue;
        }
        let kept: HashSet<&str> = inputs
            .iter()
            .flat_map(|l| {
                l.history
                    .first()
                    .and_then(|s| s.id())
                    .into_iter()
                    .chain(l.current_id())
            })
            .collect();
        for s in &line.history {
            if let Some(id) = s.id() {
                assert!(
                    !gone.contains(id) || kept.contains(id),
                    "{why}: the cleared state {id} came back on the line {}",
                    line.line_id
                );
            }
        }
    }
}

/// The current state of `s`'s original line among its joined line's
/// states, as written: an edit made on that copy is never lost to a
/// join that has not seen it. Unless `other` holds a state of the same
/// id with other content: a join takes one id for one state, and two
/// contents under one id (a file no build writes) are not this check's.
fn currents_kept(why: &str, input: &str, s: &Sidecar, other: &Sidecar, j: &Sidecar) {
    let (s, other) = (written(&filled(s)), written(&filled(other)));
    let (j, line) = (written(j), &s.line);
    if !line.recorded() || line.line_id.is_empty() {
        return;
    }
    let id = line.current_id();
    let clash = other.lines().any(|l| {
        l.history
            .iter()
            .chain(&l.redo)
            .map(|s| (s.id(), s.edit()))
            .chain([(l.current_id(), l.current())])
            .any(|(i, e)| i.is_some() && i == id && e != line.current())
    });
    if clash {
        return;
    }
    let Some(held) = j.lines().find(|l| l.line_id == line.line_id) else {
        return;
    };
    assert!(
        held.edits().any(|e| e == line.current()),
        "{why}: {input}'s current state lost from the line {}",
        line.line_id
    );
}

/// The join of `a` and `b` with every tombstone of Clear history and
/// without: the same snapshots, first state, and current edit on every
/// line, the tombstones taking only history. Where the tombstones are
/// all that tells two copies apart, which is the newer is theirs to
/// say, and the two joins are not compared.
fn tombstones_take_only_history(a: &Sidecar, b: &Sidecar, ab: &Sidecar) {
    let bare = |s: &Sidecar| {
        let mut s = s.clone();
        for line in s.lines_mut() {
            line.cleared.clear();
        }
        s
    };
    let (bare_a, bare_b) = (bare(a), bare(b));
    if bare_a == *a && bare_b == *b {
        return;
    }
    if super::newer_of(a, b) != super::newer_of(&bare_a, &bare_b) {
        return;
    }
    let (ab, bare_ab) = (written(ab), written(&join(&bare_a, &bare_b)));
    for line in ab.lines() {
        let Some(other) = bare_ab.lines().find(|l| l.line_id == line.line_id) else {
            continue;
        };
        assert!(
            line.snapshots == other.snapshots,
            "a tombstone took a snapshot on the line {}",
            line.line_id
        );
        assert!(
            line.history.first().and_then(|s| s.id()) == other.history.first().and_then(|s| s.id()),
            "a tombstone took the first state on the line {}",
            line.line_id
        );
        assert!(
            line.current() == other.current(),
            "a tombstone moved the current state on the line {}",
            line.line_id
        );
    }
}

/// No line and no version held twice, and no id on a line nobody
/// developed: what a copy read holds too.
fn holds_each_once(why: &str, j: &Sidecar) {
    let mut lines = HashSet::new();
    for l in j.lines() {
        assert!(
            l.recorded() || l.line_id.is_empty(),
            "{why}: a line nobody developed holds the id {}",
            l.line_id
        );
        assert!(
            l.line_id.is_empty() || lines.insert(l.line_id.as_str()),
            "{why}: the line {} held twice",
            l.line_id
        );
    }
    let mut versions = HashSet::new();
    for v in &j.versions {
        assert!(
            versions.insert(v.id.as_str()),
            "{why}: the version {} held twice",
            v.id
        );
    }
}

/// [`holds_each_once`] of a join, no tombstone for what it holds,
/// which a file may have, and its versions in the order made.
fn joined_right(why: &str, j: &Sidecar) {
    holds_each_once(why, j);
    for l in j.lines() {
        for s in &l.snapshots {
            assert!(
                !l.snapshots_removed.contains_key(&s.id),
                "{why}: the snapshot {} held and tombstoned",
                s.id
            );
        }
    }
    for v in &j.versions {
        assert!(
            !j.removed.contains_key(&v.id),
            "{why}: the version {} held and tombstoned",
            v.id
        );
    }
    assert!(
        j.versions
            .windows(2)
            .all(|w| (w[0].made, &w[0].id) <= (w[1].made, &w[1].id)),
        "{why}: the versions out of order"
    );
}

/// `s` with its ids given, as a join gives them to both copies first.
fn filled(s: &Sidecar) -> Sidecar {
    let mut s = s.clone();
    s.fill_ids();
    s.pin_times();
    s
}

/// `s` with every state as written in place of its shown copy: what a
/// join goes by (§724), since two copies may show one state under two
/// turns.
fn written(s: &Sidecar) -> Sidecar {
    let mut s = s.clone();
    for line in s.lines_mut() {
        *line = std::mem::take(line).into_written();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::check;
    use crate::{Edit, Sidecar};

    fn read(json: &str) -> Sidecar {
        Sidecar::from_json(json, &Edit::default()).unwrap()
    }

    /// `s` written and read back: the same file, as another machine
    /// reads it.
    fn reread(s: &Sidecar) -> Sidecar {
        read(&s.to_json())
    }

    /// Pairs made to break a join: line ids shared across the two
    /// copies in every place a line can sit, a version id holding
    /// another line on each side, and a file holding one line id more
    /// than once. A state at version 10 is a recorded one; `{}`, at
    /// version 1 and at the start, reads as a line nobody developed.
    const HOSTILE: [&str; 12] = [
        // A line id held twice in one file (no build writes one), the
        // first time on a line read as nobody developed.
        r#"{"current":{},"line_id":"9e3779b97f4a7c15","versions":[
            {"id":"9e3779b97f4a7c15","line":{"current":{"version":10,"light":{"exposure":9}}}}]}"#,
        r#"{"current":{"version":10},"line_id":"9e3779b97f4a7c15","versions":[
            {"id":"9e3779b97f4a7c15","line":{"current":{"version":10,"light":{"exposure":9}}}}]}"#,
        // Three times, and once under the id the second is made anew as.
        r#"{"current":{"version":10},"line_id":"9e3779b97f4a7c15","versions":[
            {"id":"a1","line":{"current":{"version":10,"light":{"exposure":1}},
                "line_id":"c518635dc6eea7e0"}},
            {"id":"a2","line":{"current":{"version":10,"light":{"exposure":2}},
                "line_id":"9e3779b97f4a7c15"}},
            {"id":"a3","line":{"current":{"version":10,"light":{"exposure":3}},
                "line_id":"9e3779b97f4a7c15"}}]}"#,
        // The original's line held by a version on the other copy, and
        // the other way about.
        r#"{"current":{"version":10,"light":{"exposure":0.5}},"line_id":"9e3779b97f4a7c15",
            "versions":[{"id":"b1","line":{"current":{"version":10},
                "line_id":"3c6ef372fe94f82a"}}]}"#,
        r#"{"current":{"version":10,"light":{"exposure":0.7}},"line_id":"3c6ef372fe94f82a",
            "versions":[{"id":"b1","line":{"current":{"version":10},
                "line_id":"9e3779b97f4a7c15"}}]}"#,
        // A version on both under one id, a different line on each.
        r#"{"current":{"version":10},"versions":[{"id":"c1","made":5,
            "line":{"current":{"version":10,"light":{"exposure":1}},
                "line_id":"daa66d2c7ddf743f"}}]}"#,
        r#"{"current":{"version":10},"versions":[{"id":"c1","made":5,
            "line":{"current":{"version":10,"light":{"exposure":2}},
                "line_id":"f9a2b1c3d4e5f607"}}]}"#,
        // Versions deleted here and still on another copy, and a
        // version whose id is another copy's line's.
        r#"{"current":{"version":10},"removed":{"c1":9,"b1":9},
            "versions":[{"id":"daa66d2c7ddf743f",
                "line":{"current":{"version":10,"light":{"exposure":3}}}}]}"#,
        // A line nobody developed, with a version holding an original's
        // line id.
        r#"{"versions":[{"id":"c1","line":{"current":{"version":10,"light":{"exposure":4}},
            "line_id":"9e3779b97f4a7c15"}}]}"#,
        // Swapped later, its original another copy's version's line.
        r#"{"current":{"version":10,"light":{"exposure":1}},"line_id":"daa66d2c7ddf743f",
            "original_at":7,"versions":[{"id":"c1","made":5,
            "line":{"current":{"version":10},"line_id":"9e3779b97f4a7c15"}}]}"#,
        // A history cleared on one copy and whole on the other, the
        // tombstones also naming the other's current state and a state
        // of a version.
        r#"{"current":{"version":10,"light":{"exposure":3}},"id":"s3","parent":"s2",
            "history":[{"version":10,"id":"s0"}],"line_id":"9e3779b97f4a7c15",
            "cleared":{"s1":9,"s2":9,"t1":9},
            "versions":[{"id":"c1","made":5,"line":{"current":{"version":10,"light":{"exposure":5}},
                "history":[{"version":10,"id":"t0"}],"cleared":{"t1":9},
                "line_id":"3c6ef372fe94f82a"}}]}"#,
        r#"{"current":{"version":10,"light":{"exposure":2}},"id":"s2","parent":"s1",
            "history":[{"version":10,"id":"s0"},
                {"version":10,"light":{"exposure":1},"id":"s1","parent":"s0"}],
            "line_id":"9e3779b97f4a7c15",
            "versions":[{"id":"c1","made":5,"line":{"current":{"version":10,"light":{"exposure":5}},
                "history":[{"version":10,"id":"t0"},{"version":10,"light":{"exposure":4},"id":"t1"}],
                "line_id":"3c6ef372fe94f82a"}}]}"#,
    ];

    /// Every pair of the hostile files, each also against itself read
    /// back on another machine.
    #[test]
    fn a_join_of_hostile_copies_holds_each_line_once() {
        let files: Vec<Sidecar> = HOSTILE.iter().map(|j| read(j)).collect();
        for (i, a) in files.iter().enumerate() {
            check(a, &reread(a));
            for b in &files[i..] {
                check(a, b);
            }
        }
    }

    /// A history cleared on one copy and whole on the other: the join
    /// either way holds none of the cleared states but the other copy's
    /// current one, on the original and on the version, and keeps the
    /// tombstones of what it does not hold.
    #[test]
    fn a_join_drops_what_either_copy_cleared_but_a_current_state() {
        let (a, b) = (read(HOSTILE[10]), read(HOSTILE[11]));
        check(&a, &b);
        for j in [super::join(&a, &b), super::join(&b, &a)] {
            let ids = super::super::state_ids(&j.line);
            assert!(!ids.iter().any(|i| i == "s1"), "{ids:?}");
            assert!(ids.iter().any(|i| i == "s2") && ids.iter().any(|i| i == "s3"));
            assert_eq!(
                j.line.cleared.keys().collect::<Vec<_>>(),
                ["s1", "t1"],
                "s2 is held, so not tombstoned"
            );
            let v = super::super::state_ids(&j.versions[0].line);
            assert!(!v.iter().any(|i| i == "t1"), "{v:?}");
        }
    }

    /// A line a build before version 10 wrote untouched reads as one
    /// nobody developed (§724), and so with no id: one kept from the
    /// file was held by no line written, and a join with the copy read
    /// back placed the version of that id on the original, its edit
    /// with it, or the other way about held the version twice.
    #[test]
    fn a_line_read_as_nobody_developed_holds_no_id() {
        let s = read(HOSTILE[0]);
        assert!(!s.line.recorded());
        assert_eq!(s.line.line_id, "");
        let back = reread(&s);
        for j in [super::join(&s, &back), super::join(&back, &s)] {
            assert!(!j.line.recorded());
            assert_eq!(j.line.current().light.exposure, 0.0);
            assert_eq!(j.versions.len(), 1);
            assert_eq!(j.versions[0].line.current().light.exposure, 9.0);
        }
    }

    /// Two copies nothing told apart (no revision, one time, an
    /// original nobody developed) but a version each holding one line
    /// were joined as the first said first: the join from one side
    /// stacked the two the other way about from the join from the
    /// other. The file as written tells them apart now.
    #[test]
    fn copies_told_apart_only_by_their_versions_join_alike_from_either_side() {
        let (a, b) = (read(HOSTILE[0]), read(HOSTILE[8]));
        let exposures = |j: &Sidecar| -> Vec<f32> {
            let line = &j.versions[0].line;
            line.history
                .iter()
                .map(|s| s.edit().light.exposure)
                .chain([line.current().light.exposure])
                .collect()
        };
        assert_eq!(
            exposures(&super::join(&a, &b)),
            exposures(&super::join(&b, &a))
        );
    }

    /// The hostile files and the fixtures, mutated: values carried
    /// from one file to a place in another, an id copied over another,
    /// a key or an element taken out or doubled, and every pair that
    /// reads checked. What the fuzz target does by its own mutations,
    /// for a build with no cargo-fuzz; `GREYCARD_JOIN_SWEEP` sets how
    /// many pairs (a few hundred by default).
    #[test]
    fn mutated_copies_join_as_a_join_must() {
        use serde_json::Value;
        let pairs: usize = std::env::var("GREYCARD_JOIN_SWEEP")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(300);
        let mut seeds: Vec<Value> = HOSTILE
            .iter()
            .chain(&[
                include_str!("../../fixtures/states-at-10.gcd"),
                include_str!("../../fixtures/sidecar-before-versions.gcd"),
            ])
            .map(|j| serde_json::from_str(j).unwrap())
            .collect();
        let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
        let mut below = |n: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % n.max(1) as u64) as usize
        };
        /// Every value in `v`, by its JSON pointer.
        fn pointers(v: &Value, at: String, out: &mut Vec<String>) {
            match v {
                Value::Object(o) => o.iter().for_each(|(k, x)| {
                    let k = k.replace('~', "~0").replace('/', "~1");
                    pointers(x, format!("{at}/{k}"), out)
                }),
                Value::Array(a) => a
                    .iter()
                    .enumerate()
                    .for_each(|(i, x)| pointers(x, format!("{at}/{i}"), out)),
                _ => {}
            }
            out.push(at);
        }
        /// Clear history on one line of `v` (the original's, or a
        /// version's), by the file: its history after the first state
        /// out, their ids tombstoned.
        fn clear(v: &mut Value, pick: usize) {
            let mut lines: Vec<&mut serde_json::Map<String, Value>> = Vec::new();
            let Value::Object(root) = v else {
                return;
            };
            let mut versions = root.remove("versions");
            lines.push(root);
            if let Some(Value::Array(vs)) = &mut versions {
                for v in vs {
                    if let Some(Value::Object(l)) = v.get_mut("line") {
                        lines.push(l);
                    }
                }
            }
            let n = lines.len();
            let line = &mut lines[pick % n];
            if let Some(Value::Array(history)) = line.get("history").cloned() {
                let ids: serde_json::Map<String, Value> = history
                    .iter()
                    .skip(1)
                    .filter_map(|s| s.get("id").and_then(Value::as_str))
                    .map(|id| (id.to_string(), Value::from(9)))
                    .collect();
                line.insert(
                    "history".into(),
                    Value::Array(history.into_iter().take(1).collect()),
                );
                line.insert("cleared".into(), Value::Object(ids));
            }
            if let (Some(vs), Value::Object(root)) = (versions, v) {
                root.insert("versions".into(), vs);
            }
        }
        let mut checked = 0;
        while checked < pairs {
            let mut two = [
                seeds[below(seeds.len())].clone(),
                seeds[below(seeds.len())].clone(),
            ];
            // One file in two places, cleared on one of them.
            if below(3) == 0 {
                two[1] = two[0].clone();
                let (side, pick) = (below(2), below(4));
                clear(&mut two[side], pick);
            }
            for _ in 0..1 + below(4) {
                let (to, from) = if below(2) == 0 { (0, 1) } else { (1, 0) };
                let mut there = Vec::new();
                pointers(&two[to], String::new(), &mut there);
                let mut here = Vec::new();
                pointers(&two[from], String::new(), &mut here);
                let at = there[below(there.len())].clone();
                let value = two[from].pointer(&here[below(here.len())]).cloned();
                let (parent, key) = at.rsplit_once('/').unwrap_or(("", ""));
                match (below(4), value) {
                    // Another file's value, put here.
                    (0 | 1, Some(value)) if !at.is_empty() => {
                        *two[to].pointer_mut(&at).unwrap() = value;
                    }
                    // Taken out.
                    (2, _) => match two[to].pointer_mut(parent) {
                        Some(Value::Object(o)) => {
                            o.remove(&key.replace("~1", "/").replace("~0", "~"));
                        }
                        Some(Value::Array(a)) => {
                            if let Ok(i) = key.parse::<usize>() {
                                a.remove(i);
                            }
                        }
                        _ => {}
                    },
                    // Doubled.
                    _ => {
                        if let Some(Value::Array(a)) = two[to].pointer_mut(parent)
                            && let Ok(i) = key.parse::<usize>()
                        {
                            let x = a[i].clone();
                            a.insert(i, x);
                        }
                    }
                }
            }
            let read = |v: &Value| Sidecar::from_value(v.clone(), &Edit::default()).ok();
            let (Some(a), Some(b)) = (read(&two[0]), read(&two[1])) else {
                continue;
            };
            let run = std::panic::catch_unwind(|| check(&a, &b));
            if let Err(e) = run {
                eprintln!("a: {}\nb: {}", two[0], two[1]);
                std::panic::resume_unwind(e);
            }
            checked += 1;
            // What read and joined goes on as a seed.
            if below(8) == 0 {
                seeds.push(two[below(2)].clone());
            }
        }
    }
}
