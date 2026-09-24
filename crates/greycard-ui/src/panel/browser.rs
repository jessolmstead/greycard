use crate::panel::crop::read_geometry;
use crate::panel::cull::{
    cull_select, drop_placeholder, enter_cull, say_if_empty, show_filter, start_placeholder,
};
use crate::panel::edit::{
    current_turn, edit_to_develop, read_edit, save_edit, show_edit, write_sidecar,
};
use crate::panel::history::show_history;
use crate::*;

/// A browser row for a file: its name, the badges its meta asks for,
/// and no picture until the worker has made one.
pub(crate) fn thumb_for(path: &Path, meta: &Meta) -> Thumb {
    Thumb {
        name: file_name(path).into(),
        image: slint::Image::default(),
        rating: meta.rating.min(meta::STARS) as i32,
        flag: meta.flag.code(),
        label: meta.label.code(),
        chosen: false,
    }
}

/// The selection as it is acted on: the current frame and the rest
/// of the set, as files, sorted. Empty with nothing open.
pub(crate) fn chosen_frames(st: &State) -> Vec<usize> {
    selection::frames(&st.picked, st.current)
}

/// Put the set on the strip's and the grid's rows, and its size on
/// the window. Only the rows whose mark changed are written, so a
/// click in a folder of hundreds touches two rows and not all of
/// them.
pub(crate) fn show_set(st: &mut State, app: &App) {
    st.picked = chosen_frames(st);
    let model = app.get_thumbs();
    for (row, &f) in st.shown.iter().enumerate() {
        let chosen = st.picked.binary_search(&f).is_ok();
        if let Some(mut t) = model.row_data(row)
            && t.chosen != chosen
        {
            t.chosen = chosen;
            model.set_row_data(row, t);
        }
    }
    app.set_set_count(st.picked.len().max(1) as i32);
}

/// Put file `i`'s meta on its row in the strip and the grid, leaving
/// whatever picture the row already holds.
///
/// The row and the file are two different numbers whenever a filter
/// is on — `row_of` is what maps one to the other — and both the
/// read and the write are the row's, as `show_thumb` has it.
pub(crate) fn show_badges(st: &State, app: &App, i: usize) {
    let Some(meta) = st.sidecars.get(i).map(|s| &s.meta) else {
        return;
    };
    let Some(row) = row_of(st, i) else {
        return;
    };
    let model = app.get_thumbs();
    if let Some(mut t) = model.row_data(row) {
        t.rating = meta.rating.min(meta::STARS) as i32;
        t.flag = meta.flag.code();
        t.label = meta.label.code();
        model.set_row_data(row, t);
    }
}

/// Put a culling key's change into every frame in `frames`, write
/// each one's sidecar, and put the badges on its thumbnail.
///
/// The badge is the whole answer to the key; there is no word in the
/// status line to go with it. A cull is one key and then the arrow
/// to the next frame, and the develop that arrow starts would write
/// over any such word before it had been read.
///
/// The set is the whole selection (`chosen_frames`): the frame on
/// screen and whatever Ctrl, Shift or Shift and an arrow put beside
/// it. A label key is settled against the set first, so one press on
/// a mixed selection makes it uniform rather than half one thing and
/// half the other.
///
/// The write goes through the sidecar the edit is saved from, so
/// neither can lose the other: they are one file and one struct, and
/// what is written is always both. The panel may hold an edit newer
/// than the one on disk, which the save timer writes with this meta
/// beside it a moment later.
///
/// Under a filter the list can change: a frame just rejected leaves
/// a browser showing the picks, and so does one just given a star
/// under a filter that wants four. The selection then moves to the
/// nearest frame still shown, whose row comes back for the caller to
/// open once the state is free — a switch in culling, a develop out
/// of it, which is the rule the three-way Show set and the one
/// Lightroom keeps.
///
/// Whether the frame is still shown is asked of the filter rather
/// than of the kind of key, since every one of the three fields the
/// keys write is a field the filter can be reading. The chips' counts
/// move with any of them, so they are put out again either way.
pub(crate) fn set_meta(
    st: &mut State,
    app: &App,
    frames: &[usize],
    change: meta::Change,
) -> Option<usize> {
    let was_shown: Vec<bool> = frames.iter().map(|&i| row_of(st, i).is_some()).collect();
    let (_, moved) = greycard_edit::meta_into(&mut st.sidecars, frames, change);
    for &i in &moved {
        write_sidecar(st, i);
        show_badges(st, app, i);
    }
    app.set_reject_count(reject_count(st) as i32);
    if moved.is_empty() {
        return None;
    }
    let hides = frames
        .iter()
        .zip(&was_shown)
        .any(|(&i, &was)| was != filter_shows(st, i));
    if !hides {
        show_filter(st, app);
        return None;
    }
    let next = rebuild_browser(st, app);
    if next.is_none()
        && st.cull.is_some()
        && let Some(c) = st.current
    {
        // The rows moved under the window: decode about them anew.
        cull_select(st, app, c);
    }
    say_if_empty(st, app);
    next
}

/// Turn every frame in `frames` by `quarters` quarter turns
/// clockwise: the camera's orientation tag put right, which is a
/// fact about the picture and not a step in developing it.
///
/// The turn goes on the sidecar beside the edit, as the meta does
/// (§117), and is written promptly through the same `Sidecar::save`.
/// Nothing here records a history state or marks the panel dirty.
/// The crop, the keystone and a held aspect turn with the picture
/// (`Geometry::under_turned_source`); masks do not, since they are
/// in the developed picture's own units and a turn moves what is
/// under them, which is what an edit's own quarter turns already do.
///
/// For the open frame outside culling the panel holds the edit, so
/// the crop turns there and the sidecar takes the panel's edit
/// without recording it; whatever the panel was holding before is
/// recorded first, so a turn cannot swallow a slider's state.
pub(crate) fn turn_frames(
    st: &mut State,
    app: &App,
    worker: &Worker,
    frames: &[usize],
    quarters: i32,
) {
    if quarters.rem_euclid(4) == 0 {
        return;
    }
    // The frame whose edit the panel owns: the open one, and not
    // while culling, where the panel still holds the last-opened
    // frame's edit and the sidecars are the truth.
    let panel_frame = st.current.filter(|_| st.cull.is_none());
    if let Some(cull) = st.cull.as_mut() {
        cull.turned = Some(std::time::Instant::now());
    }
    let mut developing = false;
    for &f in frames {
        if st.sidecars.get(f).is_none() {
            continue;
        }
        migrate_frame(st, f);
        let aspect = frame_aspect(st, f);
        if aspect.is_none() && st.sidecars[f].placed() {
            tracing::warn!(
                "{}: no picture of it yet to measure by, so its masks and \
                 repair patches stay where they are on the screen",
                file_name(&st.files[f])
            );
        }
        if Some(f) == panel_frame {
            // The panel owns the open frame's edit, so what it is
            // holding is recorded first and the sidecar's states are
            // then turned together; the turn itself records nothing.
            let pending = read_edit(app, &st.edit, st.target);
            save_edit(st, pending);
            st.sidecars[f].turn_by(quarters, aspect);
            let edit = st.sidecars[f].current.clone();
            // The whole panel, since a turn moves masks and patches
            // as well as the crop. `show_edit` puts a chosen
            // adjustment's tab up, which a turn pressed on the Crop
            // tab has not asked for, so the tab is kept.
            let tab = app.get_panel_tab();
            show_edit(st, &edit, app, st.target);
            app.set_panel_tab(tab);
            st.edit = edit;
            // The developed size stands on end now, and the develop
            // that will say so is a second away: a second turn
            // pressed before it lands would otherwise measure the
            // frame by the shape it used to be.
            if quarters.rem_euclid(2) == 1 {
                st.source_size = (st.source_size.1, st.source_size.0);
            }
            developing = true;
        } else {
            greycard_edit::turn_into(&mut st.sidecars, &[f], quarters, aspect);
        }
        write_sidecar(st, f);
        let (turns, flip) = thumb_turns(st, app, f);
        show_thumb(st, app, f, turns, flip);
    }
    // The rasters are keyed by adjustment and component and know
    // nothing of a turn: a brush's is only brought up to its strokes
    // rather than remade, and a subject's shape does not change at
    // all, so both would go on masking the pixels they masked
    // before. They are cheap beside a mask in the wrong place.
    st.rasters.clear();
    st.learned.clear();
    st.asked.clear();
    // The picture stands on end now, so whatever was fitted is
    // fitted afresh; the culling loupe reads the turn out of the
    // same matrix on its next frame, which is a redraw and not a
    // decode.
    st.image_size = (0, 0);
    if developing {
        // Whatever the panel was holding was recorded on the way in;
        // the turn itself added no state.
        show_history(st, app);
        st.generation += 1;
        app.set_status("developing...".into());
        app.set_busy(true);
        worker.send(Job::Develop {
            edit: edit_to_develop(app, &st.edit),
            generation: st.generation,
            turn: current_turn(st),
        });
    }
    app.window().request_redraw();
}

/// Put file `i`'s picture on the filmstrip turned `turns` quarters and
/// mirrored if `flip`, as the viewport turns the file.
pub(crate) fn show_thumb(st: &mut State, app: &App, i: usize, turns: u8, flip: bool) {
    let Some((w, h, rgb)) = &st.thumb_base[i] else {
        return;
    };
    // A frame the filter hides has no row; its picture waits for the
    // list to show it again.
    st.thumb_shown[i] = Some((turns, flip));
    let Some(row) = row_of(st, i) else {
        return;
    };
    let (pw, ph, turned) = greycard_edit::geometry::turn_pixels(*w, *h, rgb, turns, flip);
    let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(pw, ph);
    buf.make_mut_bytes().copy_from_slice(&turned);
    let model = app.get_thumbs();
    if let Some(mut t) = model.row_data(row) {
        t.image = slint::Image::from_rgb8(buf);
        model.set_row_data(row, t);
    }
}

/// How a frame stands in its own file, read once and kept: the
/// orientation tag, and the size a develop of it comes out at. One
/// metadata read a frame a session, and only for a frame that is
/// turned or whose meta is written to an XMP.
pub(crate) fn stance(st: &mut State, file: usize) -> Option<greycard_core::decode::Stance> {
    let path = st.files.get(file)?.clone();
    if let Some(s) = st.stances.get(&path) {
        return Some(*s);
    }
    match greycard_core::decode::stance_path(&path) {
        Ok(s) => {
            st.stances.insert(path, s);
            Some(s)
        }
        Err(e) => {
            tracing::warn!("{}: how the frame stands: {e}", file_name(&path));
            None
        }
    }
}

/// A frame's own orientation tag. `None` when the file will not say,
/// which is when nothing here writes or believes an XMP's
/// orientation for it.
pub(crate) fn camera_tag(st: &mut State, file: usize) -> Option<Orientation> {
    stance(st, file).map(|s| s.orientation)
}

/// A turn in words, for the log: which way and how far.
pub(crate) fn turn_words(quarters: i32) -> &'static str {
    match quarters.rem_euclid(4) {
        1 => "a quarter turn to the right of its tag",
        2 => "a half turn from its tag",
        3 => "a quarter turn to the left of its tag",
        _ => "as its tag says",
    }
}

/// The shape of a frame's developed picture, width over height, for
/// the map that carries masks and repair patches round a turn.
///
/// They are in units of that picture's width, both ways, so the map
/// wants its aspect and nothing else — but it has to be *that*
/// picture's. The file's own answer comes first: the develop on
/// screen when it is this frame's, and otherwise the size rawler
/// reports for the frame with no pixel decoded, turned by the tag and
/// by the frame's own quarters. The camera's JPEG is only a fallback
/// for want of those, because a body can write its embedded picture
/// at 16:9 or square with the raw left full frame, and a mask mapped
/// by 1.778 where 1.5 was wanted lands nowhere.
///
/// `None` when nothing can say, which is the one case where a turn
/// cannot carry placed work with it.
pub(crate) fn frame_aspect(st: &mut State, file: usize) -> Option<f32> {
    let turn = st.sidecars.get(file).map_or(0, |s| s.turn);
    let aspect = |(w, h): (u32, u32)| (w > 0 && h > 0).then(|| w as f32 / h as f32);
    // The open frame's own develop: exactly the picture the masks
    // are in, and already the way up the frame's turn puts it.
    if st.current == Some(file)
        && st.cull.is_none()
        && let Some(a) = aspect(st.source_size)
    {
        return Some(a);
    }
    // The file itself, which is the same frame however it is shown.
    if let Some(a) = stance(st, file)
        .and_then(|s| s.shown_size(turn))
        .and_then(aspect)
    {
        return Some(a);
    }
    // Failing that, a picture of it the window already has. These
    // are turned by the camera's tag alone, so the frame's own turn
    // goes on top.
    let shown = |(w, h): (u32, u32)| {
        let (w, h) = if turn % 2 == 1 { (h, w) } else { (w, h) };
        aspect((w, h))
    };
    if let Some(cull) = st.cull.as_ref().or(st.hold.as_ref())
        && let Some(preview) = cull.cache.best(file)
        && let Some(a) = shown(preview.source)
    {
        return Some(a);
    }
    st.thumb_base
        .get(file)
        .and_then(|t| t.as_ref())
        .and_then(|(w, h, _)| shown((*w, *h)))
}

/// Finish a sidecar's version 4 migration, now that the frame's
/// shape can be had: see [`greycard_edit::Edit::migrate_with_frame`].
///
/// The folder's scan loads every sidecar with no frame to measure
/// by, so an `Original` crop is left part-migrated and reads, renders
/// and saves back as the version 3 it still is. This is called the
/// first moment a frame is in hand and before anything can act on
/// its geometry: as it is selected, as it is culled, and before a
/// turn maps it. The shape comes from [`frame_aspect`], so it is the
/// same picture the masks are in, and the file's own answer is a
/// cached metadata read rather than a decode.
///
/// A frame nothing can measure is left as it is and tried again next
/// time; the alternative is guessing which way up it is, and a guess
/// here re-crops the picture.
pub(crate) fn migrate_frame(st: &mut State, file: usize) {
    if !st.sidecars.get(file).is_some_and(Sidecar::needs_frame) {
        return;
    }
    match frame_aspect(st, file) {
        Some(aspect) => st.sidecars[file].migrate_with_frame(aspect < 1.0),
        None => tracing::warn!(
            "{}: nothing can say which way up it is yet, so its Original crop \
             waits before it is brought up to date",
            file_name(&st.files[file])
        ),
    }
}

/// The browser's row of a file, when the filter shows it.
pub(crate) fn row_of(st: &State, file: usize) -> Option<usize> {
    st.shown.binary_search(&file).ok()
}

/// The folder as the filter reads it: each file beside the meta its
/// sidecar holds. Nothing is read from disk for this — the sidecars
/// of the open folder are already in hand, which is why the filter
/// needs no index.
pub(crate) fn filter_frames(st: &State) -> Vec<filter::Frame<'_>> {
    st.files
        .iter()
        .zip(&st.sidecars)
        .map(|(path, s)| filter::Frame {
            path,
            meta: &s.meta,
        })
        .collect()
}

/// Whether the filter shows one file, as it stands now.
pub(crate) fn filter_shows(st: &State, file: usize) -> bool {
    match (st.files.get(file), st.sidecars.get(file)) {
        (Some(path), Some(s)) => st.filter.shows(filter::Frame {
            path,
            meta: &s.meta,
        }),
        _ => false,
    }
}

/// How a file's picture is turned in the browser and the loupe: the
/// open file's as the panel has it, another's as its sidecar does,
/// and the frame's own quarter turns folded into both.
///
/// The pictures these three draw are the camera's own, turned by the
/// camera's own tag and no further, so a frame's turn is folded into
/// the quarter turns rather than the picture being turned again: see
/// [`Geometry::shown_turns`]. That is what makes a turn cost a
/// redraw here and not a decode.
pub(crate) fn thumb_turns(st: &State, app: &App, file: usize) -> (u8, bool) {
    let turn = st.sidecars[file].turn;
    if st.current == Some(file) && st.cull.is_none() {
        read_geometry(app).shown_turns(turn)
    } else {
        st.sidecars[file].current.geometry.shown_turns(turn)
    }
}

/// How many frames of the folder are flagged reject.
pub(crate) fn reject_count(st: &State) -> usize {
    st.sidecars
        .iter()
        .filter(|s| s.meta.flag == meta::Flag::Reject)
        .count()
}

/// The browser's list again: the filter over the flags, the strip's
/// and the grid's rows made afresh with the pictures already made,
/// and the selection kept on its row, or put on the nearest row when
/// its frame is hidden (which the caller then opens).
pub(crate) fn rebuild_browser(st: &mut State, app: &App) -> Option<usize> {
    st.shown = st.filter.apply(&filter_frames(st));
    show_filter(st, app);
    let thumbs = Rc::new(VecModel::<Thumb>::default());
    for &f in &st.shown {
        thumbs.push(thumb_for(&st.files[f], &st.sidecars[f].meta));
    }
    app.set_thumbs(ModelRc::from(thumbs));
    for row in 0..st.shown.len() {
        let f = st.shown[row];
        if st.thumb_base[f].is_some() {
            let (turns, flip) = thumb_turns(st, app, f);
            show_thumb(st, app, f, turns, flip);
        }
    }
    app.set_reject_count(reject_count(st) as i32);
    // A frame the filter now hides leaves the set: a key or a sync
    // must not reach a frame nobody can see is chosen.
    st.picked = selection::prune(&st.picked, &st.shown, st.current);
    show_set(st, app);
    let Some(c) = st.current else {
        app.set_selected(-1);
        return None;
    };
    match row_of(st, c) {
        Some(row) => {
            app.set_selected(row as i32);
            None
        }
        None => {
            app.set_selected(-1);
            cull::nearest_row(&st.shown, c)
        }
    }
}

/// `grid_filled` over the browser's rows, with the state's fields
/// borrowed apart.
pub(crate) fn grid_filled_rows(
    shown: &[usize],
    thumb_made: &[u32],
    thumb_want: u32,
    grid_shown: Option<(i32, i32)>,
    app: &App,
) -> bool {
    if !app.get_grid_open() {
        return true;
    }
    let Some((first, last)) = grid_shown else {
        return false;
    };
    (first.max(0) as usize..=last.max(0) as usize)
        .filter_map(|r| shown.get(r))
        .all(|&f| thumb_made[f] > 0 && !grid::wants_bigger(thumb_made[f], thumb_want))
}

/// Each file's edit, from its sidecar when there is one; the fresh
/// edit when sidecars are off, or none is there yet. Beside them,
/// which files are raws whose edit is still the default: their
/// learned-denoiser blend is seeded from the ISO by the worker on
/// their first open, since reading the ISO here would cost a
/// metadata probe a file (3 to 40 ms) on the launch of every folder.
///
/// What decides that is the sidecar's contents, not whether there is
/// one: a frame given a star in the browser has a sidecar with a
/// default edit in it, and it should still open at the blend its ISO
/// asks for.
pub(crate) fn load_sidecars(files: &[PathBuf], write_sidecars: bool) -> (Vec<Sidecar>, Vec<bool>) {
    files
        .iter()
        .map(|f| {
            if !write_sidecars {
                return (Sidecar::default(), false);
            }
            // A picture that is not a raw starts from its own default.
            let fresh = || {
                if greycard_core::picture::is_picture_path(f) {
                    (
                        Sidecar {
                            current: Edit::for_picture(),
                            ..Sidecar::default()
                        },
                        false,
                    )
                } else {
                    (Sidecar::default(), true)
                }
            };
            let (mut sidecar, seed) = match Sidecar::load(f) {
                Ok(Some(s)) => {
                    let raw = !greycard_core::picture::is_picture_path(f);
                    let seed = raw && files::never_developed(&s);
                    (s, seed)
                }
                Ok(None) => fresh(),
                Err(e) => {
                    tracing::warn!(
                        "{}: sidecar: {e}; starting from the default edit",
                        file_name(f)
                    );
                    fresh()
                }
            };
            // What an XMP beside the frame has to say, when it has
            // changed since the sidecar last took its word. Reading
            // one is not behind the setting: writing an XMP is a
            // choice about somebody else's folder, believing one
            // that is already there is not. Nothing is written
            // here — the meta and the mark ride in memory until the
            // frame's next real save — so opening a folder of
            // somebody else's raws deposits nothing in it.
            // The camera's tag, for the one field of a packet that
            // is measured against it. The closure is called only for
            // a packet that carries `tiff:Orientation` at all, so a
            // folder whose XMPs are silent about it opens without
            // reading a byte of any raw.
            let took = xmp::adopt(f, &mut sidecar, || {
                greycard_core::decode::orientation_path(f)
                    .inspect_err(|e| tracing::warn!("{}: orientation: {e}", file_name(f)))
                    .ok()
            });
            // A turn taken from somebody else's file turns the
            // picture under the user without their having pressed
            // anything, and records no state to undo it with, so it
            // is said out loud and by name.
            if took.turned.is_some() {
                tracing::info!(
                    "{}: the XMP beside it says the frame stands {}; turned to match",
                    file_name(f),
                    // Where it now stands, not how far it moved to
                    // get there: a reader of the log wants the frame,
                    // not the delta.
                    turn_words(i32::from(sidecar.turn))
                );
                if took.left_placed {
                    tracing::warn!(
                        "{}: its masks and repair patches stay where they are on the \
                         screen, since nothing is decoded at a folder's opening to \
                         measure the frame by",
                        file_name(f)
                    );
                }
            }
            (sidecar, seed)
        })
        .unzip()
}

/// Replace the file list with `dir`'s files, the same scan the launch
/// path does, and select the last file open if it lies there, else
/// the first. An empty folder is left alone rather than emptying the
/// strip under the user's feet.
pub(crate) fn open_folder(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    let Ok(files) = files::list_files(dir) else {
        tracing::warn!("{}: not a folder", dir.display());
        app.set_status(format!("{}: not a folder", dir.display()).into());
        return;
    };
    if files.is_empty() {
        tracing::warn!("no pictures in {}", dir.display());
        app.set_status(format!("no pictures in {}", dir.display()).into());
        return;
    }
    let last = settings::Settings::load().last_file;
    let last = (!last.is_empty()).then(|| PathBuf::from(last));
    let select = files::select_index(&files, last.as_deref());
    open_files(state, app, worker, files, select);
}

/// What the desktop asked to open, the Finder's double-click or Open
/// With: each path as the command line's path would be, a file on its
/// own and a folder's pictures, starting on the first.
pub(crate) fn open_paths(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    paths: &[PathBuf],
) {
    let (files, refused) = files::list_paths(paths);
    for e in &refused {
        tracing::warn!("{e:#}");
    }
    if files.is_empty() {
        let said = refused
            .first()
            .map_or_else(|| "nothing to open".to_string(), |e| format!("{e:#}"));
        app.set_status(said.into());
        return;
    }
    open_files(state, app, worker, files, 0);
}

/// A new list of files in the browser, `select` opened.
fn open_files(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    files: Vec<PathBuf>,
    select: usize,
) {
    let mut st = state.borrow_mut();
    // Leave the old file as a normal selection would: its edit saved,
    // its picture held on screen until the new one develops. In
    // culling the panel is not the frame's, and there is nothing to
    // save; the mode stays on for the new folder.
    if st.current.is_some() && st.cull.is_none() {
        let outgoing = read_edit(app, &st.edit, st.target);
        if st.base_white.is_some() {
            st.held = Some(outgoing.clone());
        } else {
            st.zoom = 0.0;
        }
        save_edit(&mut st, outgoing);
    }
    if let Some(cull) = st.cull.as_mut() {
        cull.cache.clear();
        cull.textures.clear();
        cull.failed.clear();
        cull.anchor = 0;
    }
    // Another folder's files, and the pictures kept are keyed by the
    // last one's numbering.
    drop_placeholder(&mut st, app);
    st.hold = None;
    st.prefetch.want(Vec::new());
    st.current = None;
    st.picked.clear();
    let (sidecars, seed_blend) = load_sidecars(&files, st.write_sidecars);
    st.sidecars = sidecars;
    st.seed_blend = seed_blend;
    st.thumb_base = vec![None; files.len()];
    st.thumb_shown = vec![None; files.len()];
    // A folder of its own: the grid's zoom does not carry its
    // appetite for large pictures over to it.
    st.thumb_made = vec![0; files.len()];
    st.thumb_asked = vec![worker::THUMB_WIDTH; files.len()];
    st.thumb_want = worker::THUMB_WIDTH;
    st.grid_shown = None;
    worker.set_thumb_size(worker::THUMB_WIDTH);
    st.files = files.clone();
    rebuild_browser(&mut st, app);
    // The file to open, as a row of the list; hidden by the filter,
    // the nearest one shown.
    let row = row_of(&st, select).or_else(|| cull::nearest_row(&st.shown, select));
    drop(st);

    for (i, f) in files.iter().enumerate() {
        worker.send(Job::Thumbnail {
            index: i,
            path: f.clone(),
        });
    }
    if let Some(row) = row {
        app.invoke_select(row as i32);
    } else {
        tracing::warn!("no frames pass the filter");
        app.set_status(filter::NOTHING_SHOWN.into());
    }
}

/// Whether every frame the grid shows has its picture, at the size
/// the cells ask for. True whenever the grid is closed.
pub(crate) fn grid_filled(st: &State, app: &App) -> bool {
    grid_filled_rows(&st.shown, &st.thumb_made, st.thumb_want, st.grid_shown, app)
}

/// A file's name without its directory, for the panel and the log.
/// The mean, least and greatest of what a timed run measured, over
/// the steps alone: the first sample is the file the editor opened
/// on, decoded from nothing, and the rest are steps along the folder.
fn over_steps(what: &str, samples: &[f64]) -> String {
    let steps = &samples[1.min(samples.len())..];
    if steps.is_empty() {
        return format!("{what}: nothing measured");
    }
    let n = steps.len() as f64;
    let mean = steps.iter().sum::<f64>() / n;
    let min = steps.iter().copied().fold(f64::INFINITY, f64::min);
    let max = steps.iter().copied().fold(0.0, f64::max);
    format!(
        "{what} over {} steps: mean {mean:.1} ms, min {min:.1}, max {max:.1}; the first file {:.1} ms",
        steps.len(),
        samples.first().copied().unwrap_or(0.0)
    )
}

/// `--time-select`, on each frame that shows a stepped-to develop:
/// the milliseconds since the key, then the next step a tenth of a
/// second on, as a hand would arrow; after the last, the numbers on
/// the log and the terminal, and the editor quits. `time_cull`'s
/// twin, for the develop view, and it reports two numbers a step:
/// the key to the camera's picture standing in, and the key to the
/// develop that replaces it.
pub(crate) fn time_select(
    timing: &mut Option<(u32, Vec<f64>, Vec<f64>)>,
    app: &App,
    ms: f64,
    row: Option<usize>,
    count: usize,
) {
    let Some((left, to_picture, to_develop)) = timing.as_mut() else {
        return;
    };
    to_develop.push(ms);
    let at_end = row.is_some_and(|r| r + 1 >= count);
    if *left == 0 || at_end {
        for line in [
            over_steps("select to the camera picture", to_picture),
            over_steps("select to develop", to_develop),
        ] {
            tracing::info!("{line}");
            eprintln!("{line}");
        }
        *timing = None;
        let _ = slint::quit_event_loop();
        return;
    }
    *left -= 1;
    let app_weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(100), move || {
        if let Some(app) = app_weak.upgrade() {
            app.invoke_step(1, false);
        }
    });
}

pub(crate) fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Open the frame on browser row `row`: decode and develop it on the
/// worker, or in culling show its JPEG. It becomes the current frame;
/// the set becomes that frame alone, or with `extend` (Shift and an
/// arrow) keeps what it held and takes this frame as well.
pub(crate) fn open_row(st: &mut State, app: &App, worker: &Worker, row: i32, extend: bool) {
    // The window's rows are the browser's list, which the
    // filter may have shortened; the file is what is opened.
    let Some(&i) = usize::try_from(row).ok().and_then(|r| st.shown.get(r)) else {
        return;
    };
    st.picked = if extend {
        selection::union(&chosen_frames(st), &[i])
    } else {
        vec![i]
    };
    // `--cull`: the first file opens the mode, not a develop.
    if let Some(compare) = st.cull_at_start.take() {
        enter_cull(st, app, compare);
    }
    // In culling nothing is developed: the frame's JPEG shows.
    if st.cull.is_some() {
        cull_select(st, app, i);
        show_set(st, app);
        return;
    }
    // The panel is the file's: keep the old one's, show the new one's.
    // The viewport keeps the old picture under the old look
    // until the new frame's camera JPEG is decoded, which
    // stands in until its develop lands; a file chosen from
    // the strip opens fitted either way.
    if st.current.is_some() {
        let outgoing = read_edit(app, &st.edit, st.target);
        if st.base_white.is_some() {
            st.held = Some(outgoing.clone());
        } else {
            st.zoom = 0.0;
        }
        save_edit(st, outgoing);
    }
    // Before the edit reaches the panel, and so before any
    // refit can run on it: a sidecar from an older build
    // does not know which way up its Original crop goes
    // until the frame does.
    migrate_frame(st, i);
    let edit = st.sidecars[i].current.clone();
    // A mask asked for on the command line is the first file's
    // target, so a screenshot can show the panel's block for it.
    st.target = if st.current.is_none() {
        let m = st.show_mask.take().filter(|&m| m < edit.adjustments.len());
        if m.is_some() {
            app.set_show_mask(true);
        }
        // Likewise a patch, which the panel's list then shows
        // chosen and the viewport outlines.
        if let Some(p) = st.show_patch.take() {
            app.set_patch(p as i32);
        }
        m
    } else {
        None
    };
    st.placing = None;
    app.set_placing("".into());
    // Another picture: a guide stroke drawn on the last one
    // means nothing here, and the source's size is not known
    // again until this frame's develop lands — a turn before
    // that must not map this frame's masks by the last
    // frame's shape.
    st.guiding.clear();
    st.source_size = (0, 0);
    app.set_guide_mode("".into());
    show_edit(st, &edit, app, st.target);
    st.edit = edit.clone();
    st.current = Some(i);
    st.generation += 1;
    // The key's moment, until the frame that shows this
    // frame: the log's line, and `--time-select`.
    st.selected_at = Some(std::time::Instant::now());
    app.set_selected(row);
    app.set_file_name(file_name(&st.files[i]).into());
    // The last file's shot is not this one's; the open says
    // what this one was.
    app.set_shot_camera("".into());
    app.set_shot_exposure("".into());
    app.set_shot_size("".into());
    app.set_status("decoding...".into());
    app.set_busy(true);
    show_history(st, app);
    worker.send(Job::Open {
        path: st.files[i].clone(),
        edit,
        generation: st.generation,
        seed_blend: st.seed_blend.get(i).copied().unwrap_or(false),
        turn: st.sidecars[i].turn,
    });
    // The frame's own camera JPEG, to stand in for that
    // develop: after the job, so the decode that matters
    // most is under way first.
    start_placeholder(st, app, i);
    show_set(st, app);
}

/// An arrow's landing on browser row `row`, which `moves` when it is
/// not the row already current. A plain arrow opens it as a click
/// would and the set collapses to it, even at the end of the strip
/// where nothing moved; with Shift the set keeps what it had and
/// takes the frame landed on as well.
fn step_to(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Worker,
    row: i32,
    moves: bool,
    extend: bool,
) {
    match (moves, extend) {
        (true, false) => app.invoke_select(row),
        (true, true) => open_row(&mut state.borrow_mut(), app, worker, row, true),
        (false, false) => {
            let mut st = state.borrow_mut();
            st.picked = st.current.into_iter().collect();
            show_set(&mut st, app);
        }
        (false, true) => {}
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // The culling keys: a rating, a flag or a color label on the
    // selection. None of this is an edit, so nothing here records a
    // history state, marks the panel dirty or asks for a develop.
    // A key that names what a frame already carries is still the
    // browser's key and is still eaten: it just writes nothing.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_meta_key(move |key| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(change) = meta::Change::from_key(&key) else {
                return false;
            };
            let mut st = state.borrow_mut();
            // The whole selection: the current frame and the set.
            let frames = chosen_frames(&st);
            if frames.is_empty() {
                return false;
            }
            let next = set_meta(&mut st, &app, &frames, change);
            drop(st);
            // The frame left the filtered list: on to the nearest.
            if let Some(row) = next {
                app.invoke_select(row as i32);
            }
            true
        });
    }
    // The frame's own turn, [ and ], and the panel's two buttons:
    // on the selection, as the meta keys are, and in the loupe, the
    // grid and culling alike.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_frame_turned(move |quarters| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // The whole selection, as the meta keys have it.
            let frames = chosen_frames(&st);
            turn_frames(&mut st, &app, &worker, &frames, quarters);
        });
    }
    // Selecting a file: decode and develop it on the worker.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_select(move |row| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            open_row(&mut state.borrow_mut(), &app, &worker, row, false);
        });
    }
    // Arrow keys step along the strip: the set collapses to the
    // frame landed on, or with Shift held takes it as well.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_step(move |by, extend| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // Along the browser's rows, which the filter may have
            // shortened; a selection the filter hid steps from the
            // nearest row shown.
            let (row, count) = {
                let st = state.borrow();
                let row = usize::try_from(app.get_selected())
                    .ok()
                    .or_else(|| st.current.and_then(|c| cull::nearest_row(&st.shown, c)));
                (row, st.shown.len())
            };
            let (Some(row), true) = (row, count > 0) else {
                return;
            };
            let next = (row as i64 + by as i64).clamp(0, count as i64 - 1) as usize;
            step_to(
                &state,
                &app,
                &worker,
                next as i32,
                next != row || app.get_selected() < 0,
                extend,
            );
        });
    }
    // A click on a frame in the strip or the grid: a plain one opens
    // it, a Ctrl or a Shift one changes the set about the frame on
    // screen and leaves it there.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_frame_clicked(move |row, ctrl, shift| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(&file) = usize::try_from(row).ok().and_then(|r| st.shown.get(r)) else {
                return;
            };
            match selection::click(&st.picked, &st.shown, st.current, file, ctrl, shift) {
                // A plain click on the frame already on screen: the
                // set goes back to it, and nothing is opened again.
                selection::Click::Open(f) if Some(f) == st.current => {
                    st.picked = vec![f];
                    show_set(&mut st, &app);
                }
                selection::Click::Open(_) => {
                    drop(st);
                    app.invoke_select(row);
                }
                selection::Click::Set(set) => {
                    st.picked = set;
                    show_set(&mut st, &app);
                }
            }
        });
    }
    // Escape over a set: back to the frame on screen.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_set_collapsed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.picked = st.current.into_iter().collect();
            show_set(&mut st, &app);
        });
    }
    // The frames the strip shows: their pictures jump the worker's
    // thumbnail queue, so a scroll into unseen ground fills in
    // seconds rather than after the whole folder. Reported on every
    // move of the strip; the worker drops a repeat.
    {
        let (state, worker) = (state.clone(), worker.clone());
        app.on_strip_range(move |first, last| {
            if first < 0 || last < first {
                return;
            }
            // Rows to files: the range between the two, which under a
            // filter is a superset of what is shown, and fine for an
            // order.
            let st = state.borrow();
            let (Some(&f), Some(&l)) = (st.shown.get(first as usize), st.shown.get(last as usize))
            else {
                return;
            };
            worker.want_thumbnails(f, l);
        });
    }
    // The grid's layout, asked of the pure arithmetic in `grid` so
    // the numbers are tested rather than left in a binding.
    app.on_grid_columns(grid::columns);
    app.on_grid_slack(grid::slack);
    app.on_grid_max_scroll(grid::max_scroll);
    app.on_grid_reveal_to(grid::reveal);
    // An arrow in the grid: along the row, or by a whole row, and
    // the frame it lands on is opened as a click on it would.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_grid_step(move |dx, dy, columns, extend| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let count = state.borrow().shown.len() as i32;
            let next = grid::step(app.get_selected(), dx, dy, columns, count);
            if next >= 0 {
                step_to(
                    &state,
                    &app,
                    &worker,
                    next,
                    next != app.get_selected(),
                    extend,
                );
            }
        });
    }
    // Ctrl and the wheel, or the plus and minus keys: the cell takes
    // a step and the grid re-flows.
    {
        let app_weak = app.as_weak();
        app.on_grid_zoom(move |by| {
            if let Some(app) = app_weak.upgrade() {
                app.set_grid_cell(grid::zoom(app.get_grid_cell(), by));
            }
        });
    }
    // The frames the grid shows, from where it is scrolled: the
    // strip's mechanism over a rectangle rather than a row. A folder
    // of hundreds is never rendered up front; what is on screen
    // jumps the queue, and a cell larger than the pictures were made
    // for asks for those on screen again at the larger size.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_grid_range(move |scroll, height, columns| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let cell = app.get_grid_cell();
            let mut st = state.borrow_mut();
            let count = st.shown.len() as i32;
            let Some((first, last)) = grid::visible(scroll, height, cell, columns, count) else {
                return;
            };
            st.grid_shown = Some((first, last));
            let (first, last) = (
                first.max(0) as usize,
                (last.max(0) as usize).min(st.shown.len().saturating_sub(1)),
            );
            // What is made from now on is this cell's size, up or
            // down: a cell that has shrunk should not go on paying
            // the memory of the one before it.
            let want = grid::render_size(cell);
            if want != st.thumb_want {
                st.thumb_want = want;
                worker.set_thumb_size(want);
            }
            // Making one again is the part that is only ever a step
            // up: only pictures already on screen and a quarter too
            // small, and only once each. One still in the queue will
            // come back at the size the worker now has anyway, and
            // asking again would only make it twice.
            for row in first..=last {
                let i = st.shown[row];
                if st.thumb_made[i] > 0
                    && grid::wants_bigger(st.thumb_made[i], want)
                    && st.thumb_asked[i] < want
                {
                    st.thumb_asked[i] = want;
                    worker.send(Job::Thumbnail {
                        index: i,
                        path: st.files[i].clone(),
                    });
                }
            }
            // After the pushes, which clear the queue's order.
            worker.want_thumbnails(st.shown[first], st.shown[last]);
        });
    }
    // The desktop's folder chooser, for the Open folder button and
    // Ctrl+O: the file list becomes that folder's, the last file
    // open selected if it lies there, else the first.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_open_folder(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let start = {
                let st = state.borrow();
                st.current
                    .and_then(|i| st.files.get(i))
                    .and_then(|f| f.parent())
                    .map(Path::to_path_buf)
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/"))
            };
            let app_weak = app.as_weak();
            app.set_status("choosing a folder to open...".into());
            export::choose_folder("Open folder", start, move |chosen| {
                let app_weak = app_weak.clone();
                // The state and the worker through their thread
                // locals, as this runs on the chooser's own thread and
                // an `Rc` cannot cross it; `choose_open`'s importer
                // does the same.
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    let (Some(state), Some(worker)) = (
                        STATE.with(|s| s.borrow().clone()),
                        WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    match chosen {
                        Ok(Some(dir)) => open_folder(&state, &app, &worker, &dir),
                        Ok(None) => app.set_status("open folder canceled".into()),
                        Err(e) => {
                            tracing::warn!("file chooser: {e:#}");
                            app.set_status("the desktop offered no file chooser".into());
                        }
                    }
                });
            });
        });
    }
}

/// The window's keys, on the headless backend: what the grid does
/// with them cannot be reached from a unit test of `grid` alone, and
/// there is no way to press a key at a Wayland session from here.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{press, window};
    use slint::platform::{Key, WindowEvent};

    /// A row of the browser and a file of the folder are two
    /// different numbers whenever a filter is on. The badges wrote
    /// the file's number into the list, so a star set under "No
    /// rejects" landed on whichever frame happened to be that many
    /// rows down — someone else's thumbnail, not an index off the
    /// end, which is why nothing ever panicked over it.
    #[test]
    fn a_badge_set_under_a_filter_lands_on_the_filtered_row() {
        use meta::{Change, Flag, Label};
        let app = window(0);
        let files: Vec<PathBuf> = ["a.CR3", "b.CR3", "c.CR3", "d.CR3", "e.CR3"]
            .iter()
            .map(|n| PathBuf::from("/nowhere").join(n))
            .collect();
        let mut st = State::empty(files, &app);
        // One reject at the front, so every row is the file one
        // along: row 0 is file 1, and the file numbers 1 to 4 are
        // all rows that exist. The old code wrote file 1's badge at
        // row 1, which is file 2 — a neighbour, in range, wrong.
        st.sidecars[0].meta.flag = Flag::Reject;
        st.filter = filter::Filter::from_name("No rejects").expect("it parses");
        rebuild_browser(&mut st, &app);
        assert_eq!(st.shown, vec![1, 2, 3, 4]);
        assert_eq!(app.get_thumbs().row_count(), 4);

        // Four stars on file 1: row 0, and nothing on row 1.
        set_meta(&mut st, &app, &[1], Change::Rating(4));
        let rows = app.get_thumbs();
        assert_eq!(rows.row_data(0).unwrap().name, "b.CR3");
        assert_eq!(rows.row_data(0).unwrap().rating, 4, "the file's own row");
        assert_eq!(
            rows.row_data(1).unwrap().rating,
            0,
            "and not its neighbour's"
        );
        assert!(
            (2..4).all(|r| rows.row_data(r).unwrap().rating == 0),
            "and nobody else's"
        );

        // A label on the last file shown, which is file 4 at row 3.
        set_meta(&mut st, &app, &[4], Change::Label(Label::Red));
        let rows = app.get_thumbs();
        assert_eq!(rows.row_data(3).unwrap().name, "e.CR3");
        assert_eq!(rows.row_data(3).unwrap().label, Label::Red.code());
        assert!(
            (0..3).all(|r| rows.row_data(r).unwrap().label == 0),
            "the label went to one row"
        );
        // And the star set a moment ago is still where it was put.
        assert_eq!(rows.row_data(0).unwrap().rating, 4);
    }

    #[test]
    fn a_rating_from_another_tool_arrives_and_the_folder_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("greycard-xmp-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let theirs = dir.join("IMG_0001.CR3");
        let mine = dir.join("IMG_0002.CR3");
        for f in [&theirs, &mine] {
            std::fs::write(f, b"raw").expect("a file");
        }
        let three = Meta {
            rating: 3,
            ..Meta::default()
        };

        // Rated in Lightroom, never opened here: the XMP is all
        // there is, so its word is taken.
        std::fs::write(xmp::short_path(&theirs), xmp::fresh(&three, None)).expect("an xmp");
        let (sidecars, _) = load_sidecars(std::slice::from_ref(&theirs), true);
        assert_eq!(sidecars[0].meta, three);
        // And nothing was written into somebody else's folder for
        // it: the meta rides in memory until the frame is saved.
        assert!(
            !Sidecar::path_for(&theirs).exists(),
            "opening a folder deposits nothing"
        );

        // Cleared here after the XMP was taken, and saved: the XMP
        // still says three and is older than nothing in particular,
        // but it is the same XMP that was read, so it has nothing
        // new to say and the three do not come back.
        let mut sidecar = Sidecar::default();
        std::fs::write(xmp::short_path(&mine), xmp::fresh(&three, None)).expect("an xmp");
        assert!(greycard_edit::xmp::adopt(&mine, &mut sidecar, || None).moved());
        sidecar.meta.set_rating(0);
        sidecar.save(&mine).expect("the sidecar writes");
        set_modified(&xmp::short_path(&mine), 2_000_000_000);
        set_modified(&Sidecar::path_for(&mine), 1_000_000_000);
        assert_eq!(
            load_sidecars(std::slice::from_ref(&mine), true).0[0]
                .meta
                .rating,
            0,
            "a newer mtime is not a newer opinion"
        );

        // With `--no-sidecars` neither file is read at all.
        assert_eq!(
            load_sidecars(&[theirs, mine], false).0[0].meta,
            Meta::default()
        );
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
    }

    /// A file's modification time, set, so the newer-wins rule can
    /// be tested without sleeping through a filesystem's resolution.
    fn set_modified(path: &Path, secs: u64) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("the file opens")
            .set_modified(t)
            .expect("the time is set");
    }

    #[test]
    fn the_badges_read_the_meta() {
        use meta::{Flag, Label};
        let meta = Meta {
            rating: 200,
            flag: Flag::Reject,
            label: Label::Purple,
            ..Meta::default()
        };
        // A rating past the range does not draw two hundred stars.
        let thumb = thumb_for(Path::new("/tmp/IMG_0001.CR3"), &meta);
        assert_eq!(thumb.name, "IMG_0001.CR3");
        assert_eq!(thumb.rating, 5);
        assert_eq!(thumb.flag, 2);
        assert_eq!(thumb.label, 5);
        let bare = thumb_for(Path::new("/tmp/a.CR3"), &Meta::default());
        assert_eq!((bare.rating, bare.flag, bare.label), (0, 0, 0));
        assert_eq!(Flag::Pick.code(), 1);
        assert_eq!(Label::Red.code(), 1);
        assert_eq!(Label::Blue.code(), 4);
    }

    /// The filmstrip's picture, the grid's and the culling loupe's are
    /// the camera's own, so a frame's turn rides in the same quarter
    /// turns the edit's do: what comes out is the picture the develop
    /// would make of the turned frame.
    #[test]
    fn a_thumbnail_follows_the_frames_own_turn() {
        use greycard_core::WorkingImage;
        use greycard_core::develop::orient;
        use greycard_core::raw::Orientation;
        let rgb: Vec<u8> = (0..6).flat_map(|i| [i, i, i]).collect();
        let plane = |w: u32, h: u32, rgb: &[u8]| {
            WorkingImage::from_data(
                w as usize,
                h as usize,
                rgb.iter().map(|&b| b as f32).collect(),
            )
            .unwrap()
        };
        for geometry in [
            Geometry::default(),
            Geometry {
                turns: 1,
                ..Geometry::default()
            },
            Geometry {
                flip: true,
                ..Geometry::default()
            },
            Geometry {
                turns: 3,
                flip: true,
                ..Geometry::default()
            },
        ] {
            for turn in 0..4u8 {
                // The develop: the source turned by the frame's own
                // quarter turns, then the edit's geometry on top.
                let turned = orient(
                    plane(3, 2, &rgb),
                    Orientation::from_exif(match turn {
                        1 => 6,
                        2 => 3,
                        3 => 8,
                        _ => 1,
                    })
                    .unwrap(),
                );
                let bytes: Vec<u8> = turned.data.iter().map(|&v| v as u8).collect();
                let under = geometry.under_turned_source(i32::from(turn));
                let want = greycard_edit::geometry::turn_pixels(
                    turned.width as u32,
                    turned.height as u32,
                    &bytes,
                    under.turns,
                    under.flip,
                );
                // The strip: the camera's picture, with the turn
                // folded into the quarter turns.
                let (turns, flip) = under.shown_turns(turn);
                let got = greycard_edit::geometry::turn_pixels(3, 2, &rgb, turns, flip);
                assert_eq!(got, want, "{geometry:?} + {turn}");
            }
        }
    }

    #[test]
    fn g_opens_the_grid_and_three_keys_leave_it() {
        let app = window(11);
        assert!(!app.get_grid_open());
        press(&app, "g");
        assert!(app.get_grid_open());
        press(&app, Key::Return);
        assert!(!app.get_grid_open());
        press(&app, "G");
        assert!(app.get_grid_open());
        press(&app, Key::Escape);
        assert!(!app.get_grid_open());
        press(&app, "g");
        press(&app, "g");
        assert!(!app.get_grid_open());
    }

    #[test]
    fn the_arrows_step_the_strip_in_one_dimension_and_the_grid_in_two() {
        let app = window(11);
        let steps = Rc::new(RefCell::new(Vec::new()));
        let flat = steps.clone();
        app.on_step(move |by, _| flat.borrow_mut().push((by, 0)));
        let two = steps.clone();
        app.on_grid_step(move |dx, dy, cols, _| {
            assert!(cols >= 1, "the grid always has a column");
            two.borrow_mut().push((dx, dy));
        });
        // In the loupe the arrows are the strip's, along the folder.
        press(&app, Key::LeftArrow);
        press(&app, Key::RightArrow);
        press(&app, Key::UpArrow);
        assert_eq!(*steps.borrow(), vec![(-1, 0), (1, 0)]);
        steps.borrow_mut().clear();
        // In the grid they are the grid's, and the vertical pair
        // reaches it too.
        press(&app, "g");
        press(&app, Key::LeftArrow);
        press(&app, Key::RightArrow);
        press(&app, Key::UpArrow);
        press(&app, Key::DownArrow);
        assert_eq!(*steps.borrow(), vec![(-1, 0), (1, 0), (0, -1), (0, 1)]);
    }

    #[test]
    fn g_at_a_sized_window_reads_the_sheet_it_is_born_at() {
        // Opened by G with the window already at its size, the sheet
        // is created at that size and `changed width` has nothing to
        // say; the first release laid one column down the left.
        let app = window(120);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let reports = seen.clone();
        app.on_grid_range(move |scroll, height, cols| {
            reports.borrow_mut().push((scroll, height, cols));
        });
        app.set_selected(0);
        press(&app, "g");
        assert!(
            app.get_grid_w() > 1000.0,
            "sheet width {}",
            app.get_grid_w()
        );
        assert!(
            app.get_grid_h() > 500.0,
            "sheet height {}",
            app.get_grid_h()
        );
        let cols = grid::columns(app.get_grid_w(), 176.0);
        assert_eq!(cols, 8);
        assert_eq!(seen.borrow().last().map(|r| r.2), Some(8));
    }

    #[test]
    fn the_grid_lays_out_and_reports_what_it_shows() {
        let app = window(120);
        // The sheet a 1500 by 950 window leaves.
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let reports = seen.clone();
        app.on_grid_range(move |scroll, height, cols| {
            reports.borrow_mut().push((scroll, height, cols));
        });
        let steps = Rc::new(RefCell::new(0));
        let cols = steps.clone();
        app.on_grid_step(move |_, _, c, _| *cols.borrow_mut() = c);
        app.set_selected(0);
        press(&app, "g");
        press(&app, Key::DownArrow);
        // Eight 176 cells across 1500, rows 206 apart: under a
        // header of two rows and the filter's chips the sheet shows
        // four whole rows and a sliver of a fifth, so the first
        // forty frames are what the worker is told to make first.
        assert_eq!(*steps.borrow(), 8);
        assert_eq!(*seen.borrow(), vec![(0.0, 837.0, 8)]);
        assert_eq!(grid::visible(0.0, 837.0, 176.0, 8, 120), Some((0, 39)));
        // A frame at the foot of the sheet scrolls it there, and what
        // it says it shows follows the scroll.
        seen.borrow_mut().clear();
        app.set_selected(119);
        // Slint runs the changed handlers with the next event, which
        // in a window is the next frame.
        press(&app, Key::Shift);
        let scrolled = grid::reveal(0.0, 837.0, 176.0, 8, 120, 119);
        assert_eq!(scrolled, grid::max_scroll(837.0, 176.0, 8, 120));
        assert_eq!(*seen.borrow(), vec![(scrolled, 837.0, 8)]);
        assert_eq!(
            grid::visible(scrolled, 837.0, 176.0, 8, 120),
            Some((80, 119))
        );
    }

    #[test]
    fn a_sheet_owns_the_keys_over_the_grid_and_a_new_folder_re_flows_it() {
        let app = window(120);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        app.set_selected(0);
        press(&app, "g");
        assert!(app.get_grid_open());
        // A sheet is drawn over the grid, so the keys are its own:
        // neither G nor Return reaches the grid under it.
        app.set_export_open(true);
        press(&app, "g");
        press(&app, Key::Return);
        assert!(app.get_grid_open());
        app.set_export_open(false);
        press(&app, "g");
        assert!(!app.get_grid_open());
        // Another folder while the grid is up: it re-flows and says
        // afresh what it shows, rather than keeping the old scroll.
        press(&app, "g");
        let seen = Rc::new(RefCell::new(Vec::new()));
        let reports = seen.clone();
        app.on_grid_range(move |scroll, _, cols| reports.borrow_mut().push((scroll, cols)));
        let twelve: Vec<Thumb> = (0..12)
            .map(|i| Thumb {
                name: format!("n{i:02}.CR3").into(),
                image: slint::Image::default(),
                ..Default::default()
            })
            .collect();
        app.set_thumbs(ModelRc::new(VecModel::from(twelve)));
        press(&app, Key::Shift);
        assert_eq!(*seen.borrow(), vec![(0.0, 8)]);
    }

    /// The shape a turn maps masks by is the frame's own, and a
    /// frame whose develop has not landed has no shape on the panel
    /// to borrow: the last frame's would map this one's masks by an
    /// aspect that is not theirs, and say nothing about it.
    #[test]
    fn a_frame_without_its_own_develop_does_not_borrow_the_last_ones_shape() {
        let app = window(2);
        let files = vec![
            PathBuf::from("/nowhere/a.CR3"),
            PathBuf::from("/nowhere/b.CR3"),
        ];
        let mut st = State::empty(files, &app);
        // Nothing developed yet, and nothing else to measure by.
        assert_eq!(st.source_size, (0, 0));
        st.current = Some(0);
        assert_eq!(frame_aspect(&mut st, 0), None);
        // The first frame's develop lands: landscape.
        st.source_size = (6000, 4000);
        assert_eq!(frame_aspect(&mut st, 0), Some(1.5));
        // The selection moves, as `on_select` leaves it: the size is
        // forgotten, and the second frame has no shape of its own to
        // be had (its file is not there), so nothing is claimed.
        st.current = Some(1);
        st.source_size = (0, 0);
        assert_eq!(
            frame_aspect(&mut st, 1),
            None,
            "the last frame's shape is not this one's"
        );
        // Its thumbnail arrives, portrait: that is what answers, and
        // the frame's own turn goes on top of it.
        st.thumb_base[1] = Some((400, 600, Vec::new()));
        assert_eq!(frame_aspect(&mut st, 1), Some(400.0 / 600.0));
        st.sidecars[1].turn = 1;
        assert_eq!(frame_aspect(&mut st, 1), Some(600.0 / 400.0));
    }

    /// The frame's own turn: a bracket either way, unmodified, and
    /// nobody else's when a modifier is down or a sheet is over the
    /// window.
    #[test]
    fn the_brackets_turn_the_frame_and_a_modifier_takes_them_away() {
        let app = window(11);
        let turns = Rc::new(RefCell::new(Vec::new()));
        let seen = turns.clone();
        app.on_frame_turned(move |q| seen.borrow_mut().push(q));
        press(&app, "[");
        press(&app, "]");
        assert_eq!(*turns.borrow(), vec![-1, 1]);
        // In culling and on the grid alike.
        app.set_culling(true);
        press(&app, "]");
        app.set_culling(false);
        app.set_grid_open(true);
        press(&app, "[");
        app.set_grid_open(false);
        assert_eq!(*turns.borrow(), vec![-1, 1, 1, -1]);
        // Ctrl+bracket is somebody else's, and a sheet keeps its own
        // keys.
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        press(&app, "[");
        press(&app, "]");
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        assert_eq!(turns.borrow().len(), 4);
        app.set_export_open(true);
        press(&app, "[");
        press(&app, "]");
        assert_eq!(turns.borrow().len(), 4);
    }

    #[test]
    fn the_culling_keys_reach_the_browser_in_both_views() {
        let app = window(11);
        let pressed = Rc::new(RefCell::new(Vec::new()));
        let seen = pressed.clone();
        app.on_meta_key(move |key| {
            // What the window sends is the key's text; what it means
            // is `meta::Change`'s to say, as it is in the editor.
            match meta::Change::from_key(&key) {
                Some(_) => {
                    seen.borrow_mut().push(key.to_string());
                    true
                }
                None => false,
            }
        });
        let zooms = Rc::new(RefCell::new(0));
        let counted = zooms.clone();
        app.on_zoom_key(move |_| *counted.borrow_mut() += 1);
        // The loupe: the strip is the browser there.
        for key in ["1", "5", "0", "p", "X", "u", "6", "9"] {
            press(&app, key);
        }
        assert_eq!(*pressed.borrow(), ["1", "5", "0", "p", "X", "u", "6", "9"]);
        // And the grid, where the plus and minus are still its own.
        press(&app, "g");
        press(&app, "3");
        press(&app, "+");
        assert_eq!(pressed.borrow().last().map(String::as_str), Some("3"));
        // A sheet over the window takes every key it is shown.
        app.set_export_open(true);
        press(&app, "4");
        assert_eq!(pressed.borrow().last().map(String::as_str), Some("3"));
        app.set_export_open(false);
        // The keys that are not the browser's are untouched: Z still
        // zooms, and G still closes the grid.
        press(&app, "g");
        press(&app, "z");
        assert_eq!(*zooms.borrow(), 1);
        assert!(!app.get_grid_open());
        assert_eq!(pressed.borrow().len(), 9);
    }

    #[test]
    fn plus_and_minus_size_the_cells_only_in_the_grid() {
        let app = window(11);
        let zooms = Rc::new(RefCell::new(Vec::new()));
        let seen = zooms.clone();
        app.on_grid_zoom(move |by| seen.borrow_mut().push(by));
        press(&app, "+");
        assert!(zooms.borrow().is_empty());
        press(&app, "g");
        press(&app, "+");
        press(&app, "=");
        press(&app, "-");
        assert_eq!(*zooms.borrow(), vec![1, 1, -1]);
    }

    #[test]
    fn a_frame_rated_but_never_developed_still_gets_its_iso_blend() {
        let dir = std::env::temp_dir().join(format!("greycard-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let rated = dir.join("IMG_0001.CR3");
        let developed = dir.join("IMG_0002.CR3");
        let untouched = dir.join("IMG_0003.CR3");
        let picture = dir.join("IMG_0004.JPG");

        // A star and nothing else: a sidecar with a default edit.
        let mut sidecar = Sidecar::default();
        sidecar.meta.set_rating(4);
        sidecar.save(&rated).expect("the sidecar writes");
        assert!(files::never_developed(&sidecar));
        // One that has been developed, however lightly.
        let mut sidecar = Sidecar::default();
        let mut edit = Edit::default();
        edit.light.exposure = 0.3;
        sidecar.record(edit);
        sidecar.save(&developed).expect("the sidecar writes");
        assert!(!files::never_developed(&sidecar));
        // And one rated in a picture that is not a raw, which has no
        // learned denoiser to seed.
        let mut sidecar = Sidecar {
            current: Edit::for_picture(),
            ..Sidecar::default()
        };
        sidecar.meta.set_rating(1);
        sidecar.save(&picture).expect("the sidecar writes");

        let files = vec![rated, developed, untouched, picture];
        let (sidecars, seed) = load_sidecars(&files, true);
        assert_eq!(sidecars[0].meta.rating, 4, "the meta came back");
        assert_eq!(seed, vec![true, false, true, false]);
        // With sidecars off nothing is read and nothing is seeded.
        assert_eq!(load_sidecars(&files, false).1, vec![false; 4]);
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
    }
}
