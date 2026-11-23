use super::*;
use crate::filter::{Filter, Saved, SavedBar};
use greycard_library::fixture::{A7, R5, R6, write_frame};

/// A folder no other test has used, made, and canonical: the index
/// keys folders by their canonical path, and on macOS the temporary
/// directory is a link.
fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-collections-{what}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dunce::canonicalize(&dir).unwrap()
}

fn r(hash: &str, hint: &str) -> Reference {
    Reference {
        hash: hash.into(),
        hint: hint.into(),
        version: None,
    }
}

/// Every folder answers, as the disk says.
fn answering(dirs: &[PathBuf]) -> std::collections::HashMap<PathBuf, Option<bool>> {
    dirs.iter().map(|d| (d.clone(), Some(d.is_dir()))).collect()
}

fn hashes(c: &Collection) -> Vec<&str> {
    c.refs.iter().map(|r| r.hash.as_str()).collect()
}

#[test]
fn a_name_typed_with_slashes_is_its_sets_and_its_name() {
    assert_eq!(
        split_name("Clients/2026/Cover picks"),
        ("Clients/2026".into(), "Cover picks".into())
    );
    assert_eq!(
        split_name(" Portfolio "),
        (String::new(), "Portfolio".into())
    );
    assert_eq!(split_name("/a//b/"), ("a".into(), "b".into()));
    assert_eq!(split_name(""), (String::new(), String::new()));
    assert_eq!(full_name("Clients/2026", "Cover"), "Clients/2026/Cover");
    assert_eq!(full_name("", "Cover"), "Cover");
    assert_ne!(Collection::new("a").id, Collection::new("a").id);
}

/// The files round trip, one a collection, in a folder of their own;
/// a file of a later build and one that will not read are left as
/// they were, and a delete takes the file with it.
#[test]
fn the_files_round_trip_and_leave_what_they_cannot_read_alone() {
    let dir = scratch("round").join(DIR);
    let mut store = Store::load(dir.clone());
    assert!(store.list.is_empty() && store.smart.is_empty());
    let mut c = Collection::new("Clients/2026/Cover picks");
    c.add(&[
        ("aa".into(), PathBuf::from("/shoot/a.CR3"), None),
        ("bb".into(), PathBuf::from("/shoot/b.CR3"), None),
    ]);
    let filter = Filter {
        stars: crate::filter::Stars::AtLeast(4),
        text: "camera:R6 iso>=3200".into(),
        ..Filter::default()
    };
    let s = Smart::new(
        "Best of",
        Saved::of(&filter),
        Some(PathBuf::from("/photos")),
    );
    let (cid, sid) = (c.id.clone(), s.id.clone());
    store.list.push(c.clone());
    store.smart.push(s.clone());
    store.save(&cid);
    store.save(&sid);
    let files: HashSet<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        files,
        HashSet::from([format!("{cid}.json"), format!("{sid}.smart.json")])
    );
    let text = std::fs::read_to_string(dir.join(format!("{cid}.json"))).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 1);
    assert_eq!(v["name"], "Cover picks");
    assert_eq!(v["set"], "Clients/2026");
    assert_eq!(v["refs"][1]["hash"], "bb");
    assert_eq!(v["refs"][1]["hint"], "/shoot/b.CR3");
    // A later build's, and one that is not JSON.
    std::fs::write(
        dir.join("later.json"),
        r#"{"version": 9, "name": "Later", "refs": []}"#,
    )
    .unwrap();
    std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
    let back = Store::load(dir.clone());
    // As the store holds them, each stamped with its first revision.
    assert_eq!(back.list, store.list);
    assert_eq!(back.smart, store.smart);
    assert_eq!(back.smart[0].filter.filter(), filter);
    assert_eq!(
        std::fs::read_to_string(dir.join("broken.json")).unwrap(),
        "{ not json"
    );
    let mut back = back;
    assert!(back.delete(&sid));
    assert!(!dir.join(format!("{sid}.smart.json")).exists());
    assert!(dir.join("later.json").exists());
    greycard_render::testing::remove_dir_retry(dir.parent().unwrap());
}

#[test]
fn an_add_appends_new_pictures_and_moves_the_hint_of_a_member() {
    let mut c = Collection::new("Picks");
    let added = c.add(&[
        ("aa".into(), PathBuf::from("/shoot/a.CR3"), None),
        ("bb".into(), PathBuf::from("/shoot/b.CR3"), None),
    ]);
    assert_eq!(added, Added { added: 2, moved: 0 });
    // The selects folder's copy of a: the same picture, no new
    // entry, and the hint is now the copy.
    let again = c.add(&[
        ("cc".into(), PathBuf::from("/shoot/c.CR3"), None),
        ("aa".into(), PathBuf::from("/selects/a.CR3"), None),
    ]);
    assert_eq!(again, Added { added: 1, moved: 1 });
    assert_eq!(hashes(&c), ["aa", "bb", "cc"]);
    assert_eq!(c.refs[0].hint, "/selects/a.CR3");
    assert_eq!(c.remove(&HashSet::from([("bb", None), ("zz", None)])), 1);
    assert_eq!(hashes(&c), ["aa", "cc"]);
}

#[test]
fn a_reorder_puts_the_moving_frames_before_the_one_dropped_on() {
    let mut c = Collection::new("Order");
    c.refs = ["a", "b", "c", "d", "e"].map(|h| r(h, h)).to_vec();
    assert!(c.reorder(&HashSet::from([("d", None)]), Some(("b", None))));
    assert_eq!(hashes(&c), ["a", "d", "b", "c", "e"]);
    // Two, keeping their own order, to the end.
    assert!(c.reorder(&HashSet::from([("a", None), ("b", None)]), None));
    assert_eq!(hashes(&c), ["d", "c", "e", "a", "b"]);
    // Dropped on one of themselves: before the next that is not.
    assert!(!c.reorder(
        &HashSet::from([("d", None), ("c", None)]),
        Some(("c", None))
    ));
    assert_eq!(hashes(&c), ["d", "c", "e", "a", "b"]);
    assert!(!c.reorder(&HashSet::from([("e", None)]), Some(("a", None))));
}

/// A pass that finds a file rewritten heals the reference whose hint
/// is that file, and one whose old hash is gone everywhere; one
/// whose old hash is still on another copy stays with it.
#[test]
fn a_file_rewritten_heals_the_references_by_their_hints() {
    let mut store = Store::default();
    let mut one = Collection::new("One");
    one.refs = vec![r("old", "/shoot/a.CR3"), r("x", "/shoot/x.CR3")];
    let mut two = Collection::new("Two");
    two.refs = vec![r("old", "/selects/a.CR3")];
    let mut three = Collection::new("Three");
    three.refs = vec![r("old", "/selects/a.CR3"), r("new", "/shoot/n.CR3")];
    store.list = vec![one.clone(), two.clone(), three.clone()];
    let rehash = Rehash {
        path: PathBuf::from("/shoot/a.CR3"),
        old: "old".into(),
        new: "new".into(),
    };
    // The selects copy still has the old hash: only the hinted heal.
    let changed = store.heal(std::slice::from_ref(&rehash), &|_| {
        Some(vec![PathBuf::from("/selects/a.CR3")])
    });
    assert_eq!(changed, [one.id.clone()]);
    assert_eq!(store.list[0].refs[0], r("new", "/shoot/a.CR3"));
    assert_eq!(store.list[1].refs, two.refs);
    // The old hash gone from the index: every reference heals; one
    // whose new hash another holds already stays as it was, missing,
    // rather than be dropped or show the other picture twice.
    let changed = store.heal(std::slice::from_ref(&rehash), &|h| {
        Some(if h == "old" {
            Vec::new()
        } else {
            vec![PathBuf::from("/x")]
        })
    });
    assert_eq!(changed, [two.id.clone()]);
    assert_eq!(store.list[1].refs, [r("new", "/shoot/a.CR3")]);
    assert_eq!(store.list[2].refs, three.refs);
    // With no index to ask, the copies unknown: nothing taken for
    // gone, and no hint written empty.
    let mut four = Collection::new("Four");
    four.refs = vec![r("old", "/selects/a.CR3"), r("new", "/shoot/n.CR3")];
    let mut five = Collection::new("Five");
    five.refs = vec![r("old", "/shoot/a.CR3"), r("new", "/shoot/n.CR3")];
    store.list = vec![four.clone(), five.clone()];
    let changed = store.heal(&[rehash], &|_| None);
    assert!(changed.is_empty(), "{changed:?}");
    assert_eq!(store.list[0].refs, four.refs);
    assert_eq!(store.list[1].refs, five.refs, "the hint kept, never empty");
}

/// The shoot's copy of a picture rewritten with another member's
/// content, while the selects' copy keeps the picture: the reference
/// is not dropped as a duplicate but follows the copy that survives;
/// and once that copy is gone too, an open does not take the file at
/// its hint for it (that file is the other member now), and keeps it
/// as missing.
#[test]
fn a_rewrite_into_another_members_picture_drops_no_reference() {
    let dir = scratch("into-member");
    let (shoot, selects) = (dir.join("shoot"), dir.join("selects"));
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::create_dir_all(&selects).unwrap();
    let (a, b, copy) = (
        shoot.join("a.tif"),
        shoot.join("b.tif"),
        selects.join("a.tif"),
    );
    write_frame(&a, &R5, 1);
    write_frame(&b, &R6, 2);
    std::fs::copy(&a, &copy).unwrap();
    write_frame(&selects.join("other.tif"), &A7, 3);
    let mut lib = Library::open_in_memory().unwrap();
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    let hash = |lib: &Library, p: &Path| lib.by_path(p).unwrap().unwrap().hash;
    let (h, hb) = (hash(&lib, &a), hash(&lib, &b));
    let mut store = Store::default();
    let mut c = Collection::new("Two");
    c.add(&[(h.clone(), a.clone(), None), (hb.clone(), b.clone(), None)]);
    store.list.push(c);
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::copy(&b, &a).unwrap();
    let report = lib.index_tree(&dir, &mut |_| {}).unwrap();
    assert_eq!(report.rehashed.len(), 1, "{report:?}");
    assert_eq!(
        (&report.rehashed[0].old, &report.rehashed[0].new),
        (&h, &hb)
    );
    let copies = |x: &str| -> Option<Vec<PathBuf>> {
        Some(
            lib.rows_by_hash(x)
                .unwrap()
                .into_iter()
                .map(|r| r.path)
                .collect(),
        )
    };
    store.heal(&report.rehashed, &copies);
    assert_eq!(
        store.list[0].refs,
        [r(&h, &hint_of(&copy)), r(&hb, &hint_of(&b))],
        "kept, and pointed at the copy that still is the picture"
    );
    let shown = resolve(&lib, &store.list[0].refs, &[], &[], None).unwrap();
    let paths: Vec<&PathBuf> = shown.listed.iter().map(|(p, _)| p).collect();
    assert_eq!(paths, [&copy, &b]);
    // The selects' copy gone too, the reference hinted at the shoot's
    // file, which holds b's picture now: missing, not adopted.
    std::fs::remove_file(&copy).unwrap();
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    let refs = [r(&h, &hint_of(&a)), r(&hb, &hint_of(&b))];
    let shown = resolve(&lib, &refs, &[], &[], None).unwrap();
    assert_eq!(shown.missing, 1);
    assert_eq!(shown.listed.len(), 1);
    assert!(shown.changes.is_empty(), "{:?}", shown.changes);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// Both copies of a picture rewritten in one pass, each to a hash of
/// its own: a reference heals by the rewrite at its own hint, not by
/// whichever the pass reported first.
#[test]
fn both_copies_rewritten_in_one_pass_heal_each_by_its_own_path() {
    let mut store = Store::default();
    let mut c = Collection::new("Both");
    c.refs = vec![r("h", "/selects/a.CR3")];
    store.list.push(c);
    let rehashed = [
        Rehash {
            path: PathBuf::from("/shoot/a.CR3"),
            old: "h".into(),
            new: "s1".into(),
        },
        Rehash {
            path: PathBuf::from("/selects/a.CR3"),
            old: "h".into(),
            new: "s2".into(),
        },
    ];
    store.heal(&rehashed, &|_| Some(Vec::new()));
    assert_eq!(store.list[0].refs, [r("s2", "/selects/a.CR3")]);
    // Hinted somewhere neither rewrite was, with the old hash gone:
    // the first rewrite of it, and the hint goes with it.
    let mut c = Collection::new("Elsewhere");
    c.refs = vec![r("h", "/moved/a.CR3")];
    store.list = vec![c];
    store.heal(&rehashed, &|_| Some(Vec::new()));
    assert_eq!(store.list[0].refs, [r("s1", "/shoot/a.CR3")]);
    // No member's hash among the rewrites: nothing asked of the index.
    let asked = std::cell::Cell::new(0);
    let other = [Rehash {
        path: PathBuf::from("/x.CR3"),
        old: "zz".into(),
        new: "yy".into(),
    }];
    let changed = store.heal(&other, &|_| {
        asked.set(asked.get() + 1);
        Some(Vec::new())
    });
    assert!(changed.is_empty());
    assert_eq!(asked.get(), 0);
}

/// A collection's file written by someone else since this store read
/// it (a second editor, a hand edit) is read and joined before ours
/// goes over it: neither side's additions are lost.
#[test]
fn a_file_changed_on_disk_is_joined_before_it_is_written() {
    let dir = scratch("joined").join(DIR);
    let mut first = Store::load(dir.clone());
    let mut c = Collection::new("Shared");
    c.add(&[("aa".into(), PathBuf::from("/s/a.CR3"), None)]);
    let id = c.id.clone();
    first.list.push(c);
    first.save(&id);
    // A second editor reads it, adds b and writes.
    let mut second = Store::load(dir.clone());
    second
        .collection_mut(&id)
        .unwrap()
        .add(&[("bb".into(), PathBuf::from("/s/b.CR3"), None)]);
    second.save(&id);
    // The first adds c and writes: b is joined, not lost.
    first
        .collection_mut(&id)
        .unwrap()
        .add(&[("cc".into(), PathBuf::from("/s/c.CR3"), None)]);
    first.save(&id);
    let back = Store::load(dir.clone());
    assert_eq!(hashes(back.collection(&id).unwrap()), ["aa", "cc", "bb"]);
    // Its own writes since are not taken for someone else's.
    first
        .collection_mut(&id)
        .unwrap()
        .remove(&HashSet::from([("cc", None)]));
    first.save(&id);
    let back = Store::load(dir.clone());
    assert_eq!(hashes(back.collection(&id).unwrap()), ["aa", "bb"]);
    assert!(back.id_taken(&id));
    // Our own removal, then only the file's time moved (a sync tool
    // touching it, the content the same): it stays removed.
    first
        .collection_mut(&id)
        .unwrap()
        .remove(&HashSet::from([("bb", None)]));
    let file = dir.join(format!("{id}.json"));
    let same = std::fs::read(&file).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&file, &same).unwrap();
    first.save(&id);
    let back = Store::load(dir.clone());
    assert_eq!(hashes(back.collection(&id).unwrap()), ["aa"]);
    // A removal the other side made is taken from ours too.
    let mut second = Store::load(dir.clone());
    first
        .collection_mut(&id)
        .unwrap()
        .add(&[("dd".into(), PathBuf::from("/s/d.CR3"), None)]);
    second
        .collection_mut(&id)
        .unwrap()
        .remove(&HashSet::from([("aa", None)]));
    second.save(&id);
    first.save(&id);
    let back = Store::load(dir.clone());
    assert_eq!(hashes(back.collection(&id).unwrap()), ["dd"]);
    greycard_render::testing::remove_dir_retry(dir.parent().unwrap());
}
/// A collection's file that will not read any more (a hand edit
/// gone wrong since it was read) is kept beside itself as
/// `.json.bad`, with what it held, before ours is written over it.
/// An outside edit that keeps the file's length inside one mtime
/// tick (a hint moved to a path of the same length) is still a
/// change, since the stamp is the content's hash.
#[test]
fn a_same_length_edit_in_the_same_tick_is_still_joined() {
    let dir = scratch("same-length").join(DIR);
    let mut store = Store::load(dir.clone());
    let mut c = Collection::new("Shared");
    c.add(&[("aa".into(), PathBuf::from("/s/a.CR3"), None)]);
    let id = c.id.clone();
    store.list.push(c);
    store.save(&id);
    // The other side swaps a for b, same length, no time passing.
    let file = dir.join(format!("{id}.json"));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"aa\""));
    std::fs::write(&file, text.replace("\"aa\"", "\"bb\"")).unwrap();
    store
        .collection_mut(&id)
        .unwrap()
        .add(&[("cc".into(), PathBuf::from("/s/c.CR3"), None)]);
    store.save(&id);
    // Their removal of a and addition of b are both taken.
    let back = Store::load(dir.clone());
    assert_eq!(hashes(back.collection(&id).unwrap()), ["cc", "bb"]);
    greycard_render::testing::remove_dir_retry(dir.parent().unwrap());
}
#[test]
fn a_file_broken_on_disk_is_kept_aside_before_it_is_written() {
    let dir = scratch("broken").join(DIR);
    let mut store = Store::load(dir.clone());
    let mut c = Collection::new("Shared");
    c.add(&[("aa".into(), PathBuf::from("/s/a.CR3"), None)]);
    let id = c.id.clone();
    store.list.push(c);
    store.save(&id);
    let file = dir.join(format!("{id}.json"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&file, "{ not json").unwrap();
    store
        .collection_mut(&id)
        .unwrap()
        .add(&[("bb".into(), PathBuf::from("/s/b.CR3"), None)]);
    store.save(&id);
    let aside = dir.join(format!("{id}.json.bad"));
    assert_eq!(std::fs::read_to_string(&aside).unwrap(), "{ not json");
    let back = Store::load(dir.clone());
    assert_eq!(hashes(back.collection(&id).unwrap()), ["aa", "bb"]);
    greycard_render::testing::remove_dir_retry(dir.parent().unwrap());
}
/// Two copies of a frame and three references, one of them to a
/// hash nothing has: opened, each picture once, from the hint while
/// it holds the hash, the missing one counted and kept; a file
/// rewritten while nobody looked is found by its hint and healed;
/// capture order sorts by the time taken.
#[test]
fn a_collection_opened_shows_each_picture_once_and_keeps_the_missing() {
    let dir = scratch("resolve");
    let (shoot, selects) = (dir.join("shoot"), dir.join("selects"));
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::create_dir_all(&selects).unwrap();
    let a = shoot.join("a.tif");
    let b = shoot.join("b.tif");
    write_frame(&a, &R5, 1);
    write_frame(&b, &R6, 2);
    std::fs::copy(&a, selects.join("a.tif")).unwrap();
    // Not alone in its folder: an emptied folder is taken for a
    // drive not there, and its rows are left as they were.
    write_frame(&selects.join("other.tif"), &A7, 9);
    let mut lib = Library::open_in_memory().unwrap();
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    let hash = |lib: &Library, p: &Path| lib.by_path(p).unwrap().unwrap().hash;
    let (ha, hb) = (hash(&lib, &a), hash(&lib, &b));
    assert_eq!(ha, hash(&lib, &selects.join("a.tif")));
    let mut c = Collection::new("Mixed");
    c.add(&[
        (hb.clone(), b.clone(), None),
        (ha.clone(), selects.join("a.tif"), None),
        ("feedface".into(), shoot.join("gone.tif"), None),
    ]);
    let shown = resolve(&lib, &c.refs, &[], &[], None).unwrap();
    let paths: Vec<&PathBuf> = shown.listed.iter().map(|(p, _)| p).collect();
    assert_eq!(paths, [&b, &selects.join("a.tif")]);
    assert_eq!(shown.missing, 1);
    assert!(shown.changes.is_empty());
    // The hinted copy's root away: the other copy, hint kept.
    let shown = resolve(&lib, &c.refs, std::slice::from_ref(&selects), &[], None).unwrap();
    assert_eq!(shown.listed[1].0, a);
    assert!(shown.changes.is_empty());
    // The selects copy deleted and its folder passed: the shoot's,
    // and the hint follows it.
    std::fs::remove_file(selects.join("a.tif")).unwrap();
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    let shown = resolve(&lib, &c.refs, &[], &[], None).unwrap();
    assert_eq!(shown.listed[1].0, a);
    assert_eq!(
        shown.changes,
        [Change {
            was: ha.clone(),
            version: None,
            hash: ha.clone(),
            hint: hint_of(&a)
        }]
    );
    let mut store = Store::default();
    store.list.push(c.clone());
    assert!(store.apply(&c.id, &shown.changes));
    // b rewritten by another tool, and the index told before the
    // collection heard: found by its hint at the open.
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_frame(&b, &A7, 3);
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    let hb2 = hash(&lib, &b);
    let refs = store.list[0].refs.clone();
    let shown = resolve(&lib, &refs, &[], &[], None).unwrap();
    assert_eq!(shown.listed[0].0, b);
    assert_eq!(
        shown.changes,
        [Change {
            was: hb.clone(),
            version: None,
            hash: hb2.clone(),
            hint: hint_of(&b)
        }]
    );
    assert_eq!(shown.missing, 1, "the missing one is still kept");
    assert!(store.apply(&c.id, &shown.changes));
    assert_eq!(store.list[0].refs.len(), 3);
    assert_eq!(store.list[0].refs[0].hash, hb2);
    // Capture order: the test frames' dates follow their seeds.
    let shown = resolve(
        &lib,
        &store.list[0].refs,
        &[],
        &[],
        Some(greycard_library::Order::TAKEN),
    )
    .unwrap();
    let taken: Vec<Option<String>> = shown
        .listed
        .iter()
        .map(|(p, _)| lib.by_path(p).unwrap().unwrap().exif.taken)
        .collect();
    let mut sorted = taken.clone();
    sorted.sort_by(|x, y| match (x, y) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    assert_eq!(taken, sorted);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// Forget missing drops the references the index has nowhere, by the
/// status line's own count, and writes the file; a frame under an
/// offline root is kept, whether the index still holds its row or
/// last saw it missing, and with no index to ask nothing changes.
#[test]
fn forget_missing_drops_the_gone_and_keeps_the_offline() {
    let dir = scratch("forget");
    let (shoot, drive) = (dir.join("shoot"), dir.join("drive"));
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::create_dir_all(&drive).unwrap();
    let (a, b) = (shoot.join("a.tif"), shoot.join("b.tif"));
    let (on_drive, lost) = (drive.join("c.tif"), drive.join("d.tif"));
    write_frame(&a, &R5, 1);
    write_frame(&b, &R6, 2);
    write_frame(&on_drive, &A7, 3);
    write_frame(&lost, &A7, 4);
    let mut lib = Library::open_in_memory().unwrap();
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    let hash = |lib: &Library, p: &Path| lib.by_path(p).unwrap().unwrap().hash;
    let (ha, hb, hc, hd) = (
        hash(&lib, &a),
        hash(&lib, &b),
        hash(&lib, &on_drive),
        hash(&lib, &lost),
    );
    // b deleted and d deleted, each seen missing by a pass; then the
    // drive unplugged, its rows left as the index has them.
    std::fs::remove_file(&b).unwrap();
    std::fs::remove_file(&lost).unwrap();
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    assert!(lib.by_path(&lost).unwrap().unwrap().missing);
    let mut c = Collection::new("Picks");
    c.add(&[
        (ha.clone(), a.clone(), None),
        (hb.clone(), b.clone(), None),
        (hc.clone(), on_drive.clone(), None),
        (hd.clone(), lost.clone(), None),
        ("feedface".into(), shoot.join("never.tif"), None),
    ]);
    let id = c.id.clone();
    let mut store = Store {
        dir: Some(dir.join(DIR)),
        ..Store::default()
    };
    store.list.push(c);
    store.save(&id);
    let away = std::slice::from_ref(&drive);
    let look = answering;
    let before = resolve(&lib, &store.list[0].refs, away, &[], None).unwrap();
    assert_eq!(before.missing, 3, "b, d and the never-seen one");
    let gone: Vec<&str> = before.gone.iter().map(|r| r.hash.as_str()).collect();
    assert_eq!(gone, [hb.as_str(), "feedface"]);
    // No index to ask: nothing known gone, nothing changed.
    assert_eq!(store.forget_missing(&id, None, away, &[], &look), None);
    assert_eq!(store.list[0].refs.len(), 5);
    let forgot = store
        .forget_missing(&id, Some(&lib), away, &[], &look)
        .unwrap();
    assert_eq!(
        forgot,
        Forgot {
            forgotten: 2,
            missing: 1,
            away: 1,
            on_disk: 0,
            unsure: 0,
        }
    );
    assert_eq!(store.missing, Some((id.clone(), 1, 0)));
    assert_eq!(hashes(&store.list[0]), [&ha, &hc, &hd]);
    // The file on disk says so.
    let file = read_collection(&dir.join(DIR).join(format!("{id}.json"))).unwrap();
    let on_disk: Vec<&str> = file.refs.iter().map(|r| r.hash.as_str()).collect();
    assert_eq!(on_disk, [&ha, &hc, &hd]);
    // The status line's count drops by them; the offline one stays.
    let after = resolve(&lib, &store.list[0].refs, away, &[], None).unwrap();
    assert_eq!(after.missing, 1);
    assert!(after.gone.is_empty());
    let forgot = store
        .forget_missing(&id, Some(&lib), away, &[], &look)
        .unwrap();
    assert_eq!((forgot.forgotten, forgot.away), (0, 1));
    // The drive back: d is gone by the same test, and goes.
    let forgot = store
        .forget_missing(&id, Some(&lib), &[], &[], &look)
        .unwrap();
    assert_eq!((forgot.forgotten, forgot.missing), (1, 0));
    assert_eq!(hashes(&store.list[0]), [&ha, &hc]);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A reference whose hint is a file on disk the index has not read
/// (a pass still to reach it, an index rebuilt) counts missing and
/// is never forgotten; one under a network mount is kept without
/// its folder being looked at, and one in a folder that does not
/// answer is kept.
#[test]
fn forget_missing_keeps_a_file_on_disk_and_never_looks_at_a_far_one() {
    let dir = scratch("forget-on-disk");
    let a = dir.join("a.tif");
    write_frame(&a, &R5, 1);
    let (nas, quiet) = (dir.join("nas"), dir.join("quiet"));
    // An index that has read nothing: as after a rebuild.
    let lib = Library::open_in_memory().unwrap();
    let mut c = Collection::new("Unread");
    c.add(&[
        ("0123456789abcdef".into(), a.clone(), None),
        ("feedface".into(), dir.join("gone.tif"), None),
        ("cafebabe".into(), nas.join("b.tif"), None),
        ("deadbeef".into(), quiet.join("c.tif"), None),
    ]);
    let id = c.id.clone();
    let mut store = Store::default();
    store.list.push(c);
    let shown = resolve(&lib, &store.list[0].refs, &[], &[], None).unwrap();
    assert_eq!((shown.missing, shown.gone.len()), (4, 4));
    let looked = std::cell::RefCell::new(Vec::new());
    let look = |dirs: &[PathBuf]| {
        looked.borrow_mut().extend(dirs.iter().cloned());
        dirs.iter()
            .map(|d| (d.clone(), (!d.starts_with(&quiet)).then(|| d.is_dir())))
            .collect()
    };
    let far = std::slice::from_ref(&nas);
    let forgot = store
        .forget_missing(&id, Some(&lib), &[], far, &look)
        .unwrap();
    assert_eq!(
        forgot,
        Forgot {
            forgotten: 1,
            missing: 3,
            away: 2,
            on_disk: 1,
            unsure: 0,
        }
    );
    assert_eq!(
        hashes(&store.list[0]),
        ["0123456789abcdef", "cafebabe", "deadbeef"]
    );
    assert!(
        !looked.borrow().iter().any(|d| d.starts_with(&nas)),
        "the network mount is never looked at: {:?}",
        looked.borrow()
    );
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A folder that cannot be read answers the look with no, and its
/// files' stat fails: not "not found", so they are kept. As after a
/// rebuilt index, whose pass left the folder with no rows.
#[cfg(unix)]
#[test]
fn forget_missing_keeps_the_frames_of_a_folder_that_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("forget-locked");
    let locked = dir.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    let a = locked.join("a.tif");
    write_frame(&a, &R5, 1);
    // The mode put back however the test ends: a folder of mode 000
    // left under target/ stops a `cargo clean`.
    struct Restore<'a>(&'a Path);
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
        }
    }
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let restore = Restore(&locked);
    // Run as root, the mode stops nothing, and there is nothing to test.
    let readable = std::fs::read_dir(&locked).is_ok();
    let lib = Library::open_in_memory().unwrap();
    let mut c = Collection::new("Locked");
    c.add(&[
        ("0123456789abcdef".into(), a.clone(), None),
        ("feedface".into(), dir.join("gone").join("b.tif"), None),
    ]);
    let id = c.id.clone();
    let mut store = Store::default();
    store.list.push(c);
    let forgot = store
        .forget_missing(&id, Some(&lib), &[], &[], &answering)
        .unwrap();
    drop(restore);
    if !readable {
        assert_eq!(
            forgot,
            Forgot {
                forgotten: 1,
                missing: 1,
                away: 1,
                on_disk: 0,
                unsure: 0,
            }
        );
        assert_eq!(hashes(&store.list[0]), ["0123456789abcdef"]);
    }
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A version of `path`'s picture named `name`, made in its sidecar
/// and written: its id.
fn version_in(path: &Path, name: &str) -> String {
    let mut s = greycard_edit::Sidecar::load_as_file(path)
        .unwrap()
        .unwrap_or_default();
    let id = s.new_version(None, Some(name)).unwrap();
    s.save(path).unwrap();
    id
}

/// A reference with a version keeps it through the file; the file
/// says shape 2 then, and 1 again once no reference has one, the
/// file's own frame writing no version at all. A build that reads
/// only shape 1 leaves a file at 2 alone, on disk as it was.
#[test]
fn a_reference_with_a_version_round_trips_and_raises_the_shape() {
    let dir = scratch("version-file").join(DIR);
    let mut store = Store::load(dir.clone());
    let mut c = Collection::new("Warm ones");
    c.add(&[
        ("aa".into(), PathBuf::from("/shoot/a.CR3"), None),
        (
            "aa".into(),
            PathBuf::from("/shoot/a.CR3"),
            Some("0123456789abcdef".into()),
        ),
    ]);
    let id = c.id.clone();
    store.list.push(c.clone());
    store.save(&id);
    let file = dir.join(format!("{id}.json"));
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["version"], 2);
    assert!(
        v["refs"][0].get("version").is_none(),
        "the file's own: no key"
    );
    assert_eq!(v["refs"][1]["version"], "0123456789abcdef");
    assert_eq!(Store::load(dir.clone()).list, store.list);
    // An older build: reads up to 1, and leaves the file alone.
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(Store::load_reading(dir.clone(), 1).list.is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    // The version taken out: the file is a plain one again.
    let out = store
        .collection_mut(&id)
        .unwrap()
        .remove(&HashSet::from([("aa", Some("0123456789abcdef"))]));
    assert_eq!(out, 1);
    store.save(&id);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["version"], 1);
    assert_eq!(v["refs"].as_array().unwrap().len(), 1);
    assert_eq!(Store::load_reading(dir.clone(), 1).list.len(), 1);
    greycard_render::testing::remove_dir_retry(dir.parent().unwrap());
}

/// A smart collection holding a chip out is written at shape 3,
/// which a build that knows no held-out chips leaves alone rather
/// than list what they hide; one without is written at 1.
#[test]
fn a_smart_collection_holding_a_chip_out_raises_the_shape() {
    let dir = scratch("held-out-file").join(DIR);
    let mut store = Store::load(dir.clone());
    let mut filter = Filter::default();
    filter.exclude_flag(greycard_edit::meta::Flag::Reject);
    let s = Smart::new("No rejects", Saved::of(&filter), None);
    let id = s.id.clone();
    store.smart.push(s.clone());
    store.save(&id);
    let file = dir.join(format!("{id}.smart.json"));
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["version"], 3);
    assert_eq!(Store::load(dir.clone()).smart, store.smart);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(Store::load_reading(dir.clone(), 2).smart.is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    // Let back in: a plain one again.
    store.smart[0].filter = Saved::default();
    store.save(&id);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["version"], 1);
    assert_eq!(Store::load_reading(dir.clone(), 1).smart.len(), 1);
    greycard_render::testing::remove_dir_retry(dir.parent().unwrap());
}

/// Two versions of a picture are two frames: each added once, each
/// its own hint, each taken out alone; the file's own is a frame
/// apart from them.
#[test]
fn two_versions_of_a_picture_are_two_frames() {
    let mut c = Collection::new("Pairs");
    let warm = Some("1111111111111111".to_string());
    let added = c.add(&[
        ("aa".into(), PathBuf::from("/shoot/a.CR3"), warm.clone()),
        ("aa".into(), PathBuf::from("/shoot/a.CR3"), None),
        ("aa".into(), PathBuf::from("/shoot/a.CR3"), warm.clone()),
    ]);
    assert_eq!(added, Added { added: 2, moved: 0 });
    assert!(c.has(("aa", warm.as_deref())) && c.has(("aa", None)));
    assert!(!c.has(("aa", Some("2222222222222222"))));
    // The selects' copy of the file's own: its hint alone moves.
    let again = c.add(&[("aa".into(), PathBuf::from("/selects/a.CR3"), None)]);
    assert_eq!(again, Added { added: 0, moved: 1 });
    assert_eq!(c.refs[0].hint, "/shoot/a.CR3");
    assert_eq!(c.refs[1].hint, "/selects/a.CR3");
    // Reordered by the pair: the file's own before the version.
    assert!(c.reorder(
        &HashSet::from([("aa", None)]),
        Some(("aa", warm.as_deref()))
    ));
    assert_eq!(c.refs[0].version, None);
    // A version made beside the frame it came from.
    let cool = (
        "aa".to_string(),
        PathBuf::from("/shoot/a.CR3"),
        Some("3333333333333333".to_string()),
    );
    assert!(c.insert_after(("aa", None), &cool));
    assert!(!c.insert_after(("aa", None), &cool), "once");
    assert_eq!(c.refs[1].version.as_deref(), Some("3333333333333333"));
    assert_eq!(c.remove(&HashSet::from([("aa", warm.as_deref())])), 1);
    assert_eq!(c.refs.len(), 2);
    assert!(c.has(("aa", None)));
    // A frame named twice in the file (two editors' joined files)
    // counts once, held and taken out.
    let twice = c.refs[0].clone();
    c.refs.push(twice);
    assert_eq!(c.count(), 2);
    assert_eq!(c.remove(&HashSet::from([("aa", None)])), 1);
    assert_eq!(c.count(), 1);
}

/// A collection's read finds a version in the sidecar of the copy it
/// was made on, as the index read it: shown from that file even when
/// hinted at another copy, each (hash, version) once, the file listed
/// once for its two frames. A version the sidecar does not have is
/// missing, counted and kept, with the file it was looked for in; one
/// under a root that is away is counted as away.
#[test]
fn a_version_is_found_by_its_id_and_a_gone_one_is_missing() {
    let dir = scratch("resolve-versions");
    let (shoot, selects) = (dir.join("shoot"), dir.join("selects"));
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::create_dir_all(&selects).unwrap();
    let a = shoot.join("a.tif");
    write_frame(&a, &R5, 1);
    write_frame(&shoot.join("b.tif"), &R6, 2);
    std::fs::copy(&a, selects.join("a.tif")).unwrap();
    write_frame(&selects.join("other.tif"), &A7, 9);
    let warm = version_in(&a, "Warm");
    let mut lib = Library::open_in_memory().unwrap();
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    let ha = lib.by_path(&a).unwrap().unwrap().hash;
    let mut c = Collection::new("Versions");
    c.add(&[
        // Hinted at the selects' copy, whose sidecar has no Warm.
        (ha.clone(), selects.join("a.tif"), Some(warm.clone())),
        (ha.clone(), a.clone(), None),
        (ha.clone(), a.clone(), Some("ffffffffffffffff".into())),
    ]);
    // A second reference to Warm, as a join of two files could make.
    let mut refs = c.refs.clone();
    refs.push(refs[0].clone());
    let shown = resolve(&lib, &refs, &[], &[], None).unwrap();
    // The gone one among them at its file, for the window to show
    // while the sidecar it holds has it.
    assert_eq!(
        shown.frames,
        [
            (a.clone(), Some(warm.clone())),
            (a.clone(), None),
            (a.clone(), Some("ffffffffffffffff".into()))
        ]
    );
    let listed: Vec<&PathBuf> = shown.listed.iter().map(|(p, _)| p).collect();
    assert_eq!(listed, [&a], "the file once for its two frames");
    assert_eq!(shown.missing, 1);
    assert!(shown.gone.is_empty());
    assert_eq!(shown.lost.len(), 1);
    assert_eq!(shown.lost[0].0.version.as_deref(), Some("ffffffffffffffff"));
    assert_eq!(shown.lost[0].1, a);
    // Warm's hint follows to the copy that has it.
    assert_eq!(
        shown.changes,
        [Change {
            was: ha.clone(),
            version: Some(warm.clone()),
            hash: ha.clone(),
            hint: hint_of(&a),
        }]
    );
    // The shoot's root away: the version gone there is away, kept.
    let shown = resolve(&lib, &refs, std::slice::from_ref(&shoot), &[], None).unwrap();
    assert_eq!((shown.missing, shown.away), (1, 1));
    assert!(shown.lost.is_empty());
    greycard_render::testing::remove_dir_retry(&dir);
}

/// Forget missing drops a version whose file is there only when its
/// sidecar reads and lacks the id. One the sidecar has (the index
/// behind it) is kept as on disk, and one whose sidecar will not
/// read cannot tell and is kept.
#[test]
fn forget_missing_drops_a_version_only_when_its_sidecar_reads_without_it() {
    let dir = scratch("forget-versions");
    let (a, b, d) = (dir.join("a.tif"), dir.join("b.tif"), dir.join("d.tif"));
    write_frame(&a, &R5, 1);
    write_frame(&b, &R6, 2);
    write_frame(&d, &A7, 3);
    for p in [&a, &b, &d] {
        version_in(p, "Kept");
    }
    let mut lib = Library::open_in_memory().unwrap();
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    let hash = |p: &Path| lib.by_path(p).unwrap().unwrap().hash;
    // a's version deleted (gone from a sidecar that reads); b's
    // made after the pass (in the sidecar, not yet in the index);
    // d's sidecar unreadable.
    let gone_id = "aaaaaaaaaaaaaaaa".to_string();
    let late = version_in(&b, "Late");
    let mut c = Collection::new("Forget versions");
    c.add(&[
        (hash(&a), a.clone(), Some(gone_id)),
        (hash(&b), b.clone(), Some(late)),
        (hash(&d), d.clone(), Some("dddddddddddddddd".into())),
    ]);
    let gcd = greycard_edit::Sidecar::find(&d).unwrap();
    std::fs::write(&gcd, b"{ not json").unwrap();
    let id = c.id.clone();
    let mut store = Store::default();
    store.list.push(c);
    let shown = resolve(&lib, &store.list[0].refs, &[], &[], None).unwrap();
    assert_eq!((shown.missing, shown.lost.len()), (3, 3));
    let forgot = store
        .forget_missing(&id, Some(&lib), &[], &[], &answering)
        .unwrap();
    assert_eq!(
        forgot,
        Forgot {
            forgotten: 1,
            missing: 2,
            away: 0,
            on_disk: 1,
            unsure: 1,
        }
    );
    let left: Vec<PathBuf> = store.list[0]
        .refs
        .iter()
        .map(|r| PathBuf::from(&r.hint))
        .collect();
    assert_eq!(left, [b, d]);
    greycard_render::testing::remove_dir_retry(&dir);
}
