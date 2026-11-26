//! Collections, smart collections, and stacks carried beside their
//! frames, so a set gathered on one machine reaches another through an
//! archive or a backup, as the sidecars do.
//!
//! The files beside the library stay the source on each machine
//! (`collections`, `stacks`). Every save of one is also written, whole,
//! into `.greycard/sets/` of the folders it touches: a collection into
//! each folder one of its frames is in, a stack into its one folder, a
//! smart collection into the folder it looks under, or into the top of
//! every root when it looks under all of them; and each of those into
//! the folder it was backed up to. A copy is the machine's file in the
//! same shape, `<id>.json`, `<id>.smart.json`, or `<id>.stack.json`,
//! with a member in that folder kept by its file name alone, so it
//! reads the same wherever the folder is mounted; a member elsewhere
//! keeps the path it had on the machine that wrote it, as a hint. A
//! smart collection's folder is written `.`, the folder the copy is
//! in. Whether a stack is open is each machine's own, and not carried.
//!
//! Each carries a [`Lineage`]: its revisions, each the hash of the one
//! before and of what it holds (never where: the hints and the open are
//! left out of a collection's), so two copies tell which has seen the
//! other, as a sidecar's revisions do. A copy the other has seen is
//! behind, and the other wins whatever the clocks say. Two that went
//! their own ways are a conflict: the one written later keeps the id,
//! and the other is kept beside it as "<name> (on <host>)" under an id
//! made from the two, so both machines make the same pair ([`join`]).
//! A stack is an order and a top, whole, and keeps the later alone.
//! Deleting one leaves `<id>.gone.json` with its lineage, which takes
//! the copies it has seen and no copy edited since.
//!
//! A copy of a later shape than this build reads is left alone, never
//! taken and never written over: a smart collection holding a chip out
//! is shape 3, as its file beside the library is.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::collections::{
    self, Collection, CollectionFile, PLAIN_END, Reference, SMART_END, Smart, SmartFile, hint_of,
    write_atomic,
};
use crate::roots::Roots;
use crate::stacks::{self, Stack, StackFile};

/// The folder the copies are kept in, under the sidecars' folder.
pub const DIR: &str = "sets";

/// The revisions a lineage keeps, oldest dropped first: a copy left
/// behind by more saves than this reads as gone its own way, and is
/// kept beside the newer rather than lost.
pub const KEPT: usize = 256;

/// The shape of a deletion's file.
pub const GONE_VERSION: u32 = 1;

/// How old a part file is before a pass takes it for one a write left:
/// a write is waited on for seconds, but one on a share that stopped
/// answering may finish much later, and another machine's clock may be
/// hours off ours. A part file is a few kilobytes, so a day's wait
/// costs nothing.
pub const PART_STALE: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a deletion's file is kept. No machine says what it has
/// read, so when every one has seen a deletion cannot be told from the
/// folder; this stands in. A machine away longer can write the set back
/// there, and the machine that deleted it, which remembers its own
/// deletions, buries that copy again when a pass meets it.
pub const GONE_KEPT_FOR: Duration = Duration::from_secs(180 * 24 * 60 * 60);

const STACK_END: &str = ".stack.json";
const GONE_END: &str = ".gone.json";
const PART_END: &str = ".part";

/// The revision before the first.
const ZERO: &str = "00000000000000000000000000000000";

/// The first sixteen bytes of a hash, as hex.
fn short(hash: blake3::Hash) -> String {
    hash.to_hex()[..32].to_string()
}

/// The hash of what `value` says, as compact JSON.
fn sum_of(value: &impl Serialize) -> String {
    short(blake3::hash(&serde_json::to_vec(value).unwrap_or_default()))
}

/// What has been made of one collection or stack: its revisions, the
/// hash of what it held at the last, and when and on which machine that
/// was made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lineage {
    /// Oldest first, the last its current; [`KEPT`] at most. A
    /// revision is the hash of the one before and the content's.
    pub revs: Vec<String>,
    /// The content's hash at the current revision.
    pub sum: String,
    /// Seconds since the epoch.
    pub at: u64,
    pub host: String,
}

impl Lineage {
    pub fn is_empty(&self) -> bool {
        self.revs.is_empty()
    }

    /// The current revision; empty for none.
    pub fn current(&self) -> &str {
        self.revs.last().map_or("", String::as_str)
    }

    /// Whether revision `rev` is in it.
    pub fn knows(&self, rev: &str) -> bool {
        !rev.is_empty() && self.revs.iter().any(|r| r == rev)
    }

    /// A revision made when the content's hash `sum` is not the
    /// current one's, or when there is none yet. True when it was.
    pub fn stamp(&mut self, sum: String, at: u64, host: &str) -> bool {
        if !self.revs.is_empty() && self.sum == sum {
            return false;
        }
        let prev = self.revs.last().map_or(ZERO, String::as_str);
        let rev = short(blake3::hash(format!("{prev}\n{sum}").as_bytes()));
        self.revs.push(rev);
        self.sum = sum;
        self.at = at;
        self.host = host.to_string();
        self.cap();
        true
    }

    /// `other`'s revisions taken in as seen, before the current one.
    /// True when any was new.
    pub fn absorb(&mut self, other: &Lineage) -> bool {
        let Some(current) = self.revs.pop() else {
            return false;
        };
        let mut new = false;
        for r in &other.revs {
            if *r != current && !self.revs.contains(r) {
                self.revs.push(r.clone());
                new = true;
            }
        }
        self.revs.push(current);
        self.cap();
        new
    }

    fn cap(&mut self) {
        let n = self.revs.len();
        if n > KEPT {
            self.revs.drain(..n - KEPT);
        }
    }

    /// Whether this one was made later than `other`: by the time, then
    /// the revision, then the host, so two machines order a pair alike.
    pub fn later_than(&self, other: &Lineage) -> bool {
        (self.at, self.current(), &self.host) > (other.at, other.current(), &other.host)
    }
}

/// Two copies of one collection or stack, as one stands to the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// The same revision, or the same content.
    Same,
    /// Ours has seen theirs, and gone on.
    Ahead,
    /// Theirs has seen ours.
    Behind,
    /// Neither has seen the other.
    Diverged,
}

/// How `ours` stands to `theirs`.
pub fn compare(ours: &Lineage, theirs: &Lineage) -> Order {
    if ours.current() == theirs.current() || (!ours.sum.is_empty() && ours.sum == theirs.sum) {
        Order::Same
    } else if ours.knows(theirs.current()) {
        Order::Ahead
    } else if theirs.knows(ours.current()) {
        Order::Behind
    } else {
        Order::Diverged
    }
}

/// Which of the three a copy is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Collection,
    Smart,
    Stack,
}

impl Kind {
    fn end(self) -> &'static str {
        match self {
            Kind::Collection => PLAIN_END,
            Kind::Smart => SMART_END,
            Kind::Stack => STACK_END,
        }
    }
}

/// The content of a collection, as its revisions hash it: its name,
/// sets, time made, and frames in order, not where they are.
pub fn collection_sum(c: &Collection) -> String {
    let refs: Vec<(&str, Option<&str>)> = c.refs.iter().map(Reference::pair).collect();
    sum_of(&("collection", &c.name, &c.set, c.made, refs))
}

/// A smart collection's: its name, sets, time made, filter, and whether
/// it looks under one folder (which folder is each machine's).
pub fn smart_sum(s: &Smart) -> String {
    sum_of(&(
        "smart",
        &s.name,
        &s.set,
        s.made,
        &s.filter,
        s.root.is_some(),
    ))
}

/// A stack's: its time made and its members in order, each by its
/// hash, its version, and its file's name, since a stack finds its
/// frames by their paths in the one folder.
pub fn stack_sum(s: &Stack) -> String {
    let refs: Vec<(&str, Option<&str>, String)> = s
        .refs
        .iter()
        .map(|r| {
            let name = Path::new(&r.hint)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (r.hash.as_str(), r.version.as_deref(), name)
        })
        .collect();
    sum_of(&("stack", s.made, refs))
}

impl Collection {
    /// A revision made when what it holds changed. True when it was.
    pub fn stamp(&mut self, at: u64, host: &str) -> bool {
        let sum = collection_sum(self);
        self.lineage.stamp(sum, at, host)
    }

    /// Its first revision, from a file written before lineages: the
    /// same on every machine for the same content.
    pub fn seed(&mut self) {
        if self.lineage.is_empty() {
            self.stamp(self.made, "");
        }
    }
}

impl Smart {
    pub fn stamp(&mut self, at: u64, host: &str) -> bool {
        let sum = smart_sum(self);
        self.lineage.stamp(sum, at, host)
    }

    pub fn seed(&mut self) {
        if self.lineage.is_empty() {
            self.stamp(self.made, "");
        }
    }
}

impl Stack {
    pub fn stamp(&mut self, at: u64, host: &str) -> bool {
        let sum = stack_sum(self);
        self.lineage.stamp(sum, at, host)
    }

    pub fn seed(&mut self) {
        if self.lineage.is_empty() {
            self.stamp(self.made, "");
        }
    }
}

/// One collection, smart collection, or stack, as it is carried.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Collection(Collection),
    Smart(Smart),
    Stack(Stack),
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::Collection(c) => &c.id,
            Item::Smart(s) => &s.id,
            Item::Stack(s) => &s.id,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Item::Collection(_) => Kind::Collection,
            Item::Smart(_) => Kind::Smart,
            Item::Stack(_) => Kind::Stack,
        }
    }

    pub fn lineage(&self) -> &Lineage {
        match self {
            Item::Collection(c) => &c.lineage,
            Item::Smart(s) => &s.lineage,
            Item::Stack(s) => &s.lineage,
        }
    }

    pub fn lineage_mut(&mut self) -> &mut Lineage {
        match self {
            Item::Collection(c) => &mut c.lineage,
            Item::Smart(s) => &mut s.lineage,
            Item::Stack(s) => &mut s.lineage,
        }
    }

    fn stamp(&mut self, at: u64, host: &str) -> bool {
        match self {
            Item::Collection(c) => c.stamp(at, host),
            Item::Smart(s) => s.stamp(at, host),
            Item::Stack(s) => s.stamp(at, host),
        }
    }

    fn seed(&mut self) {
        match self {
            Item::Collection(c) => c.seed(),
            Item::Smart(s) => s.seed(),
            Item::Stack(s) => s.seed(),
        }
    }

    fn refs_mut(&mut self) -> Option<&mut Vec<Reference>> {
        match self {
            Item::Collection(c) => Some(&mut c.refs),
            Item::Smart(_) => None,
            Item::Stack(s) => Some(&mut s.refs),
        }
    }
}

/// A deletion, carried: the id, what it was, and its lineage when it
/// went. It takes a copy whose revision it has seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gone {
    pub id: String,
    pub kind: Kind,
    pub lineage: Lineage,
}

impl Gone {
    /// The deletion of `item` as it stands.
    pub fn of(item: &Item) -> Gone {
        Gone {
            id: item.id().to_string(),
            kind: item.kind(),
            lineage: item.lineage().clone(),
        }
    }

    /// Whether a copy of this lineage is one the deletion saw: its
    /// revision, or its content, among the deleted one's.
    pub fn covers(&self, l: &Lineage) -> bool {
        self.lineage.knows(l.current()) || (!l.sum.is_empty() && l.sum == self.lineage.sum)
    }
}

#[derive(Serialize, Deserialize)]
struct GoneFile {
    version: u32,
    kind: Kind,
    lineage: Lineage,
}

/// What a join of two copies of one id makes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Joined {
    /// What ours becomes; none when ours stands as it is.
    pub keep: Option<Item>,
    /// The other side of a conflict, kept beside it under an id of its
    /// own.
    pub copy: Option<Item>,
}

/// The id the older side of a conflict is kept under: made from the
/// id and its revision, so both machines make the same one.
pub fn copy_id(id: &str, rev: &str) -> String {
    short(blake3::hash(format!("{id}\n{rev}").as_bytes()))
}

/// `item` with each reference's hint taken from `hints`, by its frame,
/// where ours has one: our machine's paths for the frames we know.
fn with_hints(mut item: Item, hints: &[Reference]) -> Item {
    // A stack's members are its folder's by name, and the names are
    // part of it: ours' folder, their names.
    if let Item::Stack(s) = &mut item {
        if let Some(folder) = hints.first().and_then(|h| Path::new(&h.hint).parent()) {
            for r in &mut s.refs {
                if let Some(name) = Path::new(&r.hint).file_name() {
                    r.hint = hint_of(&folder.join(name));
                }
            }
        }
        return item;
    }
    if let Some(refs) = item.refs_mut() {
        for r in refs {
            if let Some(h) = hints.iter().find(|h| h.pair() == r.pair()) {
                r.hint = h.hint.clone();
            }
        }
    }
    item
}

fn refs_of(item: &Item) -> &[Reference] {
    match item {
        Item::Collection(c) => &c.refs,
        Item::Smart(_) => &[],
        Item::Stack(s) => &s.refs,
    }
}

/// Theirs joined into ours, one id. The same content or ours ahead
/// leaves ours (its lineage taking theirs in, for the same); theirs
/// ahead takes theirs, with ours' hints for the frames both hold. Two
/// that went their own ways: the later keeps the id, having seen the
/// other, and for a collection or a smart collection the earlier is
/// kept beside it as "<name> (on <host>)", or "<name> (other copy)"
/// when its machine has no name, under [`copy_id`]. A stack keeps the
/// later alone. The same pair joined on either machine makes the same
/// two.
pub fn join(ours: &Item, theirs: &Item) -> Joined {
    match compare(ours.lineage(), theirs.lineage()) {
        Order::Same => {
            let mut keep = ours.clone();
            if keep.lineage_mut().absorb(theirs.lineage()) {
                Joined {
                    keep: Some(keep),
                    copy: None,
                }
            } else {
                Joined::default()
            }
        }
        Order::Ahead => Joined::default(),
        Order::Behind => Joined {
            keep: Some(with_hints(theirs.clone(), refs_of(ours))),
            copy: None,
        },
        Order::Diverged => {
            let ours_later = ours.lineage().later_than(theirs.lineage());
            let (newer, older) = if ours_later {
                (ours.clone(), theirs)
            } else {
                (with_hints(theirs.clone(), refs_of(ours)), ours)
            };
            let mut keep = newer;
            keep.lineage_mut().absorb(older.lineage());
            let copy = match older {
                Item::Stack(_) => None,
                _ => Some(conflict_copy(older)),
            };
            Joined {
                keep: Some(keep),
                copy,
            }
        }
    }
}

/// The older side of a conflict, under its own id and name.
fn conflict_copy(older: &Item) -> Item {
    let id = copy_id(older.id(), older.lineage().current());
    let host = older.lineage().host.clone();
    let named = |name: &str| {
        if host.is_empty() {
            format!("{name} (other copy)")
        } else {
            format!("{name} (on {host})")
        }
    };
    let mut copy = older.clone();
    match &mut copy {
        Item::Collection(c) => {
            c.id = id;
            c.name = named(&c.name);
        }
        Item::Smart(s) => {
            s.id = id;
            s.name = named(&s.name);
        }
        Item::Stack(s) => s.id = id,
    }
    // Named anew, it is a revision of its own, made from the older's:
    // the same on both machines, whatever their clocks.
    let (at, host) = (older.lineage().at, older.lineage().host.clone());
    copy.stamp(at, &host);
    copy
}

/// The folder the copies of `folder` are kept in.
pub fn sets_dir(folder: &Path) -> PathBuf {
    folder.join(greycard_edit::SIDECAR_FOLDER).join(DIR)
}

/// The name of `id`'s copy, of the kind given.
pub fn file_name(id: &str, kind: Kind) -> String {
    format!("{id}{}", kind.end())
}

fn gone_name(id: &str) -> String {
    format!("{id}{GONE_END}")
}

/// A hint as a copy in `folder` keeps it: a frame of that folder by its
/// name alone.
fn hint_in(folder: &Path, hint: &str) -> String {
    let path = Path::new(hint);
    match (path.parent(), path.file_name()) {
        (Some(p), Some(name)) if p == folder => name.to_string_lossy().into_owned(),
        _ => hint.to_string(),
    }
}

/// A hint read from a copy in `folder`: a name alone is that folder's.
fn hint_from(folder: &Path, hint: &str) -> String {
    match Path::new(hint).parent() {
        Some(p) if !p.as_os_str().is_empty() => hint.to_string(),
        _ => hint_of(&folder.join(hint)),
    }
}

/// `item` as its copy is written for the folder `home`: its frames
/// there by name. `home` is the folder the copy is in, or for a copy in
/// a backup, the folder that was backed up there.
pub fn to_text(item: &Item, home: &Path) -> String {
    let local = |refs: &[Reference]| -> Vec<Reference> {
        refs.iter()
            .map(|r| Reference {
                hint: hint_in(home, &r.hint),
                ..r.clone()
            })
            .collect()
    };
    let text = match item {
        Item::Collection(c) => serde_json::to_string_pretty(&CollectionFile {
            version: c.file_version(),
            name: c.name.clone(),
            set: c.set.clone(),
            made: c.made,
            refs: local(&c.refs),
            lineage: c.lineage.clone(),
        }),
        Item::Smart(s) => {
            let mut f: SmartFile = s.to_file();
            f.root = s.root.as_ref().map(|_| ".".to_string());
            serde_json::to_string_pretty(&f)
        }
        Item::Stack(s) => serde_json::to_string_pretty(&StackFile {
            version: stacks::VERSION,
            made: s.made,
            open: false,
            refs: local(&s.refs),
            lineage: s.lineage.clone(),
        }),
    };
    text.unwrap_or_default()
}

/// What a copy's file holds, read. Made once a file and taken apart
/// at once, so the size of its largest arm costs nothing.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Parsed {
    Item(Item),
    Gone(Gone),
    /// A later build's shape: left alone.
    Later,
}

/// The kind and id a copy's file name says; none for a name that is no
/// copy's.
fn name_kind(name: &str) -> Option<(&str, Option<Kind>)> {
    let (id, kind) = if let Some(id) = name.strip_suffix(GONE_END) {
        (id, None)
    } else if let Some(id) = name.strip_suffix(SMART_END) {
        (id, Some(Kind::Smart))
    } else if let Some(id) = name.strip_suffix(STACK_END) {
        (id, Some(Kind::Stack))
    } else {
        (name.strip_suffix(PLAIN_END)?, Some(Kind::Collection))
    };
    (!id.is_empty() && !id.contains('.')).then_some((id, kind))
}

/// The copy named `name` in `folder`, from its bytes, read by a build
/// that reads shapes up to `up_to` (for a collection or a smart one;
/// [`stacks::VERSION`] for a stack): its hints made this machine's, a
/// smart collection's `.` this folder.
pub fn parse_reading(
    folder: &Path,
    name: &str,
    bytes: &[u8],
    up_to: u32,
) -> Result<Parsed, String> {
    let (id, kind) = name_kind(name).ok_or_else(|| format!("{name}: not a carried set"))?;
    let id = id.to_string();
    let err = |e: serde_json::Error| e.to_string();
    // The shape first: a later build's file may not fit ours at all,
    // and is left alone whatever else it holds.
    #[derive(Deserialize)]
    struct Shape {
        version: u32,
    }
    let shape: Shape = serde_json::from_slice(bytes).map_err(err)?;
    let ours = match kind {
        None => GONE_VERSION,
        Some(Kind::Stack) => stacks::VERSION,
        Some(_) => up_to,
    };
    if shape.version > ours {
        return Ok(Parsed::Later);
    }
    let mut item = match kind {
        None => {
            let f: GoneFile = serde_json::from_slice(bytes).map_err(err)?;
            return Ok(Parsed::Gone(Gone {
                id,
                kind: f.kind,
                lineage: f.lineage,
            }));
        }
        Some(Kind::Collection) => {
            let f: CollectionFile = serde_json::from_slice(bytes).map_err(err)?;
            Item::Collection(Collection {
                id,
                name: f.name,
                set: f.set,
                made: f.made,
                refs: f
                    .refs
                    .into_iter()
                    .map(|r| Reference {
                        hint: hint_from(folder, &r.hint),
                        ..r
                    })
                    .collect(),
                lineage: f.lineage,
            })
        }
        Some(Kind::Smart) => {
            let f: SmartFile = serde_json::from_slice(bytes).map_err(err)?;
            Item::Smart(Smart {
                id,
                name: f.name,
                set: f.set,
                made: f.made,
                filter: f.filter,
                root: f.root.map(|_| folder.to_path_buf()),
                lineage: f.lineage,
            })
        }
        Some(Kind::Stack) => {
            let f: StackFile = serde_json::from_slice(bytes).map_err(err)?;
            Item::Stack(Stack {
                id,
                made: f.made,
                open: false,
                refs: f
                    .refs
                    .into_iter()
                    .map(|r| Reference {
                        hint: hint_from(folder, &r.hint),
                        ..r
                    })
                    .collect(),
                lineage: f.lineage,
            })
        }
    };
    item.seed();
    Ok(Parsed::Item(item))
}

/// [`parse_reading`] by this build.
pub fn parse(folder: &Path, name: &str, bytes: &[u8]) -> Result<Parsed, String> {
    parse_reading(folder, name, bytes, collections::VERSION)
}

/// One copy's file as a pass found it: the folder it is carried in,
/// its name, and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub folder: PathBuf,
    pub name: String,
    pub bytes: Vec<u8>,
}

impl File {
    pub fn parse(&self) -> Result<Parsed, String> {
        parse(&self.folder, &self.name, &self.bytes)
    }
}

/// The names of the copies in `sets` (a folder's [`sets_dir`]), sorted.
/// A folder with none there is an error of kind `NotFound`.
pub fn list(sets: &Path) -> std::io::Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(sets)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| name_kind(n).is_some())
        .collect();
    names.sort();
    Ok(names)
}

/// A copy's file as a listing sees it: its size and its time.
pub type Seen = (u64, Option<std::time::SystemTime>);

/// The copy or deletion a part file was to become, by the part's name:
/// `<name>.<process>-<count>.part`, as the writes name them.
fn part_of(name: &str) -> Option<&str> {
    let (target, tag) = name.strip_suffix(PART_END)?.rsplit_once('.')?;
    let (process, count) = tag.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(process) && digits(count) && name_kind(target).is_some()).then_some(target)
}

/// Whether the file named `name` and last written at `at` is a leftover
/// to clear at `now`: a part file older than [`PART_STALE`], or a
/// deletion older than [`GONE_KEPT_FOR`]. A time to come is no age.
fn stale(name: &str, at: Option<SystemTime>, now: SystemTime) -> bool {
    let bound = if part_of(name).is_some() {
        PART_STALE
    } else if name.ends_with(GONE_END) && name_kind(name).is_some() {
        GONE_KEPT_FOR
    } else {
        return false;
    };
    at.and_then(|t| now.duration_since(t).ok())
        .is_some_and(|age| age > bound)
}

/// A sets' folder as a pass lists it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// The copies and deletions, each with its [`Seen`], sorted.
    pub copies: Vec<(String, Seen)>,
    /// The leftovers to clear, by name, sorted: part files a write left
    /// and deletions past their keep ([`stale`]).
    pub stale: Vec<String>,
}

/// The copies in `sets` (a folder's [`sets_dir`]) with their [`Seen`],
/// and the leftovers among its files, as of now.
pub fn listing(sets: &Path) -> std::io::Result<Listing> {
    listing_at(sets, SystemTime::now())
}

/// [`listing`] as of `now`.
pub fn listing_at(sets: &Path, now: SystemTime) -> std::io::Result<Listing> {
    let mut out = Listing::default();
    for e in std::fs::read_dir(sets)?.filter_map(|e| e.ok()) {
        let Some(meta) = e.metadata().ok().filter(|m| m.is_file()) else {
            continue;
        };
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        let at = meta.modified().ok();
        if stale(&name, at, now) {
            out.stale.push(name.clone());
        }
        if name_kind(&name).is_some() {
            out.copies.push((name, (meta.len(), at)));
        }
    }
    out.copies.sort();
    out.stale.sort();
    Ok(out)
}

/// What [`clear`] did.
#[derive(Debug, Default)]
pub struct Cleared {
    /// How many leftovers went.
    pub removed: usize,
    /// The first refusal, when one or more would not go: a read-only
    /// share, say. The rest were still tried.
    pub refused: Option<std::io::Error>,
}

/// The leftovers at `paths`, each in a sets' folder, cleared where each
/// is still one now: its time is asked again first, so a deletion
/// written again since the pass, or a part file of a write under way,
/// stays, and a deletion is read for its shape, a later build's left
/// alone. One that will not go leaves the rest to be tried, and the
/// sets' folder is tidied after.
pub fn clear(paths: &[PathBuf]) -> Cleared {
    let now = SystemTime::now();
    let mut done = Cleared::default();
    for p in paths {
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let tried = std::fs::metadata(p).and_then(|m| {
            if !stale(name, m.modified().ok(), now) || later_gone(p, name) {
                return Ok(false);
            }
            remove_quietly(p).map(|()| true)
        });
        match tried {
            Ok(true) => done.removed += 1,
            Ok(false) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                done.refused.get_or_insert(e);
            }
        }
    }
    let folder = paths.first().and_then(|p| p.parent()?.parent()?.parent());
    if let Some(folder) = folder {
        tidy(folder);
    }
    done
}

/// Whether the deletion's file at `path` is a later build's, or will
/// not say its shape: left alone, as every later build's file is.
fn later_gone(path: &Path, name: &str) -> bool {
    if part_of(name).is_some() {
        return false;
    }
    #[derive(Deserialize)]
    struct Shape {
        version: u32,
    }
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Shape>(&b).ok())
        .is_none_or(|s| s.version > GONE_VERSION)
}

/// Every copy in `folder`, read by this build; one that will not read
/// is said in the log and passed over.
pub fn read_folder(folder: &Path) -> Vec<File> {
    let sets = sets_dir(folder);
    let Ok(names) = list(&sets) else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter_map(|name| match std::fs::read(sets.join(&name)) {
            Ok(bytes) => Some(File {
                folder: folder.to_path_buf(),
                name,
                bytes,
            }),
            Err(e) => {
                log::warn!("carried set {}: {e}", sets.join(&name).display());
                None
            }
        })
        .collect()
}

/// What a write of a copy into a folder did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrote {
    /// Written: none was there, or the one there was behind.
    Written,
    /// The one there says the same.
    Same,
    /// The one there is ahead or went its own way: left for this
    /// machine to take when it reads the folder.
    Theirs,
    /// A later build's: left alone.
    Later,
    /// A deletion there has seen this one: left for this machine to
    /// take.
    Gone,
}

fn read_there(folder: &Path, name: &str, up_to: u32) -> Option<Result<Parsed, String>> {
    let bytes = std::fs::read(sets_dir(folder).join(name)).ok()?;
    Some(parse_reading(folder, name, &bytes, up_to))
}

fn remove_quietly(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// `item`'s copy written into `folder`, by a build that reads shapes up
/// to `up_to`. Never over a copy that is ahead, that went its own way,
/// or that a later build wrote, and never where a deletion has seen it;
/// a copy there that will not read is written over, the one beside the
/// library being the source. A folder that is not there is an error:
/// no folder is made for a copy, only the sets' folder in it.
pub fn put_reading(folder: &Path, item: &Item, up_to: u32) -> std::io::Result<Wrote> {
    put_as(folder, folder, item, up_to)
}

/// [`put_reading`] for the folder `home`'s frames: `home` is `folder`
/// itself, or the folder backed up to it, whose frames the copy names
/// by name alone, as they are named there.
pub fn put_as(folder: &Path, home: &Path, item: &Item, up_to: u32) -> std::io::Result<Wrote> {
    if !folder.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{}: no such folder", folder.display()),
        ));
    }
    let name = file_name(item.id(), item.kind());
    let gone = gone_name(item.id());
    match read_there(folder, &gone, up_to) {
        Some(Ok(Parsed::Gone(g))) if g.covers(item.lineage()) => return Ok(Wrote::Gone),
        Some(Ok(Parsed::Later)) => return Ok(Wrote::Later),
        _ => {}
    }
    match read_there(folder, &name, up_to) {
        Some(Ok(Parsed::Later)) => return Ok(Wrote::Later),
        Some(Ok(Parsed::Item(theirs))) => match compare(item.lineage(), theirs.lineage()) {
            Order::Same => return Ok(Wrote::Same),
            Order::Behind | Order::Diverged => return Ok(Wrote::Theirs),
            Order::Ahead => {}
        },
        Some(Ok(Parsed::Gone(_))) => {}
        Some(Err(e)) => log::warn!(
            "carried set {}: {e}; written over",
            sets_dir(folder).join(&name).display()
        ),
        None => {}
    }
    let sets = sets_dir(folder);
    write_atomic(&sets.join(&name), &to_text(item, home))?;
    remove_quietly(&sets.join(gone))?;
    Ok(Wrote::Written)
}

/// [`put_reading`] by this build.
pub fn put(folder: &Path, item: &Item) -> std::io::Result<Wrote> {
    put_reading(folder, item, collections::VERSION)
}

/// `item`'s copy taken out of `folder`, which holds none of its frames
/// now: only when it is the same as `item` or behind it. True when it
/// went.
pub fn take_out(folder: &Path, item: &Item) -> std::io::Result<bool> {
    let name = file_name(item.id(), item.kind());
    match read_there(folder, &name, collections::VERSION) {
        Some(Ok(Parsed::Item(theirs)))
            if matches!(
                compare(item.lineage(), theirs.lineage()),
                Order::Same | Order::Ahead
            ) =>
        {
            remove_quietly(&sets_dir(folder).join(name))?;
            tidy(folder);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The sets' folder in `folder` taken away when nothing is left in it,
/// and the sidecars' folder after it, so a folder emptied of frames
/// can still be removed.
fn tidy(folder: &Path) {
    let sets = sets_dir(folder);
    if std::fs::remove_dir(&sets).is_ok()
        && let Some(hidden) = sets.parent()
    {
        let _ = std::fs::remove_dir(hidden);
    }
}

/// A deletion carried into `folder`: the copy there taken when the
/// deletion has seen it, and the deletion's file left in its place. A
/// copy edited since, or a later build's, is left, and so is the
/// deletion's file then. A folder that is not there is an error.
pub fn bury(folder: &Path, gone: &Gone) -> std::io::Result<Wrote> {
    if !folder.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{}: no such folder", folder.display()),
        ));
    }
    let sets = sets_dir(folder);
    let name = file_name(&gone.id, gone.kind);
    if let Some(Ok(Parsed::Later)) = read_there(folder, &gone_name(&gone.id), collections::VERSION)
    {
        return Ok(Wrote::Later);
    }
    match read_there(folder, &name, collections::VERSION) {
        Some(Ok(Parsed::Later)) => return Ok(Wrote::Later),
        Some(Ok(Parsed::Item(theirs))) if !gone.covers(theirs.lineage()) => {
            return Ok(Wrote::Theirs);
        }
        _ => {}
    }
    let text = serde_json::to_string_pretty(&GoneFile {
        version: GONE_VERSION,
        kind: gone.kind,
        lineage: gone.lineage.clone(),
    })
    .map_err(std::io::Error::other)?;
    write_atomic(&sets.join(gone_name(&gone.id)), &text)?;
    remove_quietly(&sets.join(name))?;
    Ok(Wrote::Gone)
}

/// The folders `item`'s copies go to: each folder one of its frames is
/// in (a smart collection's own, or the top of every root for one over
/// all of them), and for each of those the folder it was backed up to
/// under an archive, by the roots' pairings. With roots to judge by,
/// a folder under none of them gets no copy: the sets' folder is not
/// left in a download folder the library does not hold.
pub fn folders_of(item: &Item, roots: Option<&Roots>) -> Vec<PathBuf> {
    targets_of(item, roots)
        .into_iter()
        .map(|t| t.folder)
        .collect()
}

/// A folder a copy goes to, and the folder whose frames it names by
/// name: the same folder, or for a backup's, the folder backed up.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Target {
    pub folder: PathBuf,
    pub home: PathBuf,
}

/// [`folders_of`], each with its home.
pub fn targets_of(item: &Item, roots: Option<&Roots>) -> Vec<Target> {
    fn add(out: &mut Vec<PathBuf>, p: PathBuf) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    let mut out: Vec<PathBuf> = Vec::new();
    match item {
        Item::Collection(Collection { refs, .. }) | Item::Stack(Stack { refs, .. }) => {
            for r in refs {
                if let Some(p) = Path::new(&r.hint).parent()
                    && p.is_absolute()
                {
                    add(&mut out, p.to_path_buf());
                }
            }
        }
        Item::Smart(s) => match &s.root {
            Some(r) => add(&mut out, r.clone()),
            None => {
                for r in roots.map(Roots::list).unwrap_or_default() {
                    add(&mut out, r.clone());
                }
            }
        },
    }
    if let Some(roots) = roots {
        out.retain(|f| roots.list().iter().any(|r| f.starts_with(r)));
    }
    let mut targets: Vec<Target> = out
        .iter()
        .map(|f| Target {
            folder: f.clone(),
            home: f.clone(),
        })
        .collect();
    if let Some(roots) = roots {
        for f in &out {
            for p in roots.backups() {
                if let Ok(rel) = f.strip_prefix(&p.source) {
                    let folder = p.dest.join(rel);
                    if !targets.iter().any(|t| t.folder == folder) {
                        targets.push(Target {
                            folder,
                            home: f.clone(),
                        });
                    }
                }
            }
        }
    }
    targets.sort();
    targets
}

/// What [`across`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Across {
    /// Copies and deletions written, on either side.
    pub written: usize,
    /// Pairs that went their own ways, left for the machines to join
    /// when they read them.
    pub apart: usize,
}

/// The copies carried between a folder and its backup, a run of Back up
/// or Bring back from `from` to `to`: each copy in `from` that `to`
/// lacks, or holds behind, goes across; one that `to` holds ahead comes
/// back; a deletion takes the copies it has seen on the other side. Two
/// that went their own ways are left, and a later build's. What is in
/// `to` alone stays there: it may be a copy `from` gave up. `beat` is
/// called after every file read or written: a job on a slow share is
/// told by it from one that stopped answering.
pub fn across(from: &Path, to: &Path, beat: &dyn Fn()) -> Across {
    let mut done = Across::default();
    if !to.is_dir() {
        return done;
    }
    let (fs, ts) = (sets_dir(from), sets_dir(to));
    let Ok(names) = list(&fs) else {
        return done;
    };
    beat();
    let read_there = |folder: &Path, name: &str, up_to: u32| {
        let read = read_there(folder, name, up_to);
        beat();
        read
    };
    let copy = |name: &str, a: &Path, b: &Path| -> bool {
        let read = std::fs::read_to_string(a.join(name));
        beat();
        let wrote = read.map(|text| write_atomic(&b.join(name), &text));
        beat();
        match wrote {
            Ok(Ok(())) => true,
            Ok(Err(e)) | Err(e) => {
                log::warn!("carried set {}: {e}", b.join(name).display());
                false
            }
        }
    };
    let up_to = collections::VERSION;
    let mut seen: HashSet<String> = HashSet::new();
    for name in names {
        let Some((id, _)) = name_kind(&name) else {
            continue;
        };
        let Some(Ok(mine)) = read_there(from, &name, up_to) else {
            continue;
        };
        match mine {
            Parsed::Later => {}
            Parsed::Item(ours) => {
                let gone_there = match read_there(to, &gone_name(id), up_to) {
                    Some(Ok(Parsed::Gone(g))) => Some(g),
                    // A later build's deletion over there: left, and
                    // nothing of the id carried beside it.
                    Some(Ok(Parsed::Later)) => continue,
                    _ => None,
                };
                if let Some(g) = gone_there.filter(|g| g.covers(ours.lineage())) {
                    // Deleted over there after it saw this one: the
                    // deletion comes back.
                    if copy(&gone_name(id), &ts, &fs) {
                        let _ = remove_quietly(&fs.join(&name));
                        done.written += 1;
                    }
                    seen.insert(g.id);
                    continue;
                }
                match read_there(to, &name, up_to) {
                    None | Some(Err(_)) => done.written += usize::from(copy(&name, &fs, &ts)),
                    Some(Ok(Parsed::Item(theirs))) => {
                        match compare(ours.lineage(), theirs.lineage()) {
                            Order::Same => {}
                            Order::Ahead => done.written += usize::from(copy(&name, &fs, &ts)),
                            Order::Behind => done.written += usize::from(copy(&name, &ts, &fs)),
                            Order::Diverged => done.apart += 1,
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
            Parsed::Gone(g) => {
                if seen.contains(&g.id) {
                    continue;
                }
                let item = file_name(&g.id, g.kind);
                if let Some(Ok(Parsed::Later)) = read_there(to, &name, up_to) {
                    continue;
                }
                match read_there(to, &item, up_to) {
                    Some(Ok(Parsed::Item(theirs))) if !g.covers(theirs.lineage()) => {}
                    Some(Ok(Parsed::Later)) => {}
                    _ => {
                        if copy(&name, &fs, &ts) {
                            let _ = remove_quietly(&ts.join(&item));
                            done.written += 1;
                        }
                    }
                }
            }
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Saved;

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-carry-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn r(hash: &str, hint: &Path) -> Reference {
        Reference {
            hash: hash.into(),
            hint: hint_of(hint),
            version: None,
        }
    }

    fn picks(shoot: &Path) -> Collection {
        let mut c = Collection::new("Clients/Picks");
        c.refs = vec![
            r("h1", &shoot.join("IMG_0001.CR3")),
            r("h2", &shoot.join("IMG_0002.CR3")),
        ];
        c.stamp(100, "laptop");
        c
    }

    #[test]
    fn a_revision_is_made_only_when_the_content_changes() {
        let dir = scratch("stamp");
        let mut c = picks(&dir);
        let first = c.lineage.current().to_string();
        assert!(!c.stamp(200, "laptop"), "nothing changed");
        c.refs[0].hint = "/elsewhere/IMG_0001.CR3".into();
        assert!(!c.stamp(200, "laptop"), "a hint is no revision");
        c.refs.reverse();
        assert!(c.stamp(200, "laptop"));
        assert_eq!(c.lineage.revs[0], first);
        assert_eq!(c.lineage.revs.len(), 2);
        for i in 0..KEPT as u64 {
            c.name = format!("n{i}");
            c.stamp(300 + i, "laptop");
        }
        assert_eq!(c.lineage.revs.len(), KEPT);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_same_change_on_two_machines_is_the_same_revision() {
        let dir = scratch("same-rev");
        let a = picks(&dir);
        let mut b = a.clone();
        let mut a = a;
        a.name = "Cover".into();
        b.name = "Cover".into();
        a.stamp(10, "laptop");
        b.stamp(99, "desk");
        assert_eq!(a.lineage.current(), b.lineage.current());
        assert_eq!(compare(&a.lineage, &b.lineage), Order::Same);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_copy_written_beside_its_frames_reads_back_whole_on_another_mount() {
        let laptop = scratch("laptop");
        let shoot = laptop.join("shoot");
        let other = laptop.join("other");
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let mut c = picks(&shoot);
        c.refs.push(r("h3", &other.join("IMG_0003.CR3")));
        c.stamp(150, "laptop");
        let item = Item::Collection(c.clone());
        assert_eq!(
            folders_of(&item, None),
            [other.clone(), shoot.clone()],
            "each folder a frame is in"
        );
        assert_eq!(put(&shoot, &item).unwrap(), Wrote::Written);
        assert_eq!(put(&shoot, &item).unwrap(), Wrote::Same);
        let text =
            std::fs::read_to_string(sets_dir(&shoot).join(file_name(&c.id, Kind::Collection)))
                .unwrap();
        assert!(text.contains("\"hint\": \"IMG_0001.CR3\""), "{text}");
        // The frame in another folder keeps its whole path, as JSON
        // writes it (its backslashes escaped on Windows).
        let whole = serde_json::to_string(&hint_of(&other.join("IMG_0003.CR3"))).unwrap();
        assert!(text.contains(&whole), "{text}");

        // The folder backed up, as a copy of it is on the desk.
        let desk = scratch("desk").join("archive-shoot");
        std::fs::create_dir_all(sets_dir(&desk)).unwrap();
        for f in read_folder(&shoot) {
            std::fs::write(sets_dir(&desk).join(&f.name), &f.bytes).unwrap();
        }
        let found = read_folder(&desk);
        assert_eq!(found.len(), 1);
        let Ok(Parsed::Item(Item::Collection(back))) = found[0].parse() else {
            panic!("{found:?}");
        };
        assert_eq!(back.id, c.id);
        assert_eq!(back.name, "Picks");
        assert_eq!(back.set, "Clients");
        assert_eq!(back.lineage, c.lineage);
        assert_eq!(back.refs[0].hint, hint_of(&desk.join("IMG_0001.CR3")));
        assert_eq!(back.refs[2].hint, hint_of(&other.join("IMG_0003.CR3")));
        greycard_core::testing::remove_dir_retry(&laptop);
        greycard_core::testing::remove_dir_retry(desk.parent().unwrap());
    }

    #[test]
    fn a_copy_ahead_or_apart_is_never_written_over() {
        let dir = scratch("never-over");
        let mut c = picks(&dir);
        let base = c.clone();
        c.refs.pop();
        c.stamp(200, "laptop");
        let ahead = Item::Collection(c);
        assert_eq!(put(&dir, &ahead).unwrap(), Wrote::Written);
        // An older copy of ours, and one that went its own way.
        assert_eq!(
            put(&dir, &Item::Collection(base.clone())).unwrap(),
            Wrote::Theirs
        );
        let mut apart = base;
        apart.name = "Other".into();
        apart.stamp(300, "desk");
        assert_eq!(put(&dir, &Item::Collection(apart)).unwrap(), Wrote::Theirs);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_stack_is_carried_by_its_names_and_keeps_its_open_at_home() {
        let laptop = scratch("stack");
        let shoot = laptop.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let mut s = Stack::new(vec![
            r("h2", &shoot.join("IMG_0002.CR3")),
            r("h1", &shoot.join("IMG_0001.CR3")),
        ]);
        s.open = true;
        s.stamp(10, "laptop");
        let open_sum = s.lineage.sum.clone();
        s.open = false;
        assert!(!s.stamp(11, "laptop"), "open is no revision");
        assert_eq!(s.lineage.sum, open_sum);
        s.open = true;
        let item = Item::Stack(s.clone());
        assert_eq!(folders_of(&item, None), std::slice::from_ref(&shoot));
        assert_eq!(put(&shoot, &item).unwrap(), Wrote::Written);
        let desk = laptop.join("desk-copy");
        std::fs::create_dir_all(&desk).unwrap();
        assert_eq!(
            across(&shoot, &desk, &|| {}),
            Across {
                written: 1,
                apart: 0
            }
        );
        let found = read_folder(&desk);
        let Ok(Parsed::Item(Item::Stack(back))) = found[0].parse() else {
            panic!("{found:?}");
        };
        assert!(!back.open, "a stack arrives closed");
        assert_eq!(back.refs[0].hint, hint_of(&desk.join("IMG_0002.CR3")));
        assert_eq!(back.lineage, s.lineage);
        // A rename of a member is a revision: a stack finds frames by name.
        s.refs[1].hint = hint_of(&shoot.join("IMG_0001-renamed.CR3"));
        assert!(s.stamp(12, "laptop"));
        greycard_core::testing::remove_dir_retry(&laptop);
    }

    #[test]
    fn a_conflict_keeps_both_and_both_machines_make_the_same_two() {
        let dir = scratch("conflict");
        let base = picks(&dir);
        let mut laptop = base.clone();
        laptop.refs.pop();
        laptop.stamp(200, "laptop");
        let mut desk = base.clone();
        desk.name = "Selects".into();
        desk.stamp(300, "desk");
        let (l, d) = (Item::Collection(laptop), Item::Collection(desk));
        let on_laptop = join(&l, &d);
        let on_desk = join(&d, &l);
        // The desk's is the later and keeps the id; the laptop's beside.
        let Some(Item::Collection(keep)) = &on_laptop.keep else {
            panic!("{on_laptop:?}");
        };
        assert_eq!(keep.name, "Selects");
        assert_eq!(keep.refs.len(), 2);
        let Some(Item::Collection(copy)) = &on_laptop.copy else {
            panic!("{on_laptop:?}");
        };
        assert_eq!(copy.name, "Picks (on laptop)");
        assert_eq!(copy.refs.len(), 1);
        assert_ne!(copy.id, keep.id);
        // The desk makes the same two: its own stands, having seen the
        // laptop's, and the same copy beside it.
        let Some(Item::Collection(desk_keep)) = &on_desk.keep else {
            panic!("{on_desk:?}");
        };
        assert_eq!(desk_keep.lineage.current(), keep.lineage.current());
        assert_eq!(on_desk.copy, on_laptop.copy);
        // Joined again, each is the same: nothing more is made.
        let k = on_laptop.keep.clone().unwrap();
        let again = join(&k, &on_desk.keep.clone().unwrap());
        assert!(again.copy.is_none());
        assert!(
            join(&l, &k).copy.is_none(),
            "the laptop's own is behind the join"
        );
        assert_eq!(compare(l.lineage(), k.lineage()), Order::Behind);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_stack_taken_keeps_our_folder_and_their_names() {
        let ours_dir = PathBuf::from("/desk/shoot");
        let theirs_dir = PathBuf::from("/archive/shoot");
        let mk = |d: &Path, second: &str| {
            Stack::new(vec![r("h1", &d.join("a.CR3")), r("h2", &d.join(second))])
        };
        let mut ours = mk(&ours_dir, "b.CR3");
        ours.seed();
        let mut theirs = ours.clone();
        theirs.refs = mk(&theirs_dir, "b-renamed.CR3").refs;
        theirs.stamp(5, "laptop");
        let Some(Item::Stack(keep)) = join(&Item::Stack(ours), &Item::Stack(theirs.clone())).keep
        else {
            panic!()
        };
        assert_eq!(keep.refs[1].hint, hint_of(&ours_dir.join("b-renamed.CR3")));
        let mut again = keep.clone();
        assert!(!again.stamp(9, "desk"), "no revision of its own: {keep:?}");
        assert_eq!(again.lineage, theirs.lineage);
    }

    #[test]
    fn a_stack_conflict_keeps_the_later_alone() {
        let dir = scratch("stack-conflict");
        let base = Stack::new(vec![
            r("h1", &dir.join("a.CR3")),
            r("h2", &dir.join("b.CR3")),
        ]);
        let (mut a, mut b) = (base.clone(), base);
        a.refs.reverse();
        a.stamp(5, "laptop");
        b.refs.push(r("h3", &dir.join("c.CR3")));
        b.stamp(9, "desk");
        let j = join(&Item::Stack(a), &Item::Stack(b.clone()));
        assert!(j.copy.is_none());
        let Some(Item::Stack(keep)) = j.keep else {
            panic!()
        };
        assert_eq!(keep.refs, b.refs);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_deletion_takes_what_it_saw_and_not_an_edit_made_since() {
        let dir = scratch("gone");
        let c = picks(&dir);
        let item = Item::Collection(c.clone());
        put(&dir, &item).unwrap();
        assert_eq!(bury(&dir, &Gone::of(&item)).unwrap(), Wrote::Gone);
        let found = read_folder(&dir);
        assert_eq!(found.len(), 1);
        assert!(matches!(found[0].parse(), Ok(Parsed::Gone(_))));
        assert_eq!(
            put(&dir, &item).unwrap(),
            Wrote::Gone,
            "a copy it saw stays gone"
        );
        let mut edited = c;
        edited.name = "Edited since".into();
        edited.stamp(500, "desk");
        let edited = Item::Collection(edited);
        assert_eq!(put(&dir, &edited).unwrap(), Wrote::Written);
        let names: Vec<String> = read_folder(&dir).into_iter().map(|f| f.name).collect();
        assert_eq!(names, [file_name(edited.id(), Kind::Collection)]);
        assert_eq!(bury(&dir, &Gone::of(&item)).unwrap(), Wrote::Theirs);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_later_shape_is_left_alone_by_an_older_build() {
        let dir = scratch("later");
        let mut filter = Saved {
            stars: 3,
            ..Saved::default()
        };
        filter.not_labels.push(greycard_edit::meta::Label::Red);
        let mut s = Smart::new("Held out", filter, Some(dir.clone()));
        s.stamp(10, "laptop");
        let item = Item::Smart(s.clone());
        assert_eq!(folders_of(&item, None), std::slice::from_ref(&dir));
        assert_eq!(put(&dir, &item).unwrap(), Wrote::Written);
        let name = file_name(&s.id, Kind::Smart);
        let text = std::fs::read_to_string(sets_dir(&dir).join(&name)).unwrap();
        assert!(text.contains("\"version\": 3"), "{text}");
        assert!(text.contains("\"root\": \".\""), "{text}");
        // A build that reads up to shape 2 leaves it: not taken, and not
        // written over with what it holds of the same id.
        let bytes = text.as_bytes();
        assert_eq!(parse_reading(&dir, &name, bytes, 2).unwrap(), Parsed::Later);
        let mut older = s.clone();
        older.filter.not_labels.clear();
        older.name = "Mine".into();
        older.stamp(20, "desk");
        let older = Item::Smart(older);
        assert_eq!(put_reading(&dir, &older, 2).unwrap(), Wrote::Later);
        assert_eq!(
            std::fs::read_to_string(sets_dir(&dir).join(&name)).unwrap(),
            text
        );
        // This build reads it whole, its folder this one.
        let Ok(Parsed::Item(Item::Smart(back))) = parse(&dir, &name, bytes) else {
            panic!()
        };
        assert_eq!(back.root.as_deref(), Some(dir.as_path()));
        assert_eq!(back.filter, s.filter);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_file_beside_the_library_reads_in_a_build_that_keeps_no_lineage() {
        // The shape a build before the carry reads: the same fields, no
        // lineage, unknown keys passed over.
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct Before {
            version: u32,
            name: String,
            #[serde(default)]
            set: String,
            #[serde(default)]
            made: u64,
            #[serde(default)]
            refs: Vec<Reference>,
        }
        let dir = scratch("before");
        let c = picks(&dir);
        c.write(&dir).unwrap();
        let text = std::fs::read_to_string(collections::file_of(&dir, &c.id, false)).unwrap();
        assert!(text.contains("\"lineage\""));
        let before: Before = serde_json::from_str(&text).unwrap();
        assert_eq!(before.version, 1);
        assert_eq!(before.refs, c.refs);
        // And one that build wrote, read here, is given the same first
        // revision on every machine.
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v.as_object_mut().unwrap().remove("lineage");
        std::fs::write(
            collections::file_of(&dir, &c.id, false),
            serde_json::to_string(&v).unwrap(),
        )
        .unwrap();
        let read = collections::read_folder(&dir, collections::VERSION);
        let mut fresh = c.clone();
        fresh.lineage = Lineage::default();
        fresh.seed();
        assert_eq!(read.list[0].lineage.revs, fresh.lineage.revs);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn across_carries_the_newer_either_way_and_leaves_a_pair_apart() {
        let root = scratch("across");
        let (here, there) = (root.join("here"), root.join("there"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let c = picks(&here);
        put(&here, &Item::Collection(c.clone())).unwrap();
        assert_eq!(across(&here, &there, &|| {}).written, 1);
        assert_eq!(
            across(&here, &there, &|| {}),
            Across::default(),
            "the same now"
        );
        // Edited over there: it comes back.
        let mut theirs = c.clone();
        theirs.name = "Renamed there".into();
        theirs.stamp(400, "desk");
        let theirs_item = Item::Collection(theirs);
        assert_eq!(put(&there, &theirs_item).unwrap(), Wrote::Written);
        assert_eq!(across(&here, &there, &|| {}).written, 1);
        let Ok(Parsed::Item(Item::Collection(back))) = read_folder(&here)[0].parse() else {
            panic!()
        };
        assert_eq!(back.name, "Renamed there");
        // Both edited: left for the machines to join.
        let mut a = back.clone();
        a.name = "A".into();
        a.stamp(500, "laptop");
        let mut b = back;
        b.name = "B".into();
        b.stamp(501, "desk");
        std::fs::remove_file(sets_dir(&here).join(file_name(&c.id, Kind::Collection))).unwrap();
        put(&here, &Item::Collection(a)).unwrap();
        std::fs::remove_file(sets_dir(&there).join(file_name(&c.id, Kind::Collection))).unwrap();
        put(&there, &Item::Collection(b)).unwrap();
        assert_eq!(
            across(&here, &there, &|| {}),
            Across {
                written: 0,
                apart: 1
            }
        );
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// A folder of sets on a slow share says it is moving at every file:
    /// each one read on either side and each one written beats, not the
    /// folder once.
    #[test]
    fn across_beats_at_every_file() {
        let root = scratch("across-beats");
        let (here, there) = (root.join("here"), root.join("there"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let n = 6;
        for i in 0..n {
            let mut c = Collection::new(&format!("Set {i}"));
            c.stamp(100 + i as u64, "laptop");
            put(&here, &Item::Collection(c)).unwrap();
        }
        let beats = std::cell::Cell::new(0usize);
        let done = across(&here, &there, &|| beats.set(beats.get() + 1));
        assert_eq!(done.written, n);
        // Each: ours read, theirs looked for (none), ours read again
        // and written there.
        assert!(beats.get() > 3 * n, "{} beats", beats.get());
        let before = beats.get();
        across(&here, &there, &|| beats.set(beats.get() + 1));
        assert!(beats.get() - before >= 2 * n, "the same: still read");
        greycard_core::testing::remove_dir_retry(&root);
    }

    #[test]
    fn a_copy_left_behind_by_many_saves_is_still_behind() {
        let d = scratch("cap");
        let mut c = picks(&d);
        let left = c.clone();
        for i in 0..200 {
            c.refs
                .push(r(&format!("x{i}"), &d.join(format!("x{i}.CR3"))));
            c.stamp(200 + i, "laptop");
        }
        assert_eq!(compare(&c.lineage, &left.lineage), Order::Ahead);
        assert!(
            join(&Item::Collection(c), &Item::Collection(left))
                .copy
                .is_none()
        );
        greycard_core::testing::remove_dir_retry(&d);
    }

    /// A later build's copy, or its deletion, whose shape ours cannot
    /// read at all: put, bury, and across leave each as it is.
    #[test]
    fn a_later_shape_that_does_not_fit_is_left_alone_everywhere() {
        let d = scratch("later-unfit");
        let other = d.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let c = picks(&d);
        let file = sets_dir(&d).join(file_name(&c.id, Kind::Collection));
        let gone = sets_dir(&d).join(gone_name(&c.id));
        std::fs::create_dir_all(sets_dir(&d)).unwrap();
        let later = r#"{"version": 9, "name": "Later", "refs": {"shape": "new"}}"#;
        let later_gone = r#"{"version": 2, "kind": "something new", "extra": 1}"#;
        let mut ours = c.clone();
        ours.name = "Mine".into();
        ours.stamp(5, "desk");
        let ours = Item::Collection(ours);
        std::fs::write(&file, later).unwrap();
        assert_eq!(put(&d, &ours).unwrap(), Wrote::Later);
        assert_eq!(bury(&d, &Gone::of(&ours)).unwrap(), Wrote::Later);
        bury(&other, &Gone::of(&Item::Collection(c.clone()))).unwrap();
        across(&other, &d, &|| {});
        assert_eq!(std::fs::read_to_string(&file).unwrap(), later);
        std::fs::remove_file(&file).unwrap();
        std::fs::write(&gone, later_gone).unwrap();
        assert_eq!(put(&d, &ours).unwrap(), Wrote::Later);
        assert_eq!(bury(&d, &Gone::of(&ours)).unwrap(), Wrote::Later);
        across(&other, &d, &|| {});
        assert_eq!(std::fs::read_to_string(&gone).unwrap(), later_gone);
        assert!(!file.exists());
        greycard_core::testing::remove_dir_retry(&d);
    }

    fn aged(path: &Path, by: Duration) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - by)
            .unwrap();
    }

    /// A part file older than a day and a deletion older than its keep
    /// are leftovers; a fresh one of either, a copy, and a file that is
    /// no part of a copy's are not. Cleared, each is asked its age
    /// again: a deletion written again since the pass stays, and so does
    /// a later build's deletion, however old.
    #[test]
    fn leftovers_are_told_by_their_age_and_cleared_while_they_still_are() {
        let dir = scratch("leftovers");
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let mut s = picks(&shoot);
        s.stamp(1, "laptop");
        let item = Item::Collection(s.clone());
        put(&shoot, &item).unwrap();
        let mut old = Collection::new("Old");
        old.stamp(1, "laptop");
        let old_gone = Gone::of(&Item::Collection(old.clone()));
        bury(&shoot, &old_gone).unwrap();
        let mut young = Collection::new("Young");
        young.stamp(1, "laptop");
        bury(&shoot, &Gone::of(&Item::Collection(young.clone()))).unwrap();
        let sets = sets_dir(&shoot);
        let names = [
            format!("{}.json.77-0.part", s.id),
            format!("{}.json.77-1.part", s.id),
            format!("{}.gone.json.77-2.part", old.id),
            "notes.txt.77-3.part".to_string(),
            format!("{}.json.part", s.id),
        ];
        for n in &names {
            std::fs::write(sets.join(n), "{").unwrap();
        }
        let day = Duration::from_secs(24 * 60 * 60);
        for n in [&names[0], &names[2], &names[3], &names[4]] {
            aged(&sets.join(n), 2 * day);
        }
        aged(&sets.join(gone_name(&old.id)), GONE_KEPT_FOR + day);
        aged(&sets.join(gone_name(&young.id)), GONE_KEPT_FOR - day);
        let later = gone_name("later0");
        std::fs::write(sets.join(&later), r#"{"version": 99, "what": "else"}"#).unwrap();
        aged(&sets.join(&later), GONE_KEPT_FOR + day);
        let listed = listing(&sets).unwrap();
        let mut want = vec![
            names[0].clone(),
            names[2].clone(),
            gone_name(&old.id),
            later.clone(),
        ];
        want.sort();
        assert_eq!(listed.stale, want);
        assert_eq!(listed.copies.len(), 4, "{:?}", listed.copies);
        // The old deletion written again since the pass: it stays.
        bury(&shoot, &old_gone).unwrap();
        let paths: Vec<PathBuf> = listed.stale.iter().map(|n| sets.join(n)).collect();
        let cleared = clear(&paths);
        assert_eq!(cleared.removed, 2);
        assert!(cleared.refused.is_none());
        let mut left: Vec<String> = std::fs::read_dir(&sets)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        let mut kept = vec![
            file_name(&s.id, Kind::Collection),
            gone_name(&old.id),
            gone_name(&young.id),
            later,
            names[1].clone(),
            names[3].clone(),
            names[4].clone(),
        ];
        kept.sort();
        assert_eq!(left, kept);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The sets' folder emptied by a clear goes, and the sidecars'
    /// folder with it when nothing else is there.
    #[test]
    fn a_clear_that_empties_the_sets_folder_takes_it_away() {
        let dir = scratch("leftovers-tidy");
        let mut c = Collection::new("Gone");
        c.stamp(1, "laptop");
        bury(&dir, &Gone::of(&Item::Collection(c.clone()))).unwrap();
        let gone = sets_dir(&dir).join(gone_name(&c.id));
        aged(&gone, GONE_KEPT_FOR * 2);
        assert_eq!(clear(&[gone]).removed, 1);
        assert!(!dir.join(greycard_edit::SIDECAR_FOLDER).exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_copy_in_a_backup_names_its_frames_by_name() {
        // Canonical, as the roots keep what they are given: the temp
        // folder sits behind a symlink on macOS and takes the verbatim
        // prefix on Windows.
        let d = crate::canonical(&scratch("dest")).unwrap();
        let (local, archive) = (d.join("local"), d.join("archive"));
        let src = local.join("shoot");
        let dest = archive.join("2026").join("shoot");
        let loose = d.join("downloads");
        for p in [&src, &dest, &loose] {
            std::fs::create_dir_all(p).unwrap();
        }
        let mut roots = Roots::default();
        roots.add(&local).unwrap();
        roots.add(&archive).unwrap();
        assert!(roots.set_archive(&archive, true));
        assert!(roots.set_backup_folder(&src, &archive, &dest));
        let mut s = Stack::new(vec![
            r("h1", &src.join("a.CR3")),
            r("h2", &src.join("b.CR3")),
            r("h3", &loose.join("c.CR3")),
        ]);
        s.stamp(1, "laptop");
        let item = Item::Stack(s.clone());
        let targets = targets_of(&item, Some(&roots));
        assert_eq!(
            targets.iter().map(|t| &t.folder).collect::<Vec<_>>(),
            [&dest, &src],
            "the frames' folder and its backup, never one under no root"
        );
        for t in &targets {
            put_as(&t.folder, &t.home, &item, collections::VERSION).unwrap();
        }
        assert!(!sets_dir(&loose).exists());
        // The desk mounts the archive elsewhere: the frames are its own.
        let desk = d.join("desk-mount");
        std::fs::create_dir_all(&desk).unwrap();
        let found = read_folder(&dest);
        let Ok(Parsed::Item(Item::Stack(back))) = parse(&desk, &found[0].name, &found[0].bytes)
        else {
            panic!("{found:?}")
        };
        assert_eq!(back.refs[0].hint, hint_of(&desk.join("a.CR3")));
        assert_eq!(back.refs[1].hint, hint_of(&desk.join("b.CR3")));
        greycard_core::testing::remove_dir_retry(&d);
    }
}
