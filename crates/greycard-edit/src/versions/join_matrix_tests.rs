//! Two copies of one file, each moved apart by up to two steps (a
//! swap, a delete, a new version, a rename, an edit on any line, a
//! snapshot taken or renamed), joined both ways round, at schema 10 and
//! at 9 (§724): no state lost, each on the line it was made on, no line
//! held twice, no version entry or name gone that neither copy deleted,
//! the two orders alike, and a join met again with either input
//! unchanged. Then three steps against two, three copies, and three
//! machines joining in turn.

use crate::Sidecar;
use crate::sync::{self, Revision};
use crate::{Edit, Line};
use std::collections::{BTreeMap, BTreeSet};

fn exposed(stops: f32) -> Edit {
    let mut e = Edit::default();
    e.light.exposure = stops;
    e
}

fn base() -> Sidecar {
    let mut s = Sidecar::default();
    s.record(exposed(0.5));
    s.take_snapshot("Snapshot 1", 1_700_000_000);
    s.record(exposed(1.0));
    let v = s.new_version(None, Some("V")).unwrap();
    s.line_mut(Some(&v)).unwrap().record(exposed(3.0));
    let w = s.new_version(None, Some("W")).unwrap();
    s.line_mut(Some(&w)).unwrap().record(exposed(4.0));
    s.fill_ids();
    s
}

fn at_nine(json: &str) -> String {
    fn walk(v: &mut serde_json::Value, in_snap: bool) {
        match v {
            serde_json::Value::Object(o) => {
                for k in [
                    "floors",
                    "process",
                    "line_id",
                    "original_at",
                    "placed_under",
                    "named",
                    "snapshots_removed",
                    "removed",
                ] {
                    o.remove(k);
                }
                if in_snap {
                    o.remove("id");
                }
                if o.get("version") == Some(&serde_json::json!(crate::VERSION)) {
                    o.insert("version".into(), serde_json::json!(9));
                }
                for (k, x) in o.iter_mut() {
                    walk(x, k == "snapshots");
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(|x| walk(x, in_snap)),
            _ => {}
        }
    }
    let mut v: serde_json::Value = serde_json::from_str(json).unwrap();
    walk(&mut v, false);
    v.to_string()
}

fn stamp(s: &mut Sidecar, at: u64, host: &str) {
    s.revisions.push(Revision {
        hash: format!(
            "{:032x}",
            at * 1000 + host.bytes().map(u64::from).sum::<u64>()
        ),
        at,
        host: host.into(),
    });
}

fn xs(l: &Line) -> BTreeSet<u32> {
    l.history
        .iter()
        .map(|h| &h.edit)
        .chain([&l.current])
        .map(|e| (e.light.exposure * 100.0) as u32)
        .collect()
}

/// Every line: (line_id, markers).
fn proj(s: &Sidecar) -> Vec<(String, String, BTreeSet<u32>)> {
    let mut v = vec![("orig".to_string(), s.line.line_id.clone(), xs(&s.line))];
    for ver in &s.versions {
        v.push((ver.id.clone(), ver.line.line_id.clone(), xs(&ver.line)));
    }
    v
}

/// Ops: 0 swap V, 1 swap W, 2 delete V, 3 delete W, 4 new version,
/// 5 rename V, 6 edit original, 7 edit V, 8 edit W, 9 swap the
/// newest version, 10 take a snapshot, 11 rename the first snapshot.
fn apply(
    s: &mut Sidecar,
    op: u32,
    mark: &mut u32,
    lineage: &mut Vec<(u32, String)>,
    v: &str,
    w: &str,
    swap_at: &mut u64,
) {
    *mark += 1;
    let m = *mark as f32;
    let mut swap = |s: &mut Sidecar, id: &str| {
        if s.swap_with_original(id) {
            *swap_at += 10;
            s.original_at = *swap_at;
        }
    };
    match op {
        0 => swap(s, v),
        1 => swap(s, w),
        2 => {
            s.delete_version(v);
        }
        3 => {
            s.delete_version(w);
        }
        4 => {
            let id = s.new_version(None, Some("N")).unwrap();
            s.line_mut(Some(&id)).unwrap().record(exposed(m));
            let lid = s.version(&id).unwrap().line.line_id.clone();
            lineage.push((*mark, lid));
        }
        5 => {
            s.rename_version(v, &format!("V{mark}"));
        }
        6 => {
            s.record(exposed(m));
            s.fill_line_ids();
            lineage.push((*mark, s.line.line_id.clone()));
        }
        7 | 8 => {
            let id = if op == 7 { v } else { w };
            if let Some(mut l) = s.line_mut(Some(id)) {
                l.record(exposed(m));
            }
            s.fill_line_ids();
            if let Some(ver) = s.version(id) {
                lineage.push((*mark, ver.line.line_id.clone()));
            }
        }
        9 => {
            if let Some(id) = s.versions.last().map(|x| x.id.clone()) {
                swap(s, &id)
            }
        }
        10 => {
            s.take_snapshot(format!("S{mark}"), 1_800_000_000 + u64::from(*mark));
        }
        11 => {
            s.line.rename_snapshot(0, &format!("R{mark}"));
        }
        _ => {}
    }
}

/// Every sequence of up to `max` of the first `ops` moves.
fn seqs(max: usize, ops: u32) -> Vec<Vec<u32>> {
    let mut out = vec![vec![]];
    let mut last = vec![vec![]];
    for _ in 0..max {
        let mut next = vec![];
        for s in &last {
            for op in 0..ops {
                let mut t: Vec<u32> = s.clone();
                t.push(op);
                next.push(t);
            }
        }
        out.extend(next.iter().cloned());
        last = next;
    }
    out
}

struct Copy {
    s: Sidecar,
    lineage: Vec<(u32, String)>,
    ops: Vec<u32>,
}

fn make(text: &str, ops: &[u32], mark0: u32, swap0: u64, host: &str, at: u64) -> Copy {
    let mut s = Sidecar::from_json(text, &Edit::default()).unwrap();
    let (v, w) = (s.versions[0].id.clone(), s.versions[1].id.clone());
    let mut mark = mark0;
    let mut swap_at = swap0;
    let mut lineage = vec![];
    for &op in ops {
        apply(&mut s, op, &mut mark, &mut lineage, &v, &w, &mut swap_at);
    }
    stamp(&mut s, at, host);
    Copy {
        s,
        lineage,
        ops: ops.to_vec(),
    }
}

#[derive(Default)]
struct Tally {
    pairs: u64,
    lost: u64,
    misplaced: u64,
    dup_ids: u64,
    asym: u64,
    not_idem: u64,
    entry_gone: u64,
    name_gone: u64,
    examples: BTreeMap<&'static str, Vec<String>>,
}

impl Tally {
    fn note(&mut self, kind: &'static str, s: String) {
        let e = self.examples.entry(kind).or_default();
        if e.len() < 4 {
            e.push(s);
        }
    }
}

fn check(t: &mut Tally, a: &Copy, b: &Copy, label: &str) {
    t.pairs += 1;
    let j = sync::join(&a.s, &b.s);
    let j2 = sync::join(&b.s, &a.s);
    let p = proj(&j);
    if p != proj(&j2) {
        t.asym += 1;
        t.note("asym", format!("{label} a{:?} b{:?}", a.ops, b.ops));
    }
    // Loss: every marker of either input somewhere in the join.
    let all_j: BTreeSet<u32> = p.iter().flat_map(|(_, _, x)| x.iter().copied()).collect();
    let mut lost = BTreeSet::new();
    for c in [a, b] {
        for (_, _, x) in proj(&c.s) {
            for m in x {
                if !all_j.contains(&m) {
                    lost.insert(m);
                }
            }
        }
    }
    if !lost.is_empty() {
        t.lost += 1;
        t.note(
            "lost",
            format!(
                "{label} a{:?} b{:?} lost {lost:?}; a {:?} b {:?} j {:?}",
                a.ops,
                b.ops,
                proj(&a.s),
                proj(&b.s),
                p
            ),
        );
    }
    // Lineage: a marker made on the line of id L is on the line of id L.
    for c in [a, b] {
        for (m, lid) in &c.lineage {
            let m = m * 100;
            if lid.is_empty() {
                continue;
            }
            let ok = p.iter().any(|(_, id, x)| id == lid && x.contains(&m));
            if !ok && all_j.contains(&m) {
                t.misplaced += 1;
                t.note(
                    "misplaced",
                    format!(
                        "{label} a{:?} b{:?} marker {m} of {lid}; j {:?}",
                        a.ops, b.ops, p
                    ),
                );
            }
        }
    }
    let ids: Vec<&String> = p
        .iter()
        .map(|(_, id, _)| id)
        .filter(|x| !x.is_empty())
        .collect();
    let uniq: BTreeSet<&String> = ids.iter().copied().collect();
    if uniq.len() != ids.len() {
        t.dup_ids += 1;
        t.note(
            "dup",
            format!("{label} a{:?} b{:?} j {:?}", a.ops, b.ops, p),
        );
    }
    // A version entry either copy holds is in the join unless one of
    // them deleted it; with it, its name.
    for c in [a, b] {
        for v in &c.s.versions {
            if j.version(&v.id).is_some()
                || a.s.removed.contains_key(&v.id)
                || b.s.removed.contains_key(&v.id)
            {
                continue;
            }
            t.entry_gone += 1;
            if v.name.starts_with('V') && v.name.len() > 1 {
                t.name_gone += 1;
            }
            t.note(
                "entry_gone",
                format!(
                    "{label} a{:?} b{:?} {} {:?}; j {:?}",
                    a.ops,
                    b.ops,
                    v.name,
                    xs(&v.line),
                    p
                ),
            );
        }
    }
    // Joining again with either input changes no line.
    let mut j = j;
    stamp(&mut j, 300, "j");
    let ja = sync::join(&j, &a.s);
    let jb = sync::join(&j, &b.s);
    if proj(&ja) != p || proj(&jb) != p {
        t.not_idem += 1;
        t.note(
            "not_idem",
            format!(
                "{label} a{:?} b{:?} j {:?} ja {:?} jb {:?}",
                a.ops,
                b.ops,
                p,
                proj(&ja),
                proj(&jb)
            ),
        );
    }
}

/// Every sequence in `a_seqs` against every one in `b_seqs`, both
/// orders of which copy is newer, at schema 10 and, if `nine`, at 9:
/// every count zero.
fn every_pair(a_seqs: &[Vec<u32>], b_seqs: &[Vec<u32>], nines: &[bool]) {
    for &nine in nines {
        let b0 = base();
        let text = if nine {
            at_nine(&b0.to_json())
        } else {
            b0.to_json()
        };
        let mut t = Tally::default();
        for sa in a_seqs {
            for sb in b_seqs {
                for (ta, tb) in [(200, 100), (100, 200)] {
                    let a = make(&text, sa, 1000, 0, "a", ta);
                    let b = make(&text, sb, 2000, 5, "b", tb);
                    check(&mut t, &a, &b, if ta > tb { "a newer" } else { "b newer" });
                }
            }
        }
        assert!(
            t.lost + t.misplaced + t.dup_ids + t.asym + t.not_idem + t.entry_gone == 0,
            "nine {nine}: pairs {} lost {} misplaced {} dup_ids {} asym {} not_idem {} \
             entry_gone {} name_gone {}; {:#?}",
            t.pairs,
            t.lost,
            t.misplaced,
            t.dup_ids,
            t.asym,
            t.not_idem,
            t.entry_gone,
            t.name_gone,
            t.examples
        );
    }
}

/// Every pair of up to two steps on each copy (111 sequences, both
/// orders of which copy is newer), at schema 10 and at 9.
#[test]
fn every_pair_of_moves_joins_losing_nothing() {
    let all = seqs(2, 10);
    every_pair(&all, &all, &[false, true]);
}

/// Three steps of the twelve moves on one copy against up to one on
/// the other: the depth where one copy's version could take another's
/// line (review-6bc fix-check, MEDIUM-A) and a line kept as a version
/// took a random id (MEDIUM-B).
#[test]
fn three_moves_against_one_join_losing_nothing() {
    let three: Vec<Vec<u32>> = seqs(3, 12).into_iter().filter(|s| s.len() == 3).collect();
    every_pair(&three, &seqs(1, 12), &[false]);
}

/// The reviewer's widest setting: up to three of the twelve moves
/// against up to two, at 10 (591,890 pairs; minutes in release).
#[test]
#[ignore]
fn three_moves_against_two_join_losing_nothing() {
    every_pair(&seqs(3, 12), &seqs(2, 12), &[false]);
}

/// Each version's (name, markers), the original's as "orig".
fn named(s: &Sidecar) -> BTreeSet<(String, BTreeSet<u32>)> {
    std::iter::once(("orig".to_string(), xs(&s.line)))
        .chain(s.versions.iter().map(|v| (v.name.clone(), xs(&v.line))))
        .collect()
}

/// Copy c makes a version N while a and b each delete W, which held a
/// different line on each. However the three are joined, N keeps its
/// entry and its states: W's fallback never takes N's line
/// (MEDIUM-A).
#[test]
fn a_version_s_fallback_never_takes_another_version_s_line() {
    let text = base().to_json();
    let a = make(&text, &[1, 3], 1000, 0, "a", 100);
    let b = make(&text, &[3, 6], 2000, 5, "b", 101);
    let c = make(&text, &[4, 11], 3000, 7, "c", 102);
    let n_marker = 3001 * 100;
    let mut ab = sync::join(&a.s, &b.s);
    stamp(&mut ab, 300, "j");
    let abc = sync::join(&ab, &c.s);
    let mut bc = sync::join(&b.s, &c.s);
    stamp(&mut bc, 300, "j");
    let bca = sync::join(&bc, &a.s);
    for j in [&abc, &bca] {
        let n = j.versions.iter().find(|v| v.name == "N").expect("N kept");
        assert!(xs(&n.line).contains(&n_marker), "{:?}", named(j));
        assert!(
            j.versions
                .iter()
                .filter(|v| v.name != "N")
                .all(|v| !xs(&v.line).contains(&n_marker)),
            "{:?}",
            named(j)
        );
    }
    // The same lines either way. Which entry holds the line a and b
    // both took W off (W, back from c, or one the join makes) is the
    // tombstones' to say (step 7).
    let lines = |s: &Sidecar| {
        named(s)
            .into_iter()
            .map(|(_, x)| x)
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(lines(&abc), lines(&bca));
}

/// A copy swaps V, swaps W, and deletes W; the other deletes W. The line
/// left over, whose id W's entry once held, is kept as a version under
/// an id made from the line's: the same join gives the same file every
/// time and in both orders (MEDIUM-B).
#[test]
fn a_line_kept_as_a_version_gets_the_same_id_every_time() {
    let text = base().to_json();
    let a = make(&text, &[0, 1, 3], 1000, 0, "a", 200);
    let b = make(&text, &[3], 2000, 5, "b", 100);
    let first = sync::join(&a.s, &b.s);
    let made: Vec<&str> = first
        .versions
        .iter()
        .filter(|v| a.s.version(&v.id).is_none() && b.s.version(&v.id).is_none())
        .map(|v| v.id.as_str())
        .collect();
    assert!(
        !made.is_empty(),
        "a line kept as a version: {:?}",
        proj(&first)
    );
    for _ in 0..4 {
        assert_eq!(proj(&sync::join(&a.s, &b.s)), proj(&first));
        assert_eq!(proj(&sync::join(&b.s, &a.s)), proj(&first));
    }
}

/// Three machines, each joining the next one's copy in turn for eight
/// rounds, from 600 random starts of up to two moves each: at the end
/// all three hold the same versions on the same lines (LOW-C).
#[test]
fn three_machines_joining_in_turn_agree() {
    let text = base().to_json();
    let all = seqs(2, 12);
    let mut rng = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let held = |s: &Sidecar| -> BTreeSet<(String, String)> {
        s.versions
            .iter()
            .map(|v| (v.id.clone(), v.line.line_id.clone()))
            .collect()
    };
    let mut disagree = vec![];
    for _ in 0..600 {
        let mut pick = || all[(next() % all.len() as u64) as usize].clone();
        let (oa, ob, oc) = (pick(), pick(), pick());
        let mut a = make(&text, &oa, 1000, 0, "a", 100).s;
        let mut b = make(&text, &ob, 2000, 5, "b", 101).s;
        let mut c = make(&text, &oc, 3000, 7, "c", 102).s;
        let mut at = 200;
        for _ in 0..8 {
            at += 3;
            a = sync::join(&a, &b);
            stamp(&mut a, at, "a");
            b = sync::join(&b, &c);
            stamp(&mut b, at + 1, "b");
            c = sync::join(&c, &a);
            stamp(&mut c, at + 2, "c");
        }
        if held(&a) != held(&c) || held(&b) != held(&c) {
            disagree.push((oa, ob, oc, held(&a), held(&b), held(&c)));
        }
    }
    assert!(
        disagree.is_empty(),
        "{} disagree: {:#?}",
        disagree.len(),
        &disagree[..disagree.len().min(3)]
    );
}

/// What `join` asks of each copy, and the window asserts before it
/// joins: as its last revision left it, until it changes.
#[test]
fn a_copy_is_at_its_revision_until_it_changes() {
    let at = |at| sync::Stamp {
        at,
        host: "here".into(),
    };
    let mut s = base();
    assert!(s.at_its_revision(), "no revision yet");
    s.advance(&at(100));
    assert!(s.at_its_revision());
    s.record(exposed(9.0));
    assert!(!s.at_its_revision());
    s.advance(&at(101));
    assert!(s.at_its_revision());
    let mut back = Sidecar::from_json(&s.to_json(), &Edit::default()).unwrap();
    assert!(back.at_its_revision(), "read back");
    back.record(exposed(9.5));
    assert!(!back.at_its_revision(), "changed after the read");
    // A file whose last revision another build hashed, from another
    // form of it: as read, it is at its revision.
    let mut theirs = base();
    stamp(&mut theirs, 100, "there");
    let mut read = Sidecar::from_json(&theirs.to_json(), &Edit::default()).unwrap();
    assert!(read.at_its_revision(), "as read");
    read.meta.rating = 2;
    assert!(!read.at_its_revision(), "a rating after the read");
    // The XMP marks are no part of it: a batch save sets one after the
    // revision.
    let mut saved = base();
    saved.advance(&at(102));
    saved.xmp = Some(crate::xmp::Adopted {
        file: "frame.xmp".into(),
        len: 10,
        hash: "00".into(),
    });
    assert!(saved.at_its_revision());
}
