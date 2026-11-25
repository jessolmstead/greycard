//! The settle on the worker's thread: what takes its place, what it
//! waits for, and what it gives way to, on the device (lavapipe in CI).

use super::seen::{Seen, part_of};
use super::tests::skipped;
use super::*;
use greycard_core::develop::sharpen;

const W: usize = 640;
const H: usize = 480;

const CLOSE: Seen = Seen {
    rect: [0.3, 0.3, 0.6, 0.6],
    zoom: 1.0,
};

/// The top left corner at 1:1: some of its tiles are none of
/// [`CLOSE`]'s.
const PANNED: Seen = Seen {
    rect: [0.0, 0.0, 0.2, 0.2],
    zoom: 1.0,
};

const FIT: Seen = Seen {
    rect: [0.0, 0.0, 1.0, 1.0],
    zoom: 0.25,
};

/// What the worker said, as the tests hear it.
#[derive(Debug)]
enum Said {
    Developed {
        generation: u64,
        settles: bool,
        iterations: usize,
        texture: Option<greycard_gpu::wgpu::Texture>,
    },
    Kept,
}

impl Said {
    fn developed(&self) -> Option<(u64, bool, usize)> {
        match self {
            Said::Developed {
                generation,
                settles,
                iterations,
                ..
            } => Some((*generation, *settles, *iterations)),
            Said::Kept => None,
        }
    }
}

/// A worker with a device of its own on the test's (whose textures the
/// test reads back), a cache to keep the edit's pictures in, and two
/// frames of our own writing; `hold` blocks the worker in its first
/// settling delivery until the test lets it go.
struct Rig {
    worker: Worker,
    heard: std::sync::mpsc::Receiver<Said>,
    ctx: greycard_gpu::Context,
    dir: std::path::PathBuf,
    a: std::path::PathBuf,
    b: std::path::PathBuf,
    go: Option<std::sync::mpsc::Sender<()>>,
}

impl Rig {
    fn new(seen: Option<Seen>, body: &str, hold: bool) -> Option<Self> {
        let ctx = greycard_gpu::Context::own().ok()?;
        let dir = crate::testing::scratch_dir("settle");
        let (a, b) = (dir.join("a.dng"), dir.join("b.dng"));
        crate::testing::linear_dng(&a, W, H, "Test", body);
        crate::testing::linear_dng(&b, W, H, "Test", "Other");
        let (tx, heard) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let tx = Mutex::new(tx);
        let wait = Mutex::new(hold.then_some(wait));
        let worker = Worker::new(move |o| {
            let said = match o {
                Outcome::Developed {
                    generation,
                    sharpen,
                    settles,
                    image,
                    ..
                } => Said::Developed {
                    generation,
                    settles,
                    iterations: sharpen.map_or(0, |s| s.iterations),
                    texture: match image {
                        Developed::Texture(t) => Some(t),
                        Developed::Halves(_) => None,
                    },
                },
                Outcome::EditedKept { .. } => Said::Kept,
                _ => return,
            };
            let settling = matches!(said, Said::Developed { settles: true, .. });
            let _ = tx.lock().unwrap().send(said);
            if settling && let Some(wait) = wait.lock().unwrap().take() {
                let _ = wait.recv();
            }
        });
        worker.set_gpu(ctx.device(), ctx.queue());
        worker.set_thumb_cache(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(
                crate::edited_pictures::BIG,
                greycard_library::thumbs::DEFAULT_PREVIEW_CAP,
            ),
        ));
        let rig = Rig {
            worker,
            heard,
            ctx,
            dir,
            a,
            b,
            go: Some(go),
        };
        rig.worker.send(open(&rig.a, 1));
        // Told once the frame is drawn, as the window tells it: an
        // open forgets what was shown before it.
        rig.worker.set_seen(seen);
        assert_eq!(rig.next().developed(), Some((1, false, 20)));
        Some(rig)
    }

    fn next(&self) -> Said {
        self.heard
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the worker answers")
    }

    /// Nothing more said for a second.
    fn quiet(&self) {
        let said = self.heard.recv_timeout(std::time::Duration::from_secs(1));
        assert!(said.is_err(), "{said:?}");
    }

    /// The worker held in its first settling delivery goes on.
    fn release(&mut self) {
        self.go.take();
    }

    fn rows(&self, said: &Said) -> Vec<u8> {
        let Said::Developed {
            texture: Some(t), ..
        } = said
        else {
            panic!("a texture: {said:?}");
        };
        self.ctx
            .read_viewport_rows(t, 0, t.height())
            .expect("reads back")
    }

    fn save(&self, edit: &Edit) {
        self.worker.send(Job::Keep {
            frame: Frame::file(self.a.clone()),
            edit: edit.clone(),
            turn: 0,
            iso: None,
        });
    }

    /// The 2048 picture kept of `edit` of the first frame.
    fn kept(&self, edit: &Edit) -> Option<Vec<u8>> {
        let edited = self.worker.edited();
        let hash = greycard_library::hash_file(&self.a).unwrap();
        let key = edited.key_of(&self.a, edit);
        edited
            .lookup(
                &hash,
                crate::edited_pictures::BIG,
                crate::edited_pictures::Shows::Edit(key),
            )
            .map(|p| p.rgb)
    }

    fn end(self) {
        self.worker.stop_all(std::time::Duration::from_secs(60));
        crate::testing::remove_dir_retry(&self.dir);
    }
}

fn open(path: &std::path::Path, generation: u64) -> Job {
    Job::Open {
        path: path.to_path_buf(),
        version: None,
        edit: Edit::default(),
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    }
}

fn clarity(amount: f32) -> Edit {
    let mut edit = Edit::default();
    edit.detail.clarity = amount;
    edit.sharpen.auto_radius = false;
    edit
}

fn develop(edit: &Edit, generation: u64) -> Job {
    Job::Develop {
        edit: edit.clone(),
        generation,
        turn: 0,
        of: None,
    }
}

/// Whether the pixel at `x`, `y` is in a tile `seen`'s part touches.
fn touched(seen: &Seen, x: usize, y: usize) -> bool {
    let part = part_of(seen, (W, H)).expect("a part");
    let tile = sharpen::tile_for(H);
    let (cols, bands) = part.tiles(W, H, tile);
    cols.contains(&(x / tile)) && bands.contains(&(y / tile))
}

/// Of the pixels in `seen`'s tiles, and of the rest, how many differ
/// between `a` and `b`.
fn apart(a: &[u8], b: &[u8], seen: &Seen) -> (usize, usize) {
    let (mut inside, mut outside) = (0, 0);
    for y in 0..H {
        for x in 0..W {
            let at = (y * W + x) * 8..(y * W + x + 1) * 8;
            if a[at.clone()] != b[at] {
                if touched(seen, x, y) {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
    }
    (inside, outside)
}

/// Each develop of the window says whether a settle follows: the first
/// round seen at a fit view does, the settle and an open do not.
#[test]
fn a_develop_says_whether_a_settle_follows() {
    let Some(rig) = Rig::new(Some(FIT), "Cam", false) else {
        skipped("the settle's flag");
        return;
    };
    rig.worker.send(develop(&clarity(0.4), 2));
    let first = rig.next().developed().unwrap();
    assert!(first.1 && first.2 < 20, "{first:?}");
    assert_eq!(rig.next().developed(), Some((2, false, 20)));
    rig.quiet();
    rig.end();
}

/// The Original's develop aside takes the settle's place, as a newer
/// develop does: the settle gives way to it and is not made after it
/// (the window develops the edit again when the Original goes, since a
/// first round is not put back).
#[test]
fn an_aside_takes_the_settles_place() {
    let Some(rig) = Rig::new(Some(FIT), "Cam", false) else {
        skipped("the settle and the aside");
        return;
    };
    rig.worker.queue.0.lock().unwrap().develop_on_fill_ask = Some(Job::DevelopAside {
        edit: Edit::default(),
        generation: 3,
        turn: 0,
        of: None,
    });
    rig.worker.send(develop(&clarity(0.4), 2));
    let first = rig.next().developed().unwrap();
    assert_eq!((first.0, first.1), (2, true));
    let aside = rig.next().developed().unwrap();
    assert_eq!((aside.0, aside.1), (3, false));
    rig.quiet();
    rig.end();
}

/// A develop of another file (which makes nothing) leaves the settle
/// and the picture on screen to the frame in hand: the settle still
/// lands.
#[test]
fn a_develop_of_another_file_leaves_the_settle() {
    let Some(rig) = Rig::new(Some(FIT), "Cam", false) else {
        skipped("the settle and another file");
        return;
    };
    rig.worker.queue.0.lock().unwrap().develop_on_fill_ask = Some(Job::Develop {
        edit: Edit::default(),
        generation: 3,
        turn: 0,
        of: Some((rig.b.clone(), None)),
    });
    rig.worker.send(develop(&clarity(0.4), 2));
    let first = rig.next().developed().unwrap();
    assert_eq!((first.0, first.1), (2, true));
    assert_eq!(rig.next().developed(), Some((2, false, 20)));
    rig.quiet();
    rig.end();
}

/// The view panned past what the first round sharpened before the
/// settle: a first round of the tiles seen now, delivered at once
/// under the same generation, then the settle. Each first round's
/// tiles are the whole develop's to the bit; the panned-to tiles were
/// not in the first.
#[test]
fn a_pan_before_the_settle_sharpens_what_is_seen_now() {
    let Some(rig) = Rig::new(Some(CLOSE), "Cam", false) else {
        skipped("a pan before the settle");
        return;
    };
    rig.worker.queue.0.lock().unwrap().seen_on_settle_ask = Some(PANNED);
    rig.worker.send(develop(&clarity(0.4), 2));
    let first = rig.next();
    let again = rig.next();
    let settled = rig.next();
    assert_eq!(first.developed(), Some((2, true, 20)));
    assert_eq!(again.developed(), Some((2, true, 20)));
    assert_eq!(settled.developed(), Some((2, false, 20)));
    rig.quiet();
    let (first, again, settled) = (rig.rows(&first), rig.rows(&again), rig.rows(&settled));
    let (inside, outside) = apart(&first, &settled, &CLOSE);
    assert!(inside == 0 && outside > 0, "{inside} {outside}");
    let (inside, outside) = apart(&again, &settled, &PANNED);
    assert!(inside == 0 && outside > 0, "{inside} {outside}");
    let (inside, _) = apart(&first, &settled, &PANNED);
    assert!(inside > 0, "the first round left the panned-to tiles");
    rig.end();
}

/// A view that moves within what the first round sharpened asks for
/// nothing more before the settle.
#[test]
fn a_view_inside_the_first_rounds_tiles_waits_for_the_settle() {
    let Some(rig) = Rig::new(Some(CLOSE), "Cam", false) else {
        skipped("a view inside the first round");
        return;
    };
    rig.worker.queue.0.lock().unwrap().seen_on_settle_ask = Some(Seen {
        rect: [0.35, 0.35, 0.55, 0.55],
        zoom: 2.0,
    });
    rig.worker.send(develop(&clarity(0.4), 2));
    assert_eq!(rig.next().developed(), Some((2, true, 20)));
    assert_eq!(rig.next().developed(), Some((2, false, 20)));
    rig.quiet();
    rig.end();
}

/// How a save of the first frame's edit meets the frame left or the
/// editor closing between the rounds.
#[derive(Clone, Copy, Debug)]
enum Leaving {
    /// No view: the whole develop in the first round.
    Not,
    /// The save sent with the develop; the next frame opened before the
    /// settle starts.
    Before,
    /// The save sent with the develop; the next frame opened as the
    /// settle asks its hold first.
    During,
    /// The save and the open sent together once the first round has
    /// landed, as the window sends them on a switch.
    Together,
    /// The save sent with the develop; the Original shown before the
    /// settle starts, and the next frame opened while it is up.
    Original,
    /// The save sent with the develop; the editor closing before the
    /// settle starts.
    Closing,
    /// As `Closing`, the view panned past the first round's tiles as
    /// the editor closes.
    ClosingPanned,
    /// As `Closing`, a develop of a newer edit asked as the editor
    /// closes, never to be picked.
    ClosingNewer,
}

/// The edit's 2048 picture kept of a frame left between the rounds.
fn kept_on_leaving(leaving: Leaving, body: &str) -> Option<Vec<u8>> {
    let seen = (!matches!(leaving, Leaving::Not)).then_some(CLOSE);
    let held = matches!(
        leaving,
        Leaving::Before
            | Leaving::Closing
            | Leaving::ClosingPanned
            | Leaving::ClosingNewer
            | Leaving::Original
    );
    let mut rig = Rig::new(seen, body, held)?;
    let edit = clarity(0.4);
    if matches!(leaving, Leaving::During) {
        rig.worker.queue.0.lock().unwrap().develop_on_fill_ask = Some(open(&rig.b, 3));
    }
    if !matches!(leaving, Leaving::Together) {
        rig.save(&edit);
    }
    rig.worker.send(develop(&edit, 2));
    let first = rig.next().developed().unwrap();
    assert_eq!(first.0, 2, "{leaving:?}");
    let mut opening = 3;
    match leaving {
        Leaving::Not => rig.worker.send(open(&rig.b, 3)),
        // Sent while the worker is held in the first round's delivery:
        // the open is in the queue before the settle is.
        Leaving::Before => {
            rig.worker.send(open(&rig.b, 3));
            rig.release();
        }
        Leaving::Together => {
            rig.save(&edit);
            rig.worker.send(open(&rig.b, 3));
        }
        Leaving::Original => {
            rig.worker.send(Job::DevelopAside {
                edit: Edit::default(),
                generation: 3,
                turn: 0,
                of: None,
            });
            rig.release();
            assert_eq!(rig.next().developed().map(|d| d.0), Some(3), "the Original");
            opening = 4;
            rig.worker.send(open(&rig.b, 4));
        }
        Leaving::Closing => {
            rig.worker.queue.0.lock().unwrap().stopping = true;
            rig.release();
        }
        Leaving::ClosingPanned => {
            let mut q = rig.worker.queue.0.lock().unwrap();
            q.seen = Some(PANNED);
            q.stopping = true;
            drop(q);
            rig.release();
        }
        Leaving::ClosingNewer => {
            let mut q = rig.worker.queue.0.lock().unwrap();
            q.develop = Some(develop(&clarity(0.6), 3));
            q.stopping = true;
            drop(q);
            rig.release();
        }
        Leaving::During => {}
    }
    let mut kept = false;
    let mut opened = matches!(
        leaving,
        Leaving::Closing | Leaving::ClosingPanned | Leaving::ClosingNewer
    );
    let mut said = Vec::new();
    while !(kept && opened) {
        let s = rig.next();
        kept |= matches!(s, Said::Kept);
        opened |= s.developed().is_some_and(|d| d.0 == opening);
        said.push(s.developed());
    }
    let picture = rig.kept(&edit);
    assert!(
        picture.is_some(),
        "{leaving:?}: kept nothing, after {said:?}"
    );
    rig.end();
    picture
}

/// A save waiting on the settle when its frame is left (another frame
/// opened before the settle, as it runs, with the save sent beside the
/// open, or while the Original is up) or the editor closes (with the
/// view panned, or a newer develop asked, which the settle does not
/// give way to on the way out): the settle is made first, and the
/// frame's pictures are those of a session that never sharpened a part
/// first, to the byte.
#[test]
fn a_frame_left_between_the_rounds_keeps_the_whole_pictures() {
    let Some(whole) = kept_on_leaving(Leaving::Not, "Cam") else {
        skipped("a frame left between the rounds");
        return;
    };
    for (leaving, body) in [
        (Leaving::Before, "Cam 2"),
        (Leaving::During, "Cam 3"),
        (Leaving::Together, "Cam 4"),
        (Leaving::Original, "Cam 5"),
        (Leaving::Closing, "Cam 6"),
        (Leaving::ClosingPanned, "Cam 7"),
        (Leaving::ClosingNewer, "Cam 8"),
    ] {
        let kept = kept_on_leaving(leaving, body);
        assert!(kept == Some(whole.clone()), "{leaving:?}");
    }
}

/// The 2048 picture of `saved` kept by a session with no view: the
/// save sent with its develop.
fn kept_whole(saved: &Edit, body: &str) -> Option<Vec<u8>> {
    let rig = Rig::new(None, body, false)?;
    rig.save(saved);
    rig.worker.send(develop(saved, 2));
    while !matches!(rig.next(), Said::Kept) {}
    let picture = rig.kept(saved);
    rig.end();
    picture
}

/// A save waiting on the settle, and the next develop of the frame
/// asked as the settle runs (the next slider move): the settle gives
/// way to it, as it would with no save waiting. A newer develop of the
/// same picture (an exposure) keeps the save waiting, made from that
/// develop's settle; one of another picture (Clarity again) lets it go,
/// and that edit's own save is made from its settle. Either way the
/// pictures kept are a session's with no view, to the byte.
#[test]
fn a_save_waits_for_the_newest_settle() {
    let first = clarity(0.4);
    let mut brighter = first.clone();
    brighter.light.exposure = 0.5;
    let other = clarity(0.2);
    let Some(whole_first) = kept_whole(&first, "Cam") else {
        skipped("a save and the newest settle");
        return;
    };
    let whole_other = kept_whole(&other, "Cam 2").unwrap();
    for (newer, body) in [(&brighter, "Cam 3"), (&other, "Cam 4")] {
        let rig = Rig::new(Some(CLOSE), body, false).unwrap();
        rig.worker.queue.0.lock().unwrap().develop_on_fill_ask = Some(develop(newer, 3));
        rig.save(&first);
        rig.worker.send(develop(&first, 2));
        assert_eq!(rig.next().developed(), Some((2, true, 20)));
        let same = std::ptr::eq(newer, &brighter);
        // Another picture's develop lets the first save go: the window
        // saves the newer edit once it rests.
        if !same {
            assert!(matches!(rig.next(), Said::Kept), "the first save let go");
        }
        assert_eq!(
            rig.next().developed(),
            Some((3, true, 20)),
            "the settle gave way to the newer develop"
        );
        if !same {
            rig.save(newer);
        }
        let mut said = Vec::new();
        loop {
            let s = rig.next();
            said.push(s.developed());
            if matches!(s, Said::Kept) {
                break;
            }
        }
        assert!(
            said.contains(&Some((3, false, 20))) && !said.contains(&Some((2, false, 20))),
            "{said:?}"
        );
        let (saved, whole) = if same {
            (&first, &whole_first)
        } else {
            (&other, &whole_other)
        };
        assert!(rig.kept(saved).as_ref() == Some(whole), "{newer:?}");
        rig.end();
    }
}

/// An open forgets what the window showed of the frame left, so a
/// develop sent before the new frame is drawn sharpens it whole; what
/// the window says after the open, a develop folded into it keeps.
#[test]
fn an_open_forgets_what_the_window_showed() {
    let dir = crate::testing::scratch_dir("settle-open-seen");
    let worker = Worker::new(|_| {});
    worker.set_seen(Some(CLOSE));
    worker.send(open(&dir.join("next.dng"), 2));
    assert_eq!(worker.seen(), None, "the frame left's view");
    worker.set_seen(Some(FIT));
    worker.send(develop(&clarity(0.4), 3));
    assert_eq!(worker.seen(), Some(FIT), "the new frame's view");
    worker.stop_all(std::time::Duration::from_secs(60));
    crate::testing::remove_dir_retry(&dir);
}
