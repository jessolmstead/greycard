//! What a join of any two copies must hold, by [`join`]'s own word:
//! what the `join_json` fuzz target asserts of two sidecars it reads,
//! and what the tests here assert of pairs made by hand. Behind the
//! `testing` feature, which the fuzz targets turn on, so it never
//! reaches a release build.

use std::collections::HashSet;

use super::join;
use crate::Sidecar;

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
pub fn check(a: &Sidecar, b: &Sidecar) {
    holds_each_once("a", &filled(a));
    holds_each_once("b", &filled(b));
    let ab = join(a, b);
    let ba = join(b, a);
    for (why, j) in [("a with b", &ab), ("b with a", &ba)] {
        joined_right(why, j);
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
    const HOSTILE: [&str; 10] = [
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
        let mut checked = 0;
        while checked < pairs {
            let mut two = [
                seeds[below(seeds.len())].clone(),
                seeds[below(seeds.len())].clone(),
            ];
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
