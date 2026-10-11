//! The lightbox: the grid over the view it shows (a collection in its
//! own order, the folder as the filter leaves it), with everything that
//! gets in the way of judging a set whole taken out, and each tile the
//! picture an export of the frame will be.
//!
//! The layout is the grid's own (`grid`), at a cell this module works
//! out from how many tiles go across. The pictures are not the grid's
//! thumbnails, which stop at 360 and are the camera's JPEG until the
//! edit's are kept: each tile has its own, made for its size on the
//! culling loupe's decode threads (`cull::Prefetcher`, a second one of
//! the lightbox's own) and read from the same cache.
//!
//! Which picture a tile shows:
//! - A frame with an edit (held as edited, as the strip's pencil and its
//!   picture are): the picture of its edit, the 2048 one shrunk to the
//!   tile when it is kept, else the largest of the made sizes kept. With
//!   none kept the tile stays dark and waits: the thumbnails' pool makes
//!   the edit's pictures behind the tiles' thumbnails, and a frame kept
//!   is asked for again. Never the camera's JPEG.
//! - A frame without one: the camera's JPEG, which is what a frame with
//!   no edit shows everywhere (the strip, the grid, the culling loupe),
//!   at the tile's size; or its local preview where its root is out of
//!   reach.
//!
//! Nothing here develops.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model};

use crate::cull::{self, Loaded, Preview, Source, Want};
use crate::edited::{BIG, Edited, Shows};
use crate::panel::browser::{row_of, thumb_turns};
use crate::worker::Worker;
use crate::{App, STATE, State, cells, grid};
use greycard_library::thumbs::Thumb;

/// The fewest tiles across: two, side by side, is the closest look the
/// lightbox gives; the loupe is one key away for one alone.
pub const FEWEST: i32 = 2;
/// The most: a handful. More is the grid's job.
pub const MOST: i32 = 6;
/// Across when the lightbox opens.
pub const ACROSS: i32 = 4;
/// The smallest a tile gets on a window too narrow for the tiles asked.
const LEAST: f32 = 48.0;

/// The picture sizes a tile asks for, on the long edge in physical
/// pixels: the least that covers the tile. A few sizes rather than the
/// tile's own, so a window dragged a little wider does not remake every
/// picture.
pub const SIZES: [u32; 7] = [256, 384, 512, 768, 1024, 1536, BIG];

/// A tile's side for `across` tiles over a sheet `width` wide: the
/// grid's padding and gaps kept, the rest shared out, so the grid's own
/// `columns` gives `across` back. Held to [`FEWEST`] and [`MOST`].
pub fn cell(width: f32, across: i32) -> f32 {
    let across = across.clamp(FEWEST, MOST) as f32;
    let room = width - 2.0 * grid::PAD + grid::GAP;
    if !room.is_finite() || room <= 0.0 {
        return LEAST;
    }
    (room / across - grid::GAP).floor().max(LEAST)
}

/// Across after a step toward larger tiles (`by` positive, fewer
/// across) or smaller ones.
pub fn step(across: i32, by: i32) -> i32 {
    (across - by).clamp(FEWEST, MOST)
}

/// The size to ask for a tile's picture at: the tile's box is the cell
/// and the room a name would take under it, which a tall frame fills,
/// in physical pixels; the least of [`SIZES`] that covers it.
pub fn picture_size(cell: f32, scale: f32) -> u32 {
    let need = ((cell + grid::LABEL) * scale.max(0.1)).ceil();
    if !need.is_finite() {
        return BIG;
    }
    let need = need.max(1.0) as u32;
    SIZES.iter().copied().find(|&s| s >= need).unwrap_or(BIG)
}

/// The sizes of the edit's pictures kept, in the order a tile wanting
/// `size` tries them: the least that covers it first, larger ones after,
/// then the smaller ones from the largest down, so a tile takes a picture
/// too small only when nothing larger is kept.
pub fn kept_order(size: u32) -> Vec<u32> {
    let mut kept: Vec<u32> = grid::MADE.iter().copied().chain([BIG]).collect();
    kept.sort_unstable();
    let (small, large): (Vec<u32>, Vec<u32>) = kept.into_iter().partition(|&s| s < size);
    large.into_iter().chain(small.into_iter().rev()).collect()
}

/// What a tile shows.
#[derive(Debug)]
pub(crate) enum Pick {
    /// The picture of the frame's edit, at the tile's size or the
    /// nearest kept.
    Edit(Thumb),
    /// The frame has an edit and no picture of it is kept yet: nothing,
    /// until one is.
    Wait,
    /// The frame has no edit: the camera's JPEG.
    Camera,
}

/// What a tile wanting `size` shows of the frame of content `hash`,
/// which `shows` says shows its edit or not. Only the cache is read.
pub(crate) fn pick(edited: Option<&Edited>, hash: Option<&str>, shows: Shows, size: u32) -> Pick {
    if shows == Shows::Camera {
        return Pick::Camera;
    }
    let (Some(edited), Some(hash)) = (edited, hash) else {
        return Pick::Wait;
    };
    kept_order(size)
        .into_iter()
        .find_map(|s| edited.lookup(hash, s, shows))
        .map_or(Pick::Wait, |thumb| {
            Pick::Edit(crate::edited::shrunk(&thumb, size))
        })
}

/// One tile's picture, on a decode thread: see [`pick`]. The camera's
/// is the culling loupe's own decode at the tile's size.
pub(crate) fn fetch(
    want: &Want,
    previews: Option<&Arc<crate::previews::Previews>>,
    deliver: &dyn Fn(Loaded),
) {
    let edited = previews.and_then(|p| p.edited());
    let (hash, shows) = match &want.from {
        Source::Preview { hash, .. } => (
            Some(hash.clone()),
            edited.map_or(Shows::Camera, |e| e.shows_offline(&want.path)),
        ),
        Source::File => {
            let shows = edited.map_or(Shows::Camera, |e| e.shows(&want.path));
            let hash = (shows != Shows::Camera)
                .then(|| greycard_library::hash_file(&want.path).ok())
                .flatten();
            (hash, shows)
        }
    };
    match pick(edited, hash.as_deref(), shows, want.size) {
        Pick::Edit(thumb) => deliver(Loaded::Ok {
            file: want.file,
            path: want.path.clone(),
            size: want.size,
            preview: Arc::new(Preview::edited(thumb)),
        }),
        Pick::Wait => deliver(Loaded::NoPreview {
            file: want.file,
            path: want.path.clone(),
        }),
        Pick::Camera => cull::fetch(
            &Want {
                lightbox: false,
                make_preview: false,
                ..want.clone()
            },
            previews,
            deliver,
        ),
    }
}

/// A picture in hand: as it was made, the camera's way up, RGB, and
/// the image drawn from it, turned as its frame is turned, made once a
/// turn rather than on every look.
struct Picture {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    /// The size it was asked for at; another than the lightbox's has it
    /// asked for again, and it stays drawn, stretched, until that lands.
    /// Zero for a frame whose pictures have changed since.
    size: u32,
    /// The image, and the turn it was made at.
    image: Option<((u8, bool), slint::Image)>,
}

impl Picture {
    /// From a decode's RGBA, no larger than half again `size`: a local
    /// preview comes at its own 2048 whatever the tile.
    fn of(preview: &Preview, size: u32) -> Self {
        let mut rgb = Vec::with_capacity(preview.rgba.len() / 4 * 3);
        for px in preview.rgba.as_chunks::<4>().0 {
            rgb.extend_from_slice(&px[..3]);
        }
        let thumb = Thumb {
            width: preview.width,
            height: preview.height,
            rgb,
        };
        let thumb = if preview.width.max(preview.height) > size + size / 2 {
            crate::edited::shrunk(&thumb, size)
        } else {
            thumb
        };
        Self {
            width: thumb.width,
            height: thumb.height,
            rgb: thumb.rgb,
            size,
            image: None,
        }
    }

    /// The image at `turns`, made only when the turn is another than the
    /// last one's.
    fn image(&mut self, turns: (u8, bool)) -> slint::Image {
        if let Some((at, image)) = &self.image
            && *at == turns
        {
            return image.clone();
        }
        let (w, h, rgb) = greycard_edit::geometry::turn_pixels(
            self.width,
            self.height,
            &self.rgb,
            turns.0,
            turns.1,
        );
        let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(w, h);
        buf.make_mut_bytes().copy_from_slice(&rgb);
        let image = slint::Image::from_rgb8(buf);
        self.image = Some((turns, image.clone()));
        image
    }
}

/// The lightbox while it is on: the pictures of the tiles it has cells
/// for, and what it knows of the rest of them.
pub(crate) struct Lightbox {
    /// The size the pictures are asked for at now.
    size: u32,
    /// The grid's cells, first row and how many, when last looked.
    window: (usize, usize),
    pictures: HashMap<PathBuf, Picture>,
    /// Frames whose edit has no picture kept yet: asked for again when
    /// one is. A picture of one in hand (its edit before a sync) stays
    /// up, dimmed.
    waiting: HashSet<PathBuf>,
    /// Frames no picture could be had of.
    failed: HashSet<PathBuf>,
}

impl Lightbox {
    fn new() -> Self {
        Self {
            size: 0,
            window: (0, 0),
            pictures: HashMap::new(),
            waiting: HashSet::new(),
            failed: HashSet::new(),
        }
    }

    /// Whether a tile's picture of `path` is in hand, for the tests.
    #[cfg(test)]
    pub(crate) fn has(&self, path: &std::path::Path) -> bool {
        self.pictures.contains_key(path)
    }

    /// Whether `path` waits for its edit's picture, for the tests.
    #[cfg(test)]
    pub(crate) fn waits(&self, path: &std::path::Path) -> bool {
        self.waiting.contains(path)
    }
}

/// The rows the lightbox has tiles for: the grid's cells, held to the
/// list.
fn rows(st: &State, window: (usize, usize)) -> std::ops::Range<usize> {
    let end = (window.0 + window.1).min(st.shown.len());
    window.0.min(end)..end
}

/// Put `file`'s picture on its row, turned as its thumbnail is, and
/// whether it waits for its edit's; or take the row's off when there is
/// none in hand. A row that already says so is not written.
pub(crate) fn show(st: &mut State, app: &App, file: usize) {
    if st.lightbox.is_none() {
        return;
    }
    let Some(row) = row_of(st, file) else {
        return;
    };
    let Some(path) = st.files.get(file).cloned() else {
        return;
    };
    let turns = thumb_turns(st, app, file);
    let model = app.get_thumbs();
    let Some(mut t) = model.row_data(row) else {
        return;
    };
    let Some(lb) = st.lightbox.as_mut() else {
        return;
    };
    let pending = lb.waiting.contains(&path);
    let big = lb.pictures.get_mut(&path).map(|p| p.image(turns));
    if big.is_none() && t.big.size().width == 0 && t.pending == pending {
        return;
    }
    t.big = big.unwrap_or_default();
    t.pending = pending;
    model.set_row_data(row, t);
}

/// Take the pictures off rows `range` of the list.
fn clear_rows(app: &App, range: std::ops::Range<usize>) {
    let model = app.get_thumbs();
    for row in range {
        if let Some(mut t) = model.row_data(row)
            && (t.big.size().width > 0 || t.pending)
        {
            t.big = slint::Image::default();
            t.pending = false;
            model.set_row_data(row, t);
        }
    }
}

/// The lightbox follows the grid's cells: the pictures of rows that
/// left them are let go, those in hand put on the rows that came in (or
/// on a row that lost its picture to a new list), and the rest asked
/// for, the rows on screen first. Rows that stayed are not touched: this
/// runs on every scroll and arrow. A tile grown or shrunk past its
/// pictures' size keeps them drawn until the new size lands.
pub(crate) fn refresh(st: &mut State, app: &App) {
    let size = picture_size(app.get_tile_cell(), app.window().scale_factor());
    let window = cells::window(app, cells::View::Grid);
    let Some(lb) = st.lightbox.as_mut() else {
        return;
    };
    let was = lb.window;
    lb.window = window;
    if size != lb.size {
        lb.size = size;
        lb.failed.clear();
    }
    let now = rows(st, window);
    let before = rows(st, was);
    for r in before.clone().filter(|r| !now.contains(r)) {
        clear_rows(app, r..r + 1);
    }
    let paths: HashSet<PathBuf> = now.clone().map(|r| st.files[st.shown[r]].clone()).collect();
    if let Some(lb) = st.lightbox.as_mut() {
        lb.pictures.retain(|p, _| paths.contains(p));
        lb.waiting.retain(|p| paths.contains(p));
    }
    let model = app.get_thumbs();
    for r in now {
        let f = st.shown[r];
        // A row that does not say what the lightbox holds of its frame
        // (a new list's row, which has neither its picture nor its
        // Developing word): put right, whether or not it was in the
        // window before.
        let stale = st.lightbox.as_ref().is_some_and(|lb| {
            let path = &st.files[f];
            model.row_data(r).is_some_and(|t| {
                (t.big.size().width == 0 && lb.pictures.contains_key(path))
                    || t.pending != lb.waiting.contains(path)
            })
        });
        if !before.contains(&r) || stale {
            show(st, app, f);
        }
    }
    want(st);
}

/// Ask for the pictures the tiles do not have at the size they want:
/// the rows on screen first, then the rows of the margin. Only the
/// window's rows, so nothing about the screen is read here.
fn want(st: &mut State) {
    let Some(lb) = st.lightbox.as_ref() else {
        return;
    };
    let now = rows(st, lb.window);
    let shown: Vec<usize> = match st.grid_shown {
        Some((a, b)) => (a.max(0) as usize..=b.max(0) as usize)
            .filter(|r| now.contains(r))
            .collect(),
        None => Vec::new(),
    };
    let order = shown
        .iter()
        .copied()
        .chain(now.filter(|r| !shown.contains(r)));
    let mut wants = Vec::new();
    let mut out_of_reach = Vec::new();
    for r in order {
        let f = st.shown[r];
        let path = &st.files[f];
        let had = lb.pictures.get(path).is_some_and(|p| p.size == lb.size);
        if had || lb.failed.contains(path) || lb.waiting.contains(path) {
            continue;
        }
        let plan = crate::panel::cull::loupe_plan(&st.files, &st.from_row, &st.library, f);
        let want = match plan {
            cull::Plan {
                preview: Some(key), ..
            } if !plan.file => Want::local(f, path.clone(), lb.size, key),
            cull::Plan { file: false, .. } => {
                out_of_reach.push(path.clone());
                continue;
            }
            _ => Want::of_file(f, path.clone(), lb.size),
        };
        wants.push(Want {
            lightbox: true,
            ..want
        });
    }
    if let Some(lb) = st.lightbox.as_mut() {
        lb.failed.extend(out_of_reach);
    }
    if let Some(fetch) = st.lightbox_fetch.as_ref() {
        fetch.want(wants);
    }
}

/// A frame's pictures may have changed (its edit's were kept, or it was
/// saved or synced): the tile's is asked for again, and the one it has
/// stays up meanwhile.
pub(crate) fn forget(st: &mut State, file: usize) {
    let Some(path) = st.files.get(file).cloned() else {
        return;
    };
    let Some(lb) = st.lightbox.as_mut() else {
        return;
    };
    if let Some(p) = lb.pictures.get_mut(&path) {
        p.size = 0;
    }
    lb.waiting.remove(&path);
    lb.failed.remove(&path);
    want(st);
}

/// A tile's picture from its decode thread, on the window's.
pub(crate) fn deliver(app: &App, loaded: Loaded) {
    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    take(&mut state.borrow_mut(), app, loaded);
}

/// A tile's picture, or word that there is none yet or none at all, put
/// on its row.
pub(crate) fn take(st: &mut State, app: &App, loaded: Loaded) {
    let (file, path) = match &loaded {
        Loaded::Ok { file, path, .. }
        | Loaded::Failed { file, path, .. }
        | Loaded::NoPreview { file, path } => (*file, path.clone()),
    };
    if st.files.get(file) != Some(&path) {
        return;
    }
    let in_window = row_of(st, file).is_some_and(|r| {
        st.lightbox
            .as_ref()
            .is_some_and(|lb| rows(st, lb.window).contains(&r))
    });
    let Some(lb) = st.lightbox.as_mut() else {
        return;
    };
    match loaded {
        Loaded::Ok { size, preview, .. } => {
            if size != lb.size || !in_window {
                return;
            }
            lb.waiting.remove(&path);
            lb.pictures.insert(path, Picture::of(&preview, size));
        }
        Loaded::NoPreview { .. } => {
            // An edit with no picture kept yet: a picture the tile had
            // (the edit before a sync) stays up, dimmed, until the new
            // one is kept; a tile with none says it is being made.
            lb.waiting.insert(path);
        }
        Loaded::Failed { message, .. } => {
            tracing::debug!("lightbox {}: {message}", path.display());
            lb.failed.insert(path);
        }
    }
    show(st, app, file);
}

/// Into the lightbox, or out of it: its decode threads made the first
/// time, its pictures let go on the way out.
fn changed(st: &mut State, app: &App, worker: &Worker) {
    if app.get_lightbox() {
        if st.lightbox_fetch.is_none() {
            let app_weak = app.as_weak();
            let fetch = cull::Prefetcher::new(move |loaded| {
                let app_weak = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_weak.upgrade() {
                        deliver(&app, loaded);
                    }
                });
            });
            fetch.set_previews(worker.previews());
            st.lightbox_fetch = Some(fetch);
        }
        if st.lightbox.is_none() {
            st.lightbox = Some(Lightbox::new());
        }
        refresh(st, app);
    } else if let Some(lb) = st.lightbox.take() {
        let end = (lb.window.0 + lb.window.1).min(app.get_thumbs().row_count());
        clear_rows(app, lb.window.0.min(end)..end);
        if let Some(fetch) = st.lightbox_fetch.as_ref() {
            fetch.want(Vec::new());
        }
    }
}

pub(crate) fn install(
    app: &App,
    state: &std::rc::Rc<std::cell::RefCell<State>>,
    worker: &std::rc::Rc<Worker>,
) {
    app.on_lightbox_cell(cell);
    app.set_lightbox_across(ACROSS);
    {
        let app_weak = app.as_weak();
        app.on_lightbox_zoom(move |by| {
            if let Some(app) = app_weak.upgrade() {
                app.set_lightbox_across(step(app.get_lightbox_across(), by));
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_lightbox_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                changed(&mut state.borrow_mut(), &app, &worker);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_tiles_go_across_as_asked_and_fill_the_sheet() {
        for width in [900.0, 1280.0, 1500.0, 1920.0, 2560.0] {
            for across in FEWEST..=MOST {
                let c = cell(width, across);
                assert_eq!(grid::columns(width, c), across, "{width} {across}");
                // The row, its gaps and its padding, within a tile's
                // pitch of the sheet: no column left over.
                let row = 2.0 * grid::PAD + across as f32 * (c + grid::GAP) - grid::GAP;
                assert!(
                    row <= width && width - row < across as f32,
                    "{width} {across}"
                );
            }
        }
        // Held to the range.
        assert_eq!(cell(1500.0, 1), cell(1500.0, FEWEST));
        assert_eq!(cell(1500.0, 40), cell(1500.0, MOST));
        assert_eq!(cell(0.0, 4), LEAST);
    }

    #[test]
    fn plus_is_fewer_larger_tiles_and_the_steps_stop() {
        assert_eq!(step(4, 1), 3);
        assert_eq!(step(4, -1), 5);
        assert_eq!(step(FEWEST, 1), FEWEST);
        assert_eq!(step(MOST, -1), MOST);
        assert!(cell(1500.0, step(4, 1)) > cell(1500.0, 4));
    }

    #[test]
    fn a_tile_asks_for_the_least_size_that_covers_it() {
        // Four across a 1500 window: 351 and the name's room, 373.
        assert_eq!(picture_size(351.0, 1.0), 384);
        // On a screen at twice the scale: 746, so 768.
        assert_eq!(picture_size(351.0, 2.0), 768);
        // Two across a 2560 window at 2: 2048 and no more.
        assert_eq!(picture_size(1250.0, 2.0), BIG);
        assert_eq!(picture_size(f32::INFINITY, 1.0), BIG);
    }

    use crate::testing::{folder, state_for, window};

    /// The window's settle after a change: its `changed` handlers run,
    /// and the sheet lays out and says what it shows.
    fn settle(app: &App) {
        slint::platform::update_timers_and_animations();
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        slint::platform::update_timers_and_animations();
    }

    /// A key, and the window settled after it.
    fn press(app: &App, key: impl Into<slint::SharedString>) {
        crate::testing::press(app, key);
        settle(app);
    }
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{Key, WindowEvent};

    /// A window of `count` frames laid out at 1500 by 950, wired as a
    /// session wires it, the first frame chosen and the panes showing.
    fn lightbox_window(count: usize) -> (App, std::rc::Rc<std::cell::RefCell<State>>) {
        let app = window(count);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(count));
        app.set_left_hidden(false);
        app.set_right_hidden(false);
        app.set_strip_hidden(false);
        app.invoke_select(0);
        settle(&app);
        (app, state)
    }

    /// How many elements carry `label` in `role`.
    fn with_role(app: &App, label: &str, role: i_slint_backend_testing::AccessibleRole) -> usize {
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        ElementHandle::find_by_accessible_label(app, label)
            .filter(|e| e.accessible_role() == Some(role))
            .count()
    }

    fn with_modifier(app: &App, key: Key, f: impl FnOnce()) {
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        f();
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }

    /// L puts the panes away and opens the lightbox over the grid;
    /// Escape leaves it to the grid with the panes as they were, and so
    /// does G; L from the grid comes back, and L again leaves.
    #[test]
    fn l_enters_and_escape_or_g_leaves_to_the_grid_with_the_panes_back() {
        let (app, state) = lightbox_window(11);
        assert!(!app.get_grid_open());
        press(&app, "l");
        assert!(app.get_lightbox() && app.get_grid_open());
        assert!(app.get_left_hidden() && app.get_right_hidden() && app.get_strip_hidden());
        assert!(state.borrow().lightbox.is_some());
        // Tab is the grid's over it: the left pane back, and away.
        press(&app, Key::Tab);
        assert!(!app.get_left_hidden());
        press(&app, Key::Tab);
        assert!(app.get_left_hidden());
        press(&app, Key::Escape);
        assert!(!app.get_lightbox());
        assert!(app.get_grid_open(), "Escape leaves to the grid");
        assert!(!app.get_left_hidden() && !app.get_right_hidden() && !app.get_strip_hidden());
        assert!(state.borrow().lightbox.is_none());
        // From the grid, with the left pane put away: kept so.
        app.set_left_hidden(true);
        press(&app, "L");
        assert!(app.get_lightbox());
        press(&app, "g");
        assert!(!app.get_lightbox() && app.get_grid_open());
        assert!(app.get_left_hidden() && !app.get_right_hidden());
        press(&app, "l");
        press(&app, "l");
        assert!(!app.get_lightbox() && app.get_grid_open());
    }

    /// Plus and minus, and Ctrl with the wheel, step the tiles across,
    /// and the grid's own arithmetic lays them out that many across.
    #[test]
    fn plus_and_minus_reflow_the_tiles() {
        let (app, _state) = lightbox_window(120);
        let grid_cell = app.get_grid_cell();
        press(&app, "l");
        let across = |app: &App| grid::columns(app.get_grid_w(), app.get_tile_cell());
        assert_eq!(app.get_lightbox_across(), ACROSS);
        assert_eq!(across(&app), ACROSS);
        let four = app.get_tile_cell();
        press(&app, "+");
        assert_eq!((app.get_lightbox_across(), across(&app)), (3, 3));
        assert!(app.get_tile_cell() > four);
        press(&app, "-");
        press(&app, "-");
        press(&app, "-");
        assert_eq!((app.get_lightbox_across(), across(&app)), (6, 6));
        press(&app, "-");
        assert_eq!(app.get_lightbox_across(), MOST, "a handful and no more");
        // The cells follow: whole rows of six.
        assert_eq!(cells::window(&app, cells::View::Grid).1 % 6, 0);
        // Ctrl and the wheel over the sheet: larger.
        with_modifier(&app, Key::Control, || {
            app.window().dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700.0, 400.0),
                delta_x: 0.0,
                delta_y: 60.0,
            });
        });
        settle(&app);
        assert_eq!(app.get_lightbox_across(), 5);
        // The grid's own cell is left as it was, for the grid.
        assert_eq!(app.get_grid_cell(), grid_cell);
        press(&app, Key::Escape);
        assert_eq!(app.get_tile_cell(), grid_cell);
    }

    /// The grid draws names, badges and marks; the lightbox none of
    /// them, nor its header: each tile is its picture, named only for a
    /// screen reader.
    #[test]
    fn no_badges_names_marks_or_header_in_the_lightbox() {
        use i_slint_backend_testing::AccessibleRole;
        let (app, _state) = lightbox_window(6);
        let model = app.get_thumbs();
        for r in 0..6 {
            let mut t = model.row_data(r).unwrap();
            t.rating = 3;
            t.flag = 1;
            t.label = 2;
            t.edited = true;
            t.exported = true;
            model.set_row_data(r, t);
        }
        press(&app, "g");
        let counts = |app: &App| {
            (
                crate::testing::count_labeled(app, "3 stars, pick, yellow"),
                crate::testing::count_labeled(app, "Edited"),
                crate::testing::count_labeled(app, "Exported"),
                // Named on the sheet: any element of the name that is not
                // the tile's own, read to a screen reader.
                crate::testing::count_labeled(app, "f03.CR3")
                    - with_role(app, "f03.CR3", AccessibleRole::Image),
                crate::testing::count_labeled(app, "Lightbox"),
            )
        };
        let (badges, edited, exported, name, button) = counts(&app);
        assert!(
            badges >= 6 && edited >= 6 && exported >= 6,
            "the grid draws them"
        );
        assert!(name >= 1, "the grid names the frame");
        assert_eq!(button, 1, "the header offers the lightbox");
        press(&app, "l");
        assert_eq!(counts(&app), (0, 0, 0, 0, 0));
        assert_eq!(with_role(&app, "f03.CR3", AccessibleRole::Image), 1);
        // The rating key still rates, and still draws nothing.
        press(&app, "4");
        assert_eq!(model.row_data(0).unwrap().rating, 4);
        assert_eq!(counts(&app).0, 0);
    }

    /// Return is the loupe on the frame chosen, the lightbox kept under
    /// it; an arrow there steps the frame; Return, Escape or L come back
    /// to the lightbox on that frame, scrolled where it was.
    #[test]
    fn the_loupe_round_trip_keeps_the_frame_and_the_scroll() {
        let (app, state) = lightbox_window(60);
        press(&app, "l");
        // Far down the sheet, so it scrolls.
        app.invoke_select(41);
        settle(&app);
        let scrolled = app.get_grid_scroll();
        assert!(scrolled > 0.0);
        let backs: [slint::SharedString; 3] = [Key::Return.into(), Key::Escape.into(), "l".into()];
        for back in backs {
            press(&app, Key::Return);
            assert!(!app.get_grid_open(), "the loupe");
            assert!(app.get_lightbox() && app.get_lightbox_loupe());
            press(&app, back);
            assert!(app.get_grid_open() && app.get_lightbox() && !app.get_lightbox_loupe());
            assert_eq!(app.get_selected(), 41);
            assert_eq!(app.get_grid_scroll(), scrolled);
        }
        // An arrow in the loupe steps the frame, and the sheet comes back
        // where it was, the frame on it.
        press(&app, Key::Return);
        press(&app, Key::RightArrow);
        assert_eq!(app.get_selected(), 42);
        press(&app, Key::Escape);
        assert_eq!(app.get_selected(), 42);
        assert_eq!(app.get_grid_scroll(), scrolled);
        assert_eq!(state.borrow().current, Some(42));
        // A double-click's way out of the sheet is the loupe too.
        app.set_grid_open(false);
        settle(&app);
        assert!(app.get_lightbox_loupe());
        press(&app, Key::Escape);
        assert!(app.get_grid_open() && app.get_lightbox());
        press(&app, Key::Escape);
        assert!(!app.get_lightbox() && app.get_grid_open());
    }

    /// With frames chosen beside the one on screen, Ctrl+Shift+S opens
    /// the sync sheet over the lightbox, and Escape takes the sheet down
    /// before it leaves anything.
    #[test]
    fn the_sync_sheet_opens_from_the_lightbox() {
        let (app, state) = lightbox_window(6);
        press(&app, "l");
        app.invoke_select(2);
        app.invoke_frame_clicked(3, true, false);
        app.invoke_frame_clicked(4, true, false);
        assert_eq!(app.get_set_count(), 3);
        with_modifier(&app, Key::Control, || {
            with_modifier(&app, Key::Shift, || press(&app, "S"));
        });
        assert!(app.get_sync_open());
        assert_eq!(
            app.get_sync_text(),
            "From IMG_0002.CR3 onto 2 other frames."
        );
        assert_eq!(state.borrow().current, Some(2));
        press(&app, Key::Escape);
        assert!(!app.get_sync_open());
        assert!(app.get_lightbox(), "the sheet went first");
    }

    fn mean(rgba: &[u8]) -> f64 {
        let px = rgba.as_chunks::<4>().0;
        px.iter().map(|p| p[0] as f64).sum::<f64>() / px.len().max(1) as f64
    }

    /// A tile of a frame with an edit is the picture of its edit, the
    /// 2048 one shrunk to the tile, never the camera's; with none kept it
    /// waits rather than show the camera's; a frame with no edit is the
    /// camera's picture.
    #[test]
    fn a_tile_is_the_edit_s_picture_and_never_the_camera_s_when_there_is_an_edit() {
        let dir = crate::testing::scratch_dir("lightbox-pick");
        std::fs::create_dir_all(&dir).unwrap();
        let (kept, waiting, plain) = (
            dir.join("kept.png"),
            dir.join("waiting.png"),
            dir.join("plain.png"),
        );
        // Three dim pictures, each of its own content.
        for (i, p) in [&kept, &waiting, &plain].into_iter().enumerate() {
            image::RgbImage::from_pixel(600, 400, image::Rgb([40 + i as u8, 40, 40]))
                .save(p)
                .unwrap();
        }
        // Two brightened in their sidecars; the third untouched.
        let mut edit = greycard_edit::Edit::for_picture();
        edit.light.exposure = 1.5;
        let mut sidecar = greycard_edit::Sidecar {
            current: edit.clone(),
            ..greycard_edit::Sidecar::default()
        };
        sidecar.save(&kept).unwrap();
        sidecar.save(&waiting).unwrap();
        let cache: crate::worker::ThumbCache = Arc::new(std::sync::Mutex::new(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        )));
        let edited = Arc::new(Edited::new(cache.clone()));
        edited.note_all([
            (kept.clone(), true, None, None),
            (waiting.clone(), true, None, None),
            (plain.clone(), false, None, None),
        ]);
        // The edit's pictures of the first, bright, kept as the reduced
        // develop keeps them.
        let bright = |size: u32| {
            let (w, h) = (size, size * 2 / 3);
            Thumb {
                width: w,
                height: h,
                rgb: vec![230; (w * h * 3) as usize],
            }
        };
        let hash = greycard_library::hash_file(&kept).unwrap();
        let made = crate::edited::Made {
            big: bright(BIG),
            thumbs: grid::MADE.iter().map(|&s| (s, bright(s))).collect(),
        };
        edited
            .store_reduced(&hash, edited.key_of(&edit), &made)
            .unwrap();
        let previews = Arc::new(crate::previews::Previews::new(cache).with_edited(edited));
        let fetched = |path: &Path, size: u32| {
            let got = std::cell::RefCell::new(None);
            fetch(
                &Want {
                    lightbox: true,
                    ..Want::of_file(0, path.to_path_buf(), size)
                },
                Some(&previews),
                &|l| *got.borrow_mut() = Some(l),
            );
            got.into_inner().expect("an answer")
        };
        // The edit's 2048, shrunk to the tile.
        match fetched(&kept, 768) {
            Loaded::Ok { preview, .. } => {
                assert!(preview.edited, "the edit's picture");
                assert_eq!((preview.width, preview.height), (768, 512));
                assert!(mean(&preview.rgba) > 200.0);
            }
            _ => panic!("no picture"),
        }
        // A small tile takes the least made size that covers it.
        match fetched(&kept, 256) {
            Loaded::Ok { preview, .. } => assert_eq!(preview.width, 256),
            _ => panic!("no picture"),
        }
        // Edited and nothing kept: nothing, not the camera's.
        assert!(matches!(fetched(&waiting, 768), Loaded::NoPreview { .. }));
        // No edit: the camera's picture, which here is the file.
        match fetched(&plain, 768) {
            Loaded::Ok { preview, .. } => {
                assert!(!preview.edited);
                assert!(mean(&preview.rgba) < 60.0);
            }
            _ => panic!("no picture"),
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// The window side: a tile's picture lands on its row's `big`. Word
    /// that an edited frame has no picture of its edit kept (as after a
    /// sync) keeps the picture it had, dimmed as pending, and a tile with
    /// none says Developing; a new size keeps the pictures drawn until
    /// it lands; leaving takes every tile's off.
    #[test]
    fn a_tile_keeps_its_picture_while_its_edit_s_is_made() {
        let (app, state) = lightbox_window(6);
        press(&app, "l");
        assert!(state.borrow().lightbox.as_ref().unwrap().size >= 256);
        let landed = |file: usize| {
            let path = state.borrow().files[file].clone();
            let size = state.borrow().lightbox.as_ref().unwrap().size;
            let picture = Preview::new(9, 6, vec![200; 9 * 6 * 4], (9, 6), false);
            take(
                &mut state.borrow_mut(),
                &app,
                Loaded::Ok {
                    file,
                    path,
                    size,
                    preview: Arc::new(picture),
                },
            );
        };
        let waits = |file: usize| {
            let path = state.borrow().files[file].clone();
            take(
                &mut state.borrow_mut(),
                &app,
                Loaded::NoPreview { file, path },
            );
        };
        let row = |r: usize| {
            let t = app.get_thumbs().row_data(r).unwrap();
            (t.big.size().width, t.pending)
        };
        landed(1);
        let path = state.borrow().files[1].clone();
        assert!(state.borrow().lightbox.as_ref().unwrap().has(&path));
        assert_eq!(row(1), (9, false));
        // Synced: the edit's new picture not kept yet. The old one stays.
        forget(&mut state.borrow_mut(), 1);
        assert_eq!(row(1), (9, false), "up while it is asked again");
        waits(1);
        assert!(state.borrow().lightbox.as_ref().unwrap().waits(&path));
        assert_eq!(row(1), (9, true), "kept, dimmed");
        settle(&app);
        assert_eq!(crate::testing::count_labeled(&app, "Developing"), 0);
        // A tile with nothing: dark, and says why.
        waits(3);
        assert_eq!(row(3), (0, true));
        settle(&app);
        assert_eq!(crate::testing::count_labeled(&app, "Developing"), 1);
        // The new picture lands: up, and no longer pending.
        landed(1);
        assert_eq!(row(1), (9, false));
        // A new size: the pictures stay drawn until theirs land.
        landed(2);
        let size = state.borrow().lightbox.as_ref().unwrap().size;
        press(&app, "+");
        press(&app, "+");
        assert_ne!(state.borrow().lightbox.as_ref().unwrap().size, size);
        assert_eq!(row(2).0, 9);
        landed(2);
        assert_eq!(row(2).0, 9);
        press(&app, Key::Escape);
        assert_eq!(row(2), (0, false));
        assert_eq!(row(3), (0, false));
    }

    /// The grid reopened under the lightbox's loupe by another way than
    /// its keys (a folder picked in the pane) ends the loupe: Return is
    /// the loupe again and plus sizes the tiles, not the hidden grid.
    #[test]
    fn the_grid_reopened_under_the_loupe_ends_it() {
        let (app, _state) = lightbox_window(20);
        press(&app, "l");
        press(&app, Key::Return);
        assert!(app.get_lightbox_loupe());
        app.set_grid_open(true);
        settle(&app);
        assert!(!app.get_lightbox_loupe());
        let cell = app.get_grid_cell();
        press(&app, "+");
        assert_eq!(app.get_lightbox_across(), ACROSS - 1);
        assert_eq!(app.get_grid_cell(), cell);
        press(&app, Key::Return);
        assert!(app.get_lightbox_loupe() && !app.get_grid_open());
    }

    /// A held key's repeats do nothing: held, Escape leaves the lightbox
    /// for the grid and no further, and L or Return held in the lightbox
    /// neither leave it nor flip into its loupe.
    #[test]
    fn a_held_key_is_one_press() {
        let (app, _state) = lightbox_window(20);
        let repeat = |text: slint::SharedString| {
            app.window()
                .dispatch_event(WindowEvent::KeyPressRepeated { text });
            settle(&app);
        };
        press(&app, "l");
        for key in [Key::Return.into(), "l".into(), "g".into()] {
            repeat(key);
            assert!(app.get_lightbox() && app.get_grid_open());
            assert!(!app.get_lightbox_loupe());
        }
        press(&app, Key::Escape);
        assert!(!app.get_lightbox() && app.get_grid_open());
        for _ in 0..3 {
            repeat(Key::Escape.into());
            assert!(app.get_grid_open(), "the grid stays");
        }
        // In the loupe, a held Return comes back once and stays.
        press(&app, "l");
        press(&app, Key::Return);
        assert!(app.get_lightbox_loupe());
        repeat(Key::Return.into());
        assert!(app.get_lightbox_loupe(), "the repeat alone does nothing");
        press(&app, Key::Return);
        repeat(Key::Return.into());
        assert!(app.get_grid_open() && !app.get_lightbox_loupe());
    }

    /// A frame waiting for its edit's picture, with none in hand, keeps
    /// its Developing word when its row is made again (a new list over
    /// the same frames), where the window did not move.
    #[test]
    fn a_waiting_tile_says_so_on_a_fresh_row() {
        let (app, state) = lightbox_window(6);
        press(&app, "l");
        let path = state.borrow().files[3].clone();
        take(
            &mut state.borrow_mut(),
            &app,
            Loaded::NoPreview { file: 3, path },
        );
        let pending = |app: &App| app.get_thumbs().row_data(3).unwrap().pending;
        assert!(pending(&app));
        let fresh: Vec<crate::Thumb> = (0..6)
            .map(|i| crate::Thumb {
                name: format!("f{i:02}.CR3").into(),
                ..Default::default()
            })
            .collect();
        cells::set_rows(&app, fresh);
        assert!(!pending(&app), "a fresh row knows nothing");
        settle(&app);
        assert!(pending(&app));
        assert_eq!(crate::testing::count_labeled(&app, "Developing"), 1);
    }

    /// Escape over a set of frames takes the set back to the frame on
    /// screen first, as over the grid; the next leaves.
    #[test]
    fn escape_lets_go_of_a_set_before_it_leaves() {
        let (app, _state) = lightbox_window(6);
        press(&app, "l");
        app.invoke_frame_clicked(2, true, false);
        assert_eq!(app.get_set_count(), 2);
        press(&app, Key::Escape);
        assert_eq!(app.get_set_count(), 1);
        assert!(app.get_lightbox());
        press(&app, Key::Escape);
        assert!(!app.get_lightbox());
    }

    /// Quit inside the lightbox: the settings keep the panes as they were
    /// before it put them away.
    #[test]
    fn the_settings_keep_the_panes_as_they_were_before_the_lightbox() {
        let (app, _state) = lightbox_window(6);
        app.set_right_hidden(true);
        press(&app, "l");
        assert!(app.get_left_hidden() && app.get_strip_hidden());
        let kept = crate::panel::startup::remember(&app);
        assert_eq!(
            (kept.hide_left, kept.hide_right, kept.hide_strip),
            (false, true, false)
        );
        press(&app, Key::Escape);
        let kept = crate::panel::startup::remember(&app);
        assert_eq!(
            (kept.hide_left, kept.hide_right, kept.hide_strip),
            (false, true, false)
        );
    }

    #[test]
    fn the_kept_sizes_are_tried_from_the_least_that_covers() {
        assert_eq!(kept_order(512), vec![BIG, 360, 256, 176, 128]);
        assert_eq!(kept_order(256), vec![256, 360, BIG, 176, 128]);
        assert_eq!(kept_order(4096), vec![BIG, 360, 256, 176, 128]);
    }
}
