//! Collections and smart collections: frames gathered by hand from
//! anywhere in the library, and the filter bar's question kept under a
//! name.
//!
//! Neither can be rebuilt from the disk, so neither lives in the index,
//! which can be deleted and rebuilt at any time. They live in a folder
//! of their own beside it, `collections/` under the library's data
//! directory, one small file a collection: `<id>.json` for one filled
//! by hand, `<id>.smart.json` for a saved filter. The id is made at
//! random when the collection is, so a rename never moves the file and
//! two machines never make the same one; deleting a collection deletes
//! its file. Each is written through a part file renamed over it, as
//! the roots file is.
//!
//! A collection holds pictures, not paths. A reference is the frame's
//! content hash, the index's own (the first 64 KB and the size), with
//! the path it was last seen at as a hint. Every local frame of a hash
//! is a member, since it is the same picture; opened, the collection
//! shows each hash once, from the hinted path when the index has a
//! file there, else from the first local frame of the hash it knows.
//! Adding another copy of a member adds nothing and moves the hint.
//! A file rewritten in place by another tool has a new hash at the
//! same path: the reference takes the new hash, by its hint, when the
//! index says so ([`Store::heal`]) or when the collection is opened and
//! the old hash is nowhere ([`resolve`]). A reference with nothing at
//! its hash or its hint is kept, and counted as missing, never dropped.
//!
//! A reference may name a version of the picture by its id in the
//! sidecar; none is the file's own. Two versions of one picture are two
//! frames of the collection, each shown once, and a version the sidecar
//! no longer has is missing, kept as a moved file is: a join with
//! another machine's copy may bring it back. A file with a version in
//! it is written at shape 2, which an older build leaves alone; one
//! without stays at 1.
//!
//! A collection's list is its order: a frame added goes at the end,
//! and a drag in the grid moves it. Capture order is a view of the same
//! list, sorted by the time taken, and rewrites nothing. A smart
//! collection has no list; it is the filter bar's state and the root
//! it looks under, and it sorts as the grid does.
//!
//! Nothing here touches the window: the window's side is
//! `panel::collections`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use greycard_library::carry::Item;
use greycard_library::collections::stamp_of;
use greycard_library::{Library, Rehash, RowMeta};

// The files and their writer are the library's, so the command line's
// Lightroom import writes them as the editor does. `Added` and `DIR`
// only the tests name, the binary's among them, which build this
// library without cfg(test).
pub use greycard_library::collections::{
    Added, Collection, DIR, Frame, Pair, Reference, Smart, Stamp, VERSION, file_of, full_name,
    hint_of, read_collection, split_name,
};

/// The collections as the editor holds them: the folder they are kept
/// in, each one, and what the window marks with them.
#[derive(Default)]
pub struct Store {
    /// None for a session that keeps nothing (a test's, unless it gives
    /// one), or with no data directory.
    pub dir: Option<PathBuf>,
    pub list: Vec<Collection>,
    pub smart: Vec<Smart>,
    /// The collection B adds to outside a collection's own view: the
    /// last one added to from the menu or made.
    pub target: Option<String>,
    /// The most recent destinations for Add to collection.
    pub recent: Vec<String>,
    /// The index's rows whose hash is a member of any collection, each
    /// with the versions of it that are: a cell is marked when its own
    /// version is among its row's, so a version in a collection marks
    /// that version's cell and not the original's.
    pub member_ids: std::collections::HashMap<i64, HashSet<Option<String>>>,
    /// The last read of a collection: its id, how many of its
    /// references were missing, and how many of those Forget missing
    /// may drop (the rest hinted under an offline root).
    pub missing: Option<(String, usize, usize)>,
    /// Each file's content hash as it was when read or last written
    /// here: a file whose bytes differ was written since by another
    /// editor or by hand, and is read and joined before ours goes over
    /// it.
    stamps: std::collections::HashMap<String, Stamp>,
    /// Each collection's frames, (hash, version), as its file held them
    /// when read or last written here: what a file changed since is
    /// joined against, so only the other side's own additions and
    /// removals are taken.
    bases: std::collections::HashMap<String, HashSet<(String, Option<String>)>>,
    /// The writer of the copies carried beside the frames; none for a
    /// session that carries nothing.
    pub carrier: Option<std::rc::Rc<crate::carried::Carrier>>,
}

impl Store {
    /// The folder for a library at `library`: beside it.
    pub fn dir_beside(library: &Path) -> PathBuf {
        greycard_library::collections::dir_beside(library)
    }

    /// Every collection in `dir`. A file that will not read, or one a
    /// later build wrote, is said and left where it is: nothing here
    /// writes to a file it did not read.
    pub fn load(dir: PathBuf) -> Store {
        Store::load_reading(dir, VERSION)
    }

    /// [`Store::load`] by a build that reads files up to shape `up_to`:
    /// what an older build does with a newer file, for the tests.
    fn load_reading(dir: PathBuf, up_to: u32) -> Store {
        let folder = greycard_library::collections::read_folder(&dir, up_to);
        let mut store = Store {
            dir: Some(dir.clone()),
            bases: folder
                .list
                .iter()
                .map(|c| {
                    let base = c.refs.iter().map(Reference::owned_pair).collect();
                    (c.id.clone(), base)
                })
                .collect(),
            list: folder.list,
            smart: folder.smart,
            stamps: folder.stamps,
            ..Store::default()
        };
        store.sort();
        tracing::info!(
            "collections: {} and {} smart, from {}",
            store.list.len(),
            store.smart.len(),
            dir.display()
        );
        store
    }

    /// By set, then name, case aside: the sidebar's order.
    pub fn sort(&mut self) {
        let key = |set: &str, name: &str| (set.to_lowercase(), name.to_lowercase());
        self.list.sort_by_key(|c| key(&c.set, &c.name));
        self.smart.sort_by_key(|s| key(&s.set, &s.name));
    }

    pub fn collection(&self, id: &str) -> Option<&Collection> {
        self.list.iter().find(|c| c.id == id)
    }

    pub fn collection_mut(&mut self, id: &str) -> Option<&mut Collection> {
        self.list.iter_mut().find(|c| c.id == id)
    }

    pub fn smart(&self, id: &str) -> Option<&Smart> {
        self.smart.iter().find(|s| s.id == id)
    }

    /// Whether `id` is taken: by a collection here, or by a file in
    /// the folder.
    pub fn id_taken(&self, id: &str) -> bool {
        self.collection(id).is_some()
            || self.smart(id).is_some()
            || [false, true]
                .iter()
                .any(|&smart| self.file_of(id, smart).is_some_and(|f| f.exists()))
    }

    /// Every member hash, each once.
    pub fn hashes(&self) -> HashSet<&str> {
        self.list
            .iter()
            .flat_map(|c| c.refs.iter().map(|r| r.hash.as_str()))
            .collect()
    }

    /// Every member frame, (hash, version), each once.
    pub fn pairs(&self) -> HashSet<Pair<'_>> {
        self.list
            .iter()
            .flat_map(|c| c.refs.iter().map(Reference::pair))
            .collect()
    }

    fn file_of(&self, id: &str, smart: bool) -> Option<PathBuf> {
        Some(file_of(self.dir.as_ref()?, id, smart))
    }

    /// Write the collection or the smart collection `id` to its file.
    /// Said in the log when it cannot be; the editor goes on. A file
    /// changed on disk since it was read or last written here (another
    /// editor, a hand edit, or only its time moved by a sync tool) is
    /// read first and joined against what it held then: a reference
    /// the other side added is put at our end, and one it took out is
    /// taken out of ours; what we added or took out ourselves stands.
    /// A file that will not read is kept beside itself as `.json.bad`
    /// before ours goes over it. A smart collection is ours, and the
    /// other is said in the log.
    pub fn save(&mut self, id: &str) {
        let smart = self.smart(id).is_some();
        if let Some(file) = self.file_of(id, smart)
            && let Some(now) = stamp_of(&file)
            && self.stamps.get(id) != Some(&now)
        {
            if smart {
                tracing::warn!(
                    "smart collection {}: changed on disk since it was read; ours written over it",
                    file.display()
                );
            } else {
                match read_collection(&file) {
                    Ok(theirs) if theirs.version <= VERSION => {
                        let base = self.bases.get(id).cloned().unwrap_or_default();
                        if let Some(c) = self.collection_mut(id) {
                            let on_disk: HashSet<Pair<'_>> =
                                theirs.refs.iter().map(Reference::pair).collect();
                            let before = c.refs.len();
                            c.refs.retain(|r| {
                                !base.contains(&r.owned_pair()) || on_disk.contains(&r.pair())
                            });
                            let dropped = before - c.refs.len();
                            let mut joined = 0;
                            for r in theirs.refs {
                                if !base.contains(&r.owned_pair()) && !c.has(r.pair()) {
                                    c.refs.push(r);
                                    joined += 1;
                                }
                            }
                            if joined + dropped > 0 {
                                tracing::warn!(
                                    "collection {}: changed on disk since it was read; \
                                     {joined} of its references joined to ours, {dropped} \
                                     taken out as it took them out",
                                    file.display()
                                );
                            }
                        }
                    }
                    Ok(_) => {
                        tracing::warn!(
                            "collection {}: a later build wrote it since; left alone",
                            file.display()
                        );
                        return;
                    }
                    Err(e) => {
                        // A second broken file goes over the earlier
                        // copy, on purpose: one is enough to recover a
                        // hand edit from, and names need no stamps.
                        let aside = file.with_extension("json.bad");
                        let kept = match std::fs::copy(&file, &aside) {
                            Ok(_) => format!("kept as {}", aside.display()),
                            Err(c) => format!("could not keep a copy ({c})"),
                        };
                        tracing::warn!(
                            "collection {}: changed on disk and unreadable ({e}); {kept}; ours written over it",
                            file.display()
                        );
                    }
                }
            }
        }
        let Some(dir) = self.dir.clone() else {
            return;
        };
        let (at, host) = (crate::carried::now(), crate::carried::host());
        if let Some(c) = self.collection_mut(id) {
            c.stamp(at, host);
        } else if let Some(s) = self.smart.iter_mut().find(|s| s.id == id) {
            s.stamp(at, host);
        }
        let written = if let Some(c) = self.collection(id) {
            c.write(&dir)
        } else if let Some(s) = self.smart(id) {
            s.write(&dir)
        } else {
            return;
        };
        match written {
            Ok(stamp) => {
                self.stamps.insert(id.to_string(), stamp);
                if let Some(c) = self.collection(id) {
                    let base = c.refs.iter().map(Reference::owned_pair).collect();
                    self.bases.insert(id.to_string(), base);
                }
                self.carry(id);
            }
            Err(e) => tracing::warn!(
                "collection not saved to {}: {e}",
                file_of(&dir, id, smart).display()
            ),
        }
    }

    /// `id` as it is carried; none when it is not here.
    pub fn item(&self, id: &str) -> Option<Item> {
        self.collection(id)
            .cloned()
            .map(Item::Collection)
            .or_else(|| self.smart(id).cloned().map(Item::Smart))
    }

    /// `id`'s copies beside its frames written as it stands.
    fn carry(&self, id: &str) {
        if let (Some(carrier), Some(item)) = (&self.carrier, self.item(id)) {
            carrier.put(item);
        }
    }

    /// Every one carried as it stands: the copies of a set made before
    /// the carry, or whose write failed, written; the rest pass at no
    /// cost.
    pub fn carry_all(&self) {
        for id in self
            .list
            .iter()
            .map(|c| &c.id)
            .chain(self.smart.iter().map(|s| &s.id))
        {
            self.carry(id);
        }
    }

    /// A collection or smart collection carried here from another
    /// machine, or joined with one, taken as ours and written: added,
    /// or put in place of ours. A smart collection keeps the folder
    /// ours looks under. Its id.
    pub fn take(&mut self, item: Item) -> Option<String> {
        let id = item.id().to_string();
        match item {
            Item::Collection(c) => match self.collection_mut(&id) {
                Some(ours) => *ours = c,
                None => self.list.push(c),
            },
            Item::Smart(mut s) => match self.smart.iter_mut().find(|o| o.id == id) {
                Some(ours) => {
                    if ours.root.is_some() == s.root.is_some() {
                        s.root = ours.root.clone();
                    }
                    *ours = s;
                }
                None => self.smart.push(s),
            },
            Item::Stack(_) => return None,
        }
        self.sort();
        self.save(&id);
        Some(id)
    }

    /// The collection or smart collection `id` gone, with its file.
    pub fn delete(&mut self, id: &str) -> bool {
        if let (Some(carrier), Some(item)) = (&self.carrier, self.item(id)) {
            carrier.bury(greycard_library::carry::Gone::of(&item));
        }
        let smart = self.smart(id).is_some();
        let before = self.list.len() + self.smart.len();
        self.list.retain(|c| c.id != id);
        self.smart.retain(|s| s.id != id);
        if self.target.as_deref() == Some(id) {
            self.target = None;
        }
        if let Some(file) = self.file_of(id, smart) {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("collection {}: not deleted: {e}", file.display()),
            }
        }
        before != self.list.len() + self.smart.len()
    }

    /// The files the index found rewritten in place, healed into the
    /// references. First by the hint: a reference to the old hash whose
    /// hint is the rewritten file's path follows the file, to its new
    /// hash. Then, for a reference to an old hash that no rewrite's path
    /// matched, only when the index now has that hash nowhere (`copies`
    /// lists its files, the missing left out; `None` when the index
    /// could not say, and then nothing is taken for gone): the picture
    /// was rewritten wherever it was, and the reference follows the
    /// first rewrite of it. A reference whose new hash another holds
    /// already is never dropped: while a copy of its own picture
    /// survives it is pointed at that copy, and with none known it
    /// stays as it was, missing. Only a member's hash is asked of the
    /// index.
    /// Each collection changed is written; their ids.
    pub fn heal(
        &mut self,
        rehashed: &[Rehash],
        copies: &dyn Fn(&str) -> Option<Vec<PathBuf>>,
    ) -> Vec<String> {
        let members: HashSet<String> = self.hashes().into_iter().map(str::to_string).collect();
        let ours: Vec<&Rehash> = rehashed
            .iter()
            .filter(|r| r.old != r.new && members.contains(&r.old))
            .collect();
        if ours.is_empty() {
            return Vec::new();
        }
        // Each old hash's first copy left, asked once: `None` when the
        // index could not say, `Some(None)` when there is none.
        let mut left: std::collections::HashMap<&str, Option<Option<String>>> =
            std::collections::HashMap::new();
        for r in &ours {
            left.entry(r.old.as_str()).or_insert_with(|| {
                copies(&r.old).map(|c| c.iter().map(|p| hint_of(p)).find(|p| !p.is_empty()))
            });
        }
        let by_path: std::collections::HashMap<(&str, String), &Rehash> = ours
            .iter()
            .map(|r| ((r.old.as_str(), hint_of(&r.path)), *r))
            .collect();
        let mut changed = Vec::new();
        for c in &mut self.list {
            // What each reference heals by, decided before any changes:
            // its own path first, then the fallback. A version of a
            // picture heals as the picture does: its id lives in the
            // sidecar, which a rewrite of the file leaves alone.
            let mut plan: Vec<((String, Option<String>), &Rehash)> = Vec::new();
            for x in &c.refs {
                if let Some(r) = by_path.get(&(x.hash.as_str(), x.hint.clone())) {
                    plan.push((x.owned_pair(), r));
                }
            }
            for x in &c.refs {
                if plan.iter().any(|(p, _)| *p == x.owned_pair()) {
                    continue;
                }
                if matches!(left.get(x.hash.as_str()), Some(Some(None)))
                    && let Some(r) = ours.iter().find(|r| r.old == x.hash)
                {
                    plan.push((x.owned_pair(), r));
                }
            }
            let mut any = false;
            for ((old, version), r) in plan {
                let survivor = left.get(old.as_str()).cloned().flatten().flatten();
                any |= c.take_hash(
                    (&old, version.as_deref()),
                    &r.new,
                    &hint_of(&r.path),
                    survivor.as_deref(),
                );
            }
            if any {
                changed.push(c.id.clone());
            }
        }
        for id in &changed {
            tracing::info!(
                "collection {}: healed by its hints after a file was rewritten",
                self.collection(id).map(|c| c.name.as_str()).unwrap_or("?")
            );
            self.save(id);
        }
        changed
    }

    /// The references of collection `id` the index has nowhere, by the
    /// same test the status line counts them missing by ([`resolve`],
    /// asked again here), dropped and the file written. Kept, as perhaps
    /// only out of reach: one whose hint is under a root in `away`
    /// (offline), under a network mount in `far`, or in a folder that
    /// does not answer `answers` (the editor's looks, each with a time
    /// limit, so a share gone quiet never holds the caller); and one
    /// whose hint is a file on disk, which a pass has yet to read, or
    /// that the disk will not say is gone (a folder that cannot be read,
    /// a mount that died). The disk is asked of a hint only once its
    /// folder has answered, and a reference is dropped only when the
    /// disk says its file, or its folder, is not found. With
    /// no index to ask, or one that will not answer, nothing is known
    /// gone and nothing changes (`None`). The caller makes sure no pass
    /// is under way: the index says a file moved is missing until the
    /// pass reaches where it went. A version whose file is there and
    /// whose id the index's row does not list is dropped only when the
    /// sidecar on disk reads and lacks the id; one that will not read
    /// cannot tell, and keeps it.
    pub fn forget_missing(
        &mut self,
        id: &str,
        lib: Option<&Library>,
        away: &[PathBuf],
        far: &[PathBuf],
        answers: &Answers<'_>,
    ) -> Option<Forgot> {
        let c = self.collection(id)?;
        let shown = match resolve(lib?, &c.refs, away, &[], None) {
            Ok(shown) => shown,
            Err(e) => {
                tracing::warn!("collection {}: {e}; nothing forgotten", c.name);
                return None;
            }
        };
        let mut kept_away = shown.away;
        let under_far = |p: &Path| far.iter().any(|f| p.starts_with(f));
        let (far_off, near): (Vec<&Reference>, Vec<&Reference>) = shown
            .gone
            .iter()
            .partition(|r| under_far(Path::new(&r.hint)));
        let (far_lost, lost): (Vec<_>, Vec<_>) =
            shown.lost.iter().partition(|(_, at)| under_far(at));
        kept_away += far_off.len() + far_lost.len();
        let mut folders: Vec<PathBuf> = Vec::new();
        let looked = near
            .iter()
            .map(|r| Path::new(&r.hint))
            .chain(lost.iter().map(|(_, at)| at.as_path()));
        for p in looked {
            if let Some(dir) = p.parent()
                && !folders.iter().any(|d| d == dir)
            {
                folders.push(dir.to_path_buf());
            }
        }
        let answered = if folders.is_empty() {
            Default::default()
        } else {
            answers(&folders)
        };
        let answer_of = |p: &Path| p.parent().and_then(|d| answered.get(d)).copied().flatten();
        let mut on_disk = 0;
        let mut unsure = 0;
        let mut gone: HashSet<Pair<'_>> = HashSet::new();
        for r in near {
            let hint = Path::new(&r.hint);
            let absent = |p: &Path| {
                matches!(
                    std::fs::symlink_metadata(p),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound
                )
            };
            match answer_of(hint) {
                None => kept_away += 1,
                // The folder not there at all: its files neither.
                Some(false) if hint.parent().is_some_and(absent) => {
                    gone.insert(r.pair());
                }
                // Gone only when the disk says so of the file itself: a
                // folder that cannot be read, a mount that has died
                // under us, any other error, keeps it.
                Some(_) => match std::fs::symlink_metadata(hint) {
                    Ok(_) => on_disk += 1,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        gone.insert(r.pair());
                    }
                    Err(_) => kept_away += 1,
                },
            }
        }
        // A version whose file is there: gone only when the sidecar
        // reads and lacks the id. One that will not read, or none where
        // the version was, cannot tell, and keeps it.
        for (r, at) in lost {
            let Some(version) = r.version.as_deref() else {
                continue;
            };
            match answer_of(at) {
                None => kept_away += 1,
                Some(_) => match greycard_edit::Sidecar::load_as_file(at) {
                    Ok(Some(s)) if s.version(version).is_none() => {
                        gone.insert(r.pair());
                    }
                    // In the sidecar after all: the index has yet to
                    // read it.
                    Ok(Some(_)) => on_disk += 1,
                    Ok(None) | Err(_) => unsure += 1,
                },
            }
        }
        let n = self.collection_mut(id)?.remove(&gone);
        if n > 0 {
            tracing::info!(
                "collection {}: {n} missing forgotten",
                self.collection(id).map_or("?", |c| c.name.as_str())
            );
            self.save(id);
        }
        let forgot = Forgot {
            forgotten: n,
            missing: shown.missing - n,
            away: kept_away,
            on_disk,
            unsure,
        };
        self.missing = Some((id.to_string(), forgot.missing, 0));
        Some(forgot)
    }

    /// What a read of a collection found to change in it, put in: each
    /// reference of `was` takes its new hash and hint. True when the
    /// collection changed, which is then written.
    pub fn apply(&mut self, id: &str, changes: &[Change]) -> bool {
        let Some(c) = self.collection_mut(id) else {
            return false;
        };
        let mut any = false;
        // A read changes a reference's hash only when nothing of its own
        // is left in the index: no copy survives it.
        for ch in changes {
            any |= c.take_hash((&ch.was, ch.version.as_deref()), &ch.hash, &ch.hint, None);
        }
        if any {
            self.save(id);
        }
        any
    }
}

/// The editor's look at folders, each with a time limit: whether each
/// is there, or `None` when it did not answer in time.
pub type Answers<'a> = dyn Fn(&[PathBuf]) -> std::collections::HashMap<PathBuf, Option<bool>> + 'a;

/// What Forget missing did, by its own fresh read: how many it dropped,
/// how many are missing still, and of those, how many are perhaps only
/// out of reach (an offline root, a network mount, a folder that did
/// not answer) and how many have a file at the hint the index has not
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Forgot {
    pub forgotten: usize,
    pub missing: usize,
    pub away: usize,
    pub on_disk: usize,
    /// Versions whose file is there and whose sidecar will not read, or
    /// is not there: whether the version is gone cannot be told.
    pub unsure: usize,
}

/// A reference that a read of its collection found somewhere else: the
/// hash it had and its version, and the hash and the hint it has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub was: String,
    pub version: Option<String>,
    pub hash: String,
    pub hint: String,
}

/// A collection read from the index for the browser.
#[derive(Debug, Clone, Default)]
pub struct Shown {
    /// The files to list, each once, in the order their first frame
    /// comes.
    pub listed: Vec<(PathBuf, RowMeta)>,
    /// The frames to show, each file's path and the version of it, each
    /// (hash, version) once, in the collection's order or by capture
    /// time: two versions of one picture are two frames of one file. A
    /// version the index does not list (`lost`) is among them too, at
    /// the file it was looked for in when that file is listed: the
    /// window shows it while the sidecar it holds has it.
    pub frames: Vec<(PathBuf, Option<String>)>,
    /// References with nothing at their hash or their hint, and
    /// versions their file's row does not list.
    pub missing: usize,
    /// The missing whose hint is under no root that is away: what
    /// Forget missing may drop, once it has looked at the disk.
    pub gone: Vec<Reference>,
    /// The versions whose file is found and whose sidecar, as the index
    /// last read it, does not have them (deleted here or on another
    /// machine): each with the file it was looked for in, under no root
    /// that is away. Missing, and never dropped by a read, since a join
    /// may bring the version back; Forget missing asks the sidecar
    /// itself.
    pub lost: Vec<(Reference, PathBuf)>,
    /// Of the missing, those hinted under a root that is away: perhaps
    /// only out of reach, and kept.
    pub away: usize,
    /// References found moved or rewritten, for the file.
    pub changes: Vec<Change>,
}

/// Where each of `refs` is shown from, by the index alone: the hinted
/// file when the index has it there and its root is not `away`, else
/// the first of the hash's files under a root that is not an archive,
/// else any that is not away, else the hinted one (shown dimmed) or
/// the first. A reference whose hash the index has nowhere takes the
/// hash of the file at its hint, when there is one (a file rewritten
/// while the editor was not looking). A hint with no file of the hash
/// at it moves to the file shown. A reference to a version is shown
/// from a file whose sidecar has it, as the index's row lists it, and is
/// missing when none has; each (hash, version) once. With an `order`,
/// sorted by it as a view of the index is (its ties by name, then
/// path), a version after its file's own and otherwise the
/// collection's order kept among equals; with none, in the
/// collection's own order.
pub fn resolve(
    lib: &Library,
    refs: &[Reference],
    away: &[PathBuf],
    archives: &[PathBuf],
    order: Option<greycard_library::Order>,
) -> greycard_library::Result<Shown> {
    // Each frame in order: its file, its row (none for a version the
    // index does not list), its version and when it was taken.
    type FoundFrame = (PathBuf, Option<RowMeta>, Option<String>, Option<String>);
    let mut shown = Shown::default();
    let mut found: Vec<FoundFrame> = Vec::new();
    let mut seen: HashSet<(String, Option<String>)> = HashSet::new();
    let is_away = |p: &Path| away.iter().any(|r| p.starts_with(r));
    let on_archive = |p: &Path| archives.iter().any(|r| p.starts_with(r));
    let held: HashSet<&str> = refs.iter().map(|r| r.hash.as_str()).collect();
    for r in refs {
        let mut hash = r.hash.clone();
        let mut rows = lib.rows_by_hash(&hash)?;
        // The file at the hint is taken for this picture rewritten only
        // when it is no picture the collection holds already: a file
        // overwritten with another member's content is that member, and
        // this reference stays missing rather than be dropped for it.
        if rows.is_empty()
            && let Some(at_hint) = lib.by_path(Path::new(&r.hint))?.filter(|e| !e.missing)
            && !held.contains(at_hint.hash.as_str())
        {
            hash = at_hint.hash;
            rows = lib.rows_by_hash(&hash)?;
        }
        // Each frame once, missing or not: a picture's own and each of
        // its versions.
        if !seen.insert((hash.clone(), r.version.clone())) {
            continue;
        }
        if rows.is_empty() {
            shown.missing += 1;
            // The index alone: the disk is never asked here.
            if is_away(Path::new(&r.hint)) {
                shown.away += 1;
            } else {
                shown.gone.push(r.clone());
            }
            continue;
        }
        // A version is in the sidecar of the copy it was made on, not
        // in every copy of the picture: only the rows whose sidecar, as
        // the index last read it, has the id can show it.
        if let Some(id) = r.version.as_deref() {
            let has = |h: &greycard_library::HashRow| h.row.versions.iter().any(|v| v.id == id);
            let at_hint = rows.iter().position(|h| hint_of(&h.path) == r.hint);
            // Where the version was to be: the hinted copy, or with none
            // of the hash there, the file at the hint (rewritten in place
            // beside a copy that survives, before any heal), never
            // another copy, whose sidecar says nothing of it.
            let (looked_in, taken) = match at_hint {
                Some(i) => (rows[i].path.clone(), rows[i].taken.clone()),
                None => (PathBuf::from(&r.hint), None),
            };
            rows.retain(has);
            // The file at the hint rewritten and its sidecar keeping the
            // version: shown from it, the reference taking its hash.
            if rows.is_empty()
                && at_hint.is_none()
                && let Some(at) = lib.by_path(Path::new(&r.hint))?.filter(|e| !e.missing)
                && at.hash != hash
            {
                let there: Vec<_> = lib
                    .rows_by_hash(&at.hash)?
                    .into_iter()
                    .filter(|h| hint_of(&h.path) == r.hint && has(h))
                    .collect();
                if !there.is_empty() {
                    hash = at.hash;
                    rows = there;
                }
            }
            if rows.is_empty() {
                if hash != r.hash {
                    shown.changes.push(Change {
                        was: r.hash.clone(),
                        version: r.version.clone(),
                        hash,
                        hint: r.hint.clone(),
                    });
                }
                shown.missing += 1;
                if is_away(&looked_in) {
                    shown.away += 1;
                } else {
                    shown.lost.push((r.clone(), looked_in.clone()));
                }
                // Kept among the frames in its place: the window shows
                // it while the sidecar it holds has the version (one just
                // made, the index not caught up), and not once a read
                // says it is gone.
                found.push((looked_in, None, r.version.clone(), taken));
                continue;
            }
        }
        let hinted = rows.iter().position(|h| hint_of(&h.path) == r.hint);
        let pick = hinted
            .filter(|&i| !is_away(&rows[i].path))
            .or_else(|| {
                rows.iter()
                    .position(|h| !is_away(&h.path) && !on_archive(&h.path))
            })
            .or_else(|| rows.iter().position(|h| !is_away(&h.path)))
            .or(hinted)
            .unwrap_or(0);
        let row = rows.swap_remove(pick);
        // The hint moves when nothing of the hash is at it any more.
        let hint = if hinted.is_none() {
            hint_of(&row.path)
        } else {
            r.hint.clone()
        };
        if hash != r.hash || hint != r.hint {
            shown.changes.push(Change {
                was: r.hash.clone(),
                version: r.version.clone(),
                hash,
                hint,
            });
        }
        found.push((row.path, Some(row.row), r.version.clone(), row.taken));
    }
    if let Some(order) = order {
        // By the order's rule; the sort is stable, so the collection's
        // order holds among the frames of one file past its own.
        fn key(f: &FoundFrame) -> greycard_library::SortKey<'_> {
            match &f.1 {
                Some(row) => row.sort_key(&f.0),
                None => greycard_library::SortKey {
                    taken: f.3.as_deref(),
                    ..greycard_library::SortKey::bare(&f.0)
                },
            }
        }
        found.sort_by(|a, b| {
            order
                .compare(&key(a), &key(b))
                .then_with(|| a.2.is_some().cmp(&b.2.is_some()))
        });
    }
    // The files listed are those a frame was found in; a version the
    // index does not list is kept among the frames only where its file
    // is listed for another frame.
    let mut files: HashSet<PathBuf> = HashSet::new();
    let mut frames = Vec::with_capacity(found.len());
    for (path, row, version, _) in found {
        if let Some(row) = row
            && files.insert(path.clone())
        {
            shown.listed.push((path.clone(), row));
        }
        frames.push((path, version));
    }
    shown.frames = frames
        .into_iter()
        .filter(|(p, _)| files.contains(p))
        .collect();
    Ok(shown)
}

#[cfg(test)]
mod tests;
