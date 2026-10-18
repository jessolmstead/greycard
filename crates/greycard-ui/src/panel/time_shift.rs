//! Shift capture time: the frame menu's sheet over the selection, as
//! Lightroom's Edit Capture Time has it, for a second body whose clock
//! was off. The chosen frames' times move by a typed difference, or one
//! frame is set to a time and the rest move by the same difference; and
//! Clear shift takes every chosen frame back to its camera's time.
//!
//! Nothing here rewrites a raw. The shift is a field of the sidecar's
//! meta (`Meta::time_shift`, seconds), written down the culling keys'
//! path (`browser::meta_on_selection`): the sidecar, the XMP, the
//! index's row and culling's undo all follow as they do for a star.
//! The index reads the frame's time as the EXIF's moved by the shift,
//! so a list by capture time and the Day chips go by it; the export's
//! EXIF and the XMP's dates carry the moved time.
//!
//! The difference is typed in one field, `+H:MM:SS`, rather than three:
//! one field takes the sign once and a paste whole, and it also reads
//! `5m`, `-1h 30m` or `90s`, which is how a difference between two
//! clocks is said aloud. A colon form is read from the right, seconds
//! then minutes then hours, so `5:00` is five minutes.

use std::time::{Duration, Instant};

use greycard_core::exif::{seconds_of, shifted};
use greycard_edit::meta::{Change, SHIFT_LIMIT};

use crate::panel::browser::{chosen_frames, file_name, meta_on_selection};
use crate::*;

/// The sheet's two ways, as its switch names them.
pub(crate) const BY: &str = "Shift by";
pub(crate) const SET: &str = "Set this frame to";

/// How long a shift's rows are waited for before a list by capture
/// time stops looking for them: a frame whose sidecar goes to an
/// archive's copy has its row written by that copy's pass, not here.
const LANDING_GIVES_UP: Duration = Duration::from_secs(60);

/// What the window keeps for the shifts: the sheet's question while it
/// is up, the camera times read so far, and the rows a list by capture
/// time waits on.
#[derive(Debug, Default)]
pub(crate) struct Shifts {
    pub(crate) asked: Option<Asked>,
    /// Each file's capture time as its camera wrote it, the index's
    /// form, once read. Only a time found is kept: a file that gave
    /// none is asked again, in case the index has it since.
    times: HashMap<PathBuf, String>,
    /// The index rows a shift was just written to, with the shift each
    /// is to carry, and since when: a list by capture time is read
    /// again as each lands (`indexed`).
    landing: Vec<(i64, i64)>,
    landing_since: Option<Instant>,
}

/// The sheet's question while it is up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Asked {
    /// How many files the shift goes over.
    count: usize,
    /// The frame the sheet names: the one on screen when it is chosen,
    /// the first chosen otherwise.
    name: String,
    /// Its camera's own time, the index's form; none when the file
    /// gave none.
    camera: Option<String>,
    /// The shift it carries now.
    shift: i64,
}

/// A shift as the info line and the notices write it: a sign, then
/// minutes and seconds, the hours in front when there are any: `+5:00`,
/// `-1:02:03`, `+0:00`.
pub(crate) fn signed(secs: i64) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    let (h, m, s) = (a / 3600, a / 60 % 60, a % 60);
    if h > 0 {
        format!("{sign}{h}:{m:02}:{s:02}")
    } else {
        format!("{sign}{m}:{s:02}")
    }
}

/// A shift as the sheet's field shows it: always hours, minutes and
/// seconds, so what is typed over it reads the same way.
pub(crate) fn field(secs: i64) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    format!("{sign}{}:{:02}:{:02}", a / 3600, a / 60 % 60, a % 60)
}

/// A difference as typed into the sheet, in seconds: an optional sign,
/// then `H:MM:SS` or `M:SS` read from the right, or units, `1h 30m`,
/// `5m`, `90s`, `2d`. None for anything else (a bare number has no
/// unit to read it by), for a shift past the limit, and for one too
/// long to count.
pub(crate) fn parse_delta(text: &str) -> Option<i64> {
    let t = text.trim();
    let (sign, rest) = match t.chars().next()? {
        '+' => (1, &t[1..]),
        '-' => (-1, &t[1..]),
        '\u{2212}' => (-1, &t['\u{2212}'.len_utf8()..]),
        _ => (1, t),
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    let secs = if rest.contains(':') {
        colons(rest)?
    } else {
        units(rest)?
    };
    Some(sign * secs).filter(|s| s.abs() <= SHIFT_LIMIT)
}

/// A count of up to nine digits.
fn count(digits: &str) -> Option<i64> {
    (!digits.is_empty() && digits.len() <= 9 && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// `H:MM:SS`, `M:SS`: the parts after the first under sixty.
fn colons(text: &str) -> Option<i64> {
    let parts: Vec<i64> = text.split(':').map(count).collect::<Option<_>>()?;
    if parts.len() > 3 {
        return None;
    }
    let last = parts.len() - 1;
    let mut total = 0;
    for (i, n) in parts.iter().rev().enumerate() {
        if i < last && *n >= 60 {
            return None;
        }
        total += n * [1, 60, 3600][i];
    }
    Some(total)
}

/// `1h 30m`, `5m`, `90s`, `2d`: each number with its unit, spaces
/// anywhere but inside a number.
fn units(text: &str) -> Option<i64> {
    let (mut total, mut number, mut gap, mut any) = (0i64, String::new(), false, false);
    for c in text.chars() {
        if c.is_ascii_digit() {
            if gap {
                return None;
            }
            number.push(c);
        } else if c.is_whitespace() {
            gap = !number.is_empty();
        } else {
            let unit = match c.to_ascii_lowercase() {
                'd' => 86_400,
                'h' => 3600,
                'm' => 60,
                's' => 1,
                _ => return None,
            };
            // A long paste of units adds up past any count: no shift.
            total = total.checked_add(count(&number)?.checked_mul(unit)?)?;
            number.clear();
            (gap, any) = (false, true);
        }
    }
    (number.is_empty() && any).then_some(total)
}

/// A capture time moved by `by` seconds, in the index's form
/// (`YYYY-MM-DD HH:MM:SS`).
pub(crate) fn moved_time(camera: &str, by: i64) -> Option<String> {
    greycard_library::normalize_taken(&shifted(camera, by)?)
}

/// A difference in words for the sheet: `5:00 later`, `1:00:00
/// earlier`.
fn later(by: i64) -> String {
    let words = signed(by);
    let way = if by < 0 { "earlier" } else { "later" };
    format!("{} {way}", &words[1..])
}

/// The file at `path`'s capture time as its camera wrote it, read from
/// the file itself (the EXIF, no samples), in the index's form. For a
/// caller with no window state: an XMP written off the window's thread.
pub(crate) fn camera_time_of(path: &Path) -> Option<String> {
    let probe = if greycard_core::picture::is_picture_path(path) {
        greycard_core::picture::probe_path(path)
    } else {
        greycard_core::decode::probe_path(path)
    };
    greycard_library::normalize_taken(probe.ok()?.taken.as_deref()?)
}

/// File `file`'s capture time as its camera wrote it, in the index's
/// form: kept once found, read from the index's row by the id the
/// window holds, and from the file only when the index has none and
/// the file is not under a root that is offline.
pub(crate) fn camera_time(st: &mut State, file: usize) -> Option<String> {
    camera_time_read(st, file, true)
}

/// [`camera_time`], the file itself read only with `read_file`: the
/// info line, which follows every arrow in culling, takes what is kept
/// and what the index has, and never reads a raw for it.
fn camera_time_read(st: &mut State, file: usize, read_file: bool) -> Option<String> {
    let path = st.files.get(file)?.clone();
    if let Some(t) = st.sheets.time_shift.times.get(&path) {
        return Some(t.clone());
    }
    let from_index = st
        .views
        .index_ids
        .get(file)
        .copied()
        .flatten()
        .and_then(|id| st.views.index_reader.as_ref()?.by_id(id).ok().flatten())
        .and_then(|e| e.exif.taken);
    let readable = read_file && !st.views.library.offline.iter().any(|r| path.starts_with(r));
    let taken = from_index.or_else(|| readable.then(|| camera_time_of(&path)).flatten())?;
    st.sheets.time_shift.times.insert(path, taken.clone());
    Some(taken)
}

/// File `file`'s camera time as an open of it read it from the file
/// (the index's form), kept for the info line and the sheet: what a
/// frame outside the library's roots, with no row, has to go by.
pub(crate) fn seed(st: &mut State, file: usize, taken: String) {
    if let Some(path) = st.files.get(file).cloned() {
        st.sheets.time_shift.times.insert(path, taken);
    }
}

/// File `file`'s capture time as the window shows it: its camera's
/// moved by its shift, and the shift marked when there is one. Empty
/// when neither the index nor an earlier read has the camera's time.
pub(crate) fn time_words(st: &mut State, file: usize) -> String {
    let Some(camera) = camera_time_read(st, file, false) else {
        return String::new();
    };
    let shift = st.sidecars.get(file).map_or(0, |s| s.meta.time_shift);
    match shift {
        0 => camera,
        by => match moved_time(&camera, by) {
            Some(now) => format!("{now} (shifted {})", signed(by)),
            None => camera,
        },
    }
}

/// The panel's capture-time line for the frame on screen.
pub(crate) fn show_shot_time(st: &mut State, app: &App) {
    let words = match st.current {
        Some(c) => {
            let file = st.file_of(c);
            time_words(st, file)
        }
        None => String::new(),
    };
    app.set_shot_time(words.into());
}

/// The selection's shift was just written (`set_meta`, or culling's
/// undo putting one back): the info line says the new time, the status
/// line what moved, and a list by capture time waits for the index to
/// write the rows, to be read again in the new order (`indexed`).
pub(crate) fn moved(st: &mut State, app: &App, files: &[usize]) {
    show_shot_time(st, app);
    if files.is_empty() {
        return;
    }
    if st.culling.cull.is_none() {
        let said = match files {
            [f] => {
                let name = file_name(&st.files[*f]);
                match time_words(st, *f) {
                    words if words.is_empty() => format!("{name}: capture time shifted"),
                    words => format!("{name}: capture time {words}"),
                }
            }
            many => format!("capture time of {} moved", crate::queue::frames(many.len())),
        };
        app.set_status(said.into());
    }
    let by_time = st.views.sort.choice(&st.views.view).order().map(|o| o.by)
        == Some(greycard_library::SortBy::Taken);
    if !by_time {
        return;
    }
    let rows: Vec<(i64, i64)> = files
        .iter()
        .filter_map(|&f| {
            let id = st.views.index_ids.get(f).copied().flatten()?;
            Some((id, st.sidecars[f].meta.time_shift))
        })
        .collect();
    let shifts = &mut st.sheets.time_shift;
    shifts
        .landing
        .retain(|(id, _)| !rows.iter().any(|(r, _)| r == id));
    shifts.landing.extend(rows);
    shifts.landing_since = Some(Instant::now());
}

/// The index wrote rows: when some of them are a shift's the list by
/// capture time waits on, the list is read again, in the new order,
/// the frame on screen kept in view. True when a read was asked.
pub(crate) fn indexed(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> bool {
    let landed = {
        let mut guard = state.borrow_mut();
        let st = &mut *guard;
        let shifts = &mut st.sheets.time_shift;
        if shifts.landing.is_empty() {
            return false;
        }
        let Some(reader) = st.views.index_reader.as_ref() else {
            shifts.landing.clear();
            return false;
        };
        let before = shifts.landing.len();
        shifts
            .landing
            .retain(|&(id, shift)| match reader.by_id(id) {
                Ok(Some(e)) => e.meta.time_shift != shift,
                Ok(None) => false,
                Err(_) => true,
            });
        let landed = shifts.landing.len() < before;
        if shifts
            .landing_since
            .is_some_and(|t| t.elapsed() > LANDING_GIVES_UP)
        {
            shifts.landing.clear();
        }
        landed
    };
    if landed {
        state.borrow_mut().views.sort_reveal = true;
        return crate::roots::refresh_view(state, app, worker);
    }
    false
}

/// The frame menu's item: the sheet over the selection, its fields at
/// no shift and at the named frame's time now.
pub(crate) fn ask(st: &mut State, app: &App) {
    let cells = chosen_frames(st);
    let Some(&first) = cells.first() else {
        return;
    };
    let files = st.files_of(&cells);
    let named = st.current.filter(|c| cells.contains(c)).unwrap_or(first);
    let file = st.file_of(named);
    let camera = camera_time(st, file);
    let asked = Asked {
        count: files.len(),
        name: file_name(&st.files[file]),
        camera,
        shift: st.sidecars[file].meta.time_shift,
    };
    let any_shifted = files.iter().any(|&f| st.sidecars[f].meta.time_shift != 0);
    app.set_time_shift_title(
        match asked.count {
            1 => format!("Shift the capture time of {}", asked.name),
            n => format!("Shift the capture time of {n} frames"),
        }
        .into(),
    );
    app.set_time_shift_frame(frame_line(&asked).into());
    app.set_time_shift_mode(BY.into());
    app.set_time_shift_delta(field(0).into());
    let now = asked
        .camera
        .as_deref()
        .and_then(|c| moved_time(c, asked.shift))
        .unwrap_or_default();
    app.set_time_shift_to(now.into());
    app.set_time_shift_can_clear(any_shifted);
    st.sheets.time_shift.asked = Some(asked);
    show(st, app);
    app.set_time_shift_open(true);
}

/// The sheet's line about the frame it names.
fn frame_line(asked: &Asked) -> String {
    let Some(camera) = asked.camera.as_deref() else {
        return format!("{}: its camera wrote no capture time", asked.name);
    };
    let others = match asked.count {
        1 => String::new(),
        2 => "; the other frame moves with it".into(),
        n => format!("; the other {} frames move with it", n - 1),
    };
    match asked.shift {
        0 => format!(
            "{}: taken {camera} by its camera's clock{others}",
            asked.name
        ),
        by => format!(
            "{}: taken {camera} by its camera's clock, {} now (shifted {}){others}",
            asked.name,
            moved_time(camera, by).unwrap_or_default(),
            signed(by)
        ),
    }
}

/// The difference Apply would make, from the sheet's fields, or why
/// they do not read.
fn delta_of(asked: &Asked, mode: &str, delta: &str, to: &str) -> Result<i64, String> {
    if mode != SET {
        return parse_delta(delta).ok_or_else(|| {
            "Type the difference as +1:00:00 or -0:05:00, or as 1h, 5m, -30s".to_string()
        });
    }
    let Some(camera) = asked.camera.as_deref().and_then(seconds_of) else {
        return Err(format!(
            "{} has no capture time to set from: shift it by a difference instead",
            asked.name
        ));
    };
    // A date, or a date and a time to the second, and nothing after:
    // a fraction or a zone typed here would be read past and lost.
    let whole = matches!(to.trim().len(), 10 | 19);
    let to = seconds_of(to)
        .filter(|_| whole)
        .ok_or_else(|| "Type the date and time as 2026-09-20 14:05:00".to_string())?;
    let by = to - camera - asked.shift;
    if (asked.shift + by).abs() > SHIFT_LIMIT {
        return Err("That is further than any clock is set wrong".into());
    }
    Ok(by)
}

/// The sheet's words and its Apply, from what is in the fields.
pub(crate) fn show(st: &State, app: &App) {
    let Some(asked) = &st.sheets.time_shift.asked else {
        return;
    };
    let (mode, delta, to) = (
        app.get_time_shift_mode(),
        app.get_time_shift_delta(),
        app.get_time_shift_to(),
    );
    let (said, refusal, ready) = match delta_of(asked, &mode, &delta, &to) {
        Ok(0) => ("No change".to_string(), String::new(), false),
        Ok(by) => {
            let whom = match asked.count {
                1 => asked.name.clone(),
                n => format!("{n} frames"),
            };
            let lands = asked
                .camera
                .as_deref()
                .and_then(|c| moved_time(c, asked.shift + by))
                .map(|t| match asked.count {
                    1 => format!(", to {t}"),
                    _ => format!("; {} to {t}", asked.name),
                })
                .unwrap_or_default();
            (
                format!("Moves {whom} {}{lands}", later(by)),
                String::new(),
                true,
            )
        }
        Err(why) => (String::new(), why, false),
    };
    app.set_time_shift_said(said.into());
    app.set_time_shift_refusal(refusal.into());
    app.set_time_shift_ready(ready);
}

/// The sheet answered: 1 Apply, 2 Clear shift, anything else Cancel.
/// Apply with fields that do not read keeps the sheet up.
pub(crate) fn answered(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, answer: i32) {
    let change = {
        let mut st = state.borrow_mut();
        let Some(asked) = st.sheets.time_shift.asked.clone() else {
            app.set_time_shift_open(false);
            return;
        };
        let change = match answer {
            1 => match delta_of(
                &asked,
                &app.get_time_shift_mode(),
                &app.get_time_shift_delta(),
                &app.get_time_shift_to(),
            ) {
                Ok(0) | Err(_) => return,
                Ok(by) => Some(Change::TimeShift(by)),
            },
            2 => Some(Change::ClearTimeShift),
            _ => None,
        };
        st.sheets.time_shift.asked = None;
        change
    };
    app.set_time_shift_open(false);
    if let Some(change) = change {
        meta_on_selection(state, app, worker, change);
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_time_shift_asked(move || {
            if let Some(app) = app_weak.upgrade() {
                ask(&mut state.borrow_mut(), &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_time_shift_edited(move || {
            if let Some(app) = app_weak.upgrade() {
                show(&state.borrow(), &app);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_time_shift_answered(move |answer| {
            if let Some(app) = app_weak.upgrade() {
                answered(&state, &app, &worker, answer);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::cull::enter_cull;
    use crate::testing::{folder, state_for, window};

    fn shifts(st: &State) -> Vec<i64> {
        st.sidecars.iter().map(|s| s.meta.time_shift).collect()
    }

    /// The sheet over two chosen frames in culling, one shifted a
    /// minute already: Apply moves both by the difference typed, the
    /// word over the picture says so, the selection stays where it is
    /// with the CULLING section's move-on switch on, and culling's undo
    /// and redo take the shift back and make it again. Clear shift takes
    /// both back to the camera's time; Cancel does nothing; a rating
    /// key after it still moves on.
    #[test]
    fn the_sheet_shifts_the_selection_and_cullings_undo_takes_it_back() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        {
            let mut st = state.borrow_mut();
            st.views.picked = vec![1, 2];
            st.sidecars[2].meta.time_shift = 60;
        }
        enter_cull(&mut state.borrow_mut(), &app, 1);
        app.set_cull_move_on(true);
        app.invoke_cull_move_on_changed();

        app.invoke_time_shift_asked();
        assert!(app.get_time_shift_open());
        assert_eq!(
            app.get_time_shift_title(),
            "Shift the capture time of 2 frames"
        );
        // Nothing on disk under these: no camera time to name or set.
        assert!(
            app.get_time_shift_frame().contains("no capture time"),
            "{}",
            app.get_time_shift_frame()
        );
        assert!(app.get_time_shift_can_clear(), "one is shifted already");
        assert_eq!(app.get_time_shift_delta(), "+0:00:00");
        assert!(!app.get_time_shift_ready());
        assert_eq!(app.get_time_shift_said(), "No change");
        app.set_time_shift_delta("+0:05:00".into());
        app.invoke_time_shift_edited();
        assert!(app.get_time_shift_ready());
        assert_eq!(app.get_time_shift_said(), "Moves 2 frames 5:00 later");
        app.set_time_shift_delta("five".into());
        app.invoke_time_shift_edited();
        assert!(!app.get_time_shift_ready());
        assert!(app.get_time_shift_refusal().contains("+1:00:00"));
        // Set this frame to: nothing to set from here.
        app.set_time_shift_mode(SET.into());
        app.invoke_time_shift_edited();
        assert!(!app.get_time_shift_ready());
        assert!(app.get_time_shift_refusal().contains("no capture time"));
        // Apply that does not read keeps the sheet up.
        app.invoke_time_shift_answered(1);
        assert!(app.get_time_shift_open());
        app.set_time_shift_mode(BY.into());
        app.set_time_shift_delta("5m".into());
        app.invoke_time_shift_edited();
        app.invoke_time_shift_answered(1);
        assert!(!app.get_time_shift_open());
        assert_eq!(shifts(&state.borrow()), [0, 300, 360, 0]);
        assert_eq!(state.borrow().current, Some(1), "no move on for a shift");
        assert_eq!(app.get_notice(), "Capture time +5:00");

        // Culling's undo, both frames back; its redo, both again.
        app.invoke_undo();
        assert_eq!(shifts(&state.borrow()), [0, 0, 60, 0]);
        assert_eq!(app.get_notice(), "Undone");
        app.invoke_redo();
        assert_eq!(shifts(&state.borrow()), [0, 300, 360, 0]);

        // Clear shift: both to the camera's time.
        app.invoke_time_shift_asked();
        app.invoke_time_shift_answered(2);
        assert!(!app.get_time_shift_open());
        assert_eq!(shifts(&state.borrow()), [0, 0, 0, 0]);
        assert_eq!(app.get_notice(), "Capture time as the camera set it");
        // Cancel: nothing.
        app.invoke_time_shift_asked();
        assert!(!app.get_time_shift_can_clear(), "nothing shifted now");
        app.set_time_shift_delta("+1:00:00".into());
        app.invoke_time_shift_edited();
        app.invoke_time_shift_answered(0);
        assert!(!app.get_time_shift_open());
        assert_eq!(shifts(&state.borrow()), [0, 0, 0, 0]);

        // A rating key on one frame still moves on.
        state.borrow_mut().views.picked.clear();
        app.invoke_meta_key("3".into());
        assert_eq!(state.borrow().current, Some(2));
    }

    #[test]
    fn a_difference_reads_as_it_is_said() {
        assert_eq!(parse_delta("+0:05:00"), Some(300));
        assert_eq!(parse_delta("5:00"), Some(300), "read from the right");
        assert_eq!(parse_delta("-1:00:00"), Some(-3600));
        assert_eq!(parse_delta("\u{2212}0:00:30"), Some(-30));
        assert_eq!(parse_delta("26:00:00"), Some(26 * 3600), "hours past a day");
        assert_eq!(parse_delta("45"), None, "a bare number has no unit");
        assert_eq!(parse_delta("5m"), Some(300));
        assert_eq!(parse_delta("-1h 30m"), Some(-5400));
        assert_eq!(parse_delta("1h30m15s"), Some(5415));
        assert_eq!(parse_delta(" + 2d "), Some(2 * 86_400));
        assert_eq!(parse_delta("90s"), Some(90));
        for bad in [
            "",
            "+",
            "5:60",
            "1:5:00:00",
            "5 minutes",
            "1 30m",
            "5x",
            "a:00",
            "-",
        ] {
            assert_eq!(parse_delta(bad), None, "{bad:?}");
        }
        assert_eq!(parse_delta("999999999d"), None, "past the limit");
        // A long paste adds up past what an i64 holds: no shift, and
        // no panic.
        let pasted = "999999999d".repeat(200_000);
        assert_eq!(parse_delta(&pasted), None);
        assert_eq!(parse_delta(&format!("-{pasted}")), None);
    }

    #[test]
    fn a_shift_is_written_short_and_the_field_long() {
        assert_eq!(signed(300), "+5:00");
        assert_eq!(signed(-3723), "-1:02:03");
        assert_eq!(signed(0), "+0:00");
        assert_eq!(field(300), "+0:05:00");
        assert_eq!(field(-90), "-0:01:30");
        assert_eq!(later(300), "5:00 later");
        assert_eq!(later(-3600), "1:00:00 earlier");
        // What the field shows reads back as what it was made from.
        for secs in [0, 59, -300, 86_399, -200_000] {
            assert_eq!(parse_delta(&field(secs)), Some(secs), "{secs}");
        }
        assert_eq!(
            moved_time("2026-09-19 23:56:00", 300).as_deref(),
            Some("2026-09-20 00:01:00")
        );
    }

    /// A frame outside the library's roots has no row to give its
    /// time: the open's own read of the file gives it, the info line
    /// shows it at once, and a shift is marked on it.
    #[test]
    fn the_info_line_takes_the_time_the_open_read_with_no_index() {
        let app = window(1);
        let (state, _worker) = state_for(&app, folder(1));
        app.invoke_select(0);
        assert_eq!(app.get_shot_time(), "");
        let generation = state.borrow().develop.generation;
        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::Opened {
                generation,
                as_shot: (5500.0, 0.0),
                frame: None,
                profile: None,
                lens: crate::worker::LensReport::NoDatabase,
                kind: crate::worker::SourceKind::Picture {
                    space: "sRGB".into(),
                    bits: 8,
                },
                shot: Default::default(),
                taken: Some("2026-05-01 10:00:00".into()),
                blend: None,
            },
        );
        assert_eq!(app.get_shot_time(), "2026-05-01 10:00:00");
        let mut st = state.borrow_mut();
        st.sidecars[0].meta.set_time_shift(300);
        show_shot_time(&mut st, &app);
        assert_eq!(app.get_shot_time(), "2026-05-01 10:05:00 (shifted +5:00)");
    }

    fn asked(camera: Option<&str>, shift: i64, count: usize) -> Asked {
        Asked {
            count,
            name: "b1.tif".into(),
            camera: camera.map(str::to_string),
            shift,
        }
    }

    #[test]
    fn set_this_frame_to_is_the_difference_from_its_time_now() {
        let a = asked(Some("2026-09-19 23:56:00"), 0, 3);
        // Set to five minutes on: the others move by the same.
        assert_eq!(delta_of(&a, SET, "", "2026-09-20 00:01:00"), Ok(300));
        // Already shifted a minute: the difference is from where it is
        // now, not from the camera's time.
        let a = asked(Some("2026-09-19 23:56:00"), 60, 3);
        assert_eq!(delta_of(&a, SET, "", "2026-09-20 00:01:00"), Ok(240));
        assert!(delta_of(&a, SET, "", "tomorrow").is_err());
        // Nothing past the seconds, no day the month has not got, one
        // separator throughout.
        for bad in [
            "2026-09-20 00:01:00.5",
            "2026-09-20 00:01:00+02:00",
            "2026-09-20 00:01:00 later",
            "2026-02-30 00:01:00",
            "2026:09-20 00:01:00",
        ] {
            assert!(delta_of(&a, SET, "", bad).is_err(), "{bad}");
        }
        assert_eq!(delta_of(&a, SET, "", "2026-09-20"), Ok(4 * 60 - 60));
        assert_eq!(delta_of(&a, SET, "", " 2026-09-20T00:01:00 "), Ok(240));
        let none = asked(None, 0, 1);
        assert!(
            delta_of(&none, SET, "", "2026-09-20 00:01:00")
                .unwrap_err()
                .contains("no capture time")
        );
        // By a difference, the camera's time is not needed.
        assert_eq!(delta_of(&none, BY, "+0:05:00", ""), Ok(300));
        assert!(delta_of(&none, BY, "five", "").is_err());
        assert!(frame_line(&none).contains("no capture time"));
        assert!(
            frame_line(&a).contains("(shifted +1:00)"),
            "{}",
            frame_line(&a)
        );
        assert!(frame_line(&a).contains("the other 2 frames move with it"));
    }
}
