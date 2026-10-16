//! Tests of a view's bar while its read is out.

use super::*;

#[test]
fn the_bar_s_words_group_the_thousands() {
    assert_eq!(
        loading_words(3400, 11711, None),
        "Reading sidecars… 3,400 of 11,711"
    );
    assert_eq!(loading_words(0, 1, None), "Reading sidecars… 0 of 1");
    assert_eq!(loading_words(0, 1, Some("Archive")), "Waiting on Archive…");
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1000), "1,000");
    assert_eq!(grouped(1234567), "1,234,567");
}

/// A view whose sidecars take a while: no bar for the first
/// moment, then one filled to the count read, by the window's own
/// timer, and gone when the view lands.
#[test]
fn a_slow_view_shows_its_bar_after_the_wait_and_drops_it_on_landing() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-slow");
    open_three(&state, &app, &worker, &dir);
    let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    assert!(state.borrow().library.loading);
    assert!(state.borrow().library.loading_timer.running());
    read_so_far(&state, 1);
    let since = state.borrow().library.loading_since.unwrap();
    loading_tick(
        &mut state.borrow_mut(),
        &app,
        since + LOADING_SHOWN_AFTER / 2,
    );
    assert!(!app.get_loading_shown(), "not within the first moment");
    loading_tick(
        &mut state.borrow_mut(),
        &app,
        since + LOADING_SHOWN_AFTER * 2,
    );
    assert!(app.get_loading_shown(), "past it");
    assert!((app.get_loading_fraction() - 1.0 / 3.0).abs() < 1e-6);
    // The window's own timer from here, the read put far enough
    // back that the machine's clock cannot matter.
    state.borrow_mut().library.loading_since =
        Some(Instant::now() - LOADING_SHOWN_AFTER - Duration::from_millis(10));
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!(app.get_loading_shown());
    assert!((app.get_loading_fraction() - 1.0 / 3.0).abs() < 1e-6);
    assert_eq!(app.get_loading_line(), "Reading sidecars… 1 of 3");
    read_so_far(&state, 2);
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!((app.get_loading_fraction() - 2.0 / 3.0).abs() < 1e-6);
    assert_eq!(app.get_loading_line(), "Reading sidecars… 2 of 3");
    // The count standing still names the root the read waits on,
    // and the count is back as soon as it moves.
    let moved = state.borrow().library.progress_counted.unwrap();
    let later = moved + LOADING_WAITING_AFTER + Duration::from_millis(500);
    loading_tick(&mut state.borrow_mut(), &app, later);
    assert_eq!(app.get_loading_line(), "Waiting on a…");
    read_so_far(&state, 3);
    loading_tick(&mut state.borrow_mut(), &app, later);
    assert_eq!(app.get_loading_line(), "Reading sidecars… 3 of 3");
    // The read itself counts: what it brings is all three.
    land(&state, &app, &worker, look.run());
    {
        let st = state.borrow();
        assert_eq!(st.files.len(), 3);
        assert!(!st.library.loading);
        assert!(st.library.progress.is_none());
        assert!(!st.library.loading_timer.running(), "the timer stopped");
    }
    assert!(!app.get_loading_shown(), "gone on landing");
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!(!app.get_loading_shown());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A view that lands within the first moment never shows a bar.
#[test]
fn a_fast_view_never_shows_its_bar() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-fast");
    open_view(&state, &app, &worker, View::Roots(None));
    let since = state.borrow().library.loading_since.unwrap();
    for ms in [0, 100, 199] {
        loading_tick(
            &mut state.borrow_mut(),
            &app,
            since + Duration::from_millis(ms),
        );
        assert!(!app.get_loading_shown(), "{ms} ms");
    }
    assert_eq!(land_all(&state, &app, &worker), 1);
    assert!(!app.get_loading_shown());
    assert!(!state.borrow().library.loading_timer.running());
    loading_tick(
        &mut state.borrow_mut(),
        &app,
        since + LOADING_SHOWN_AFTER * 2,
    );
    assert!(!app.get_loading_shown(), "nothing loading, nothing shown");
    assert_eq!(state.borrow().files.len(), 3);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A read that hangs with no report coming is given up on by the
/// timer once it is past READ_GIVES_UP and its count has stood
/// still; one still moving past it keeps its bar.
#[test]
fn the_timer_gives_up_on_a_stopped_read_and_not_a_moving_one() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-stalled");
    open_three(&state, &app, &worker, &dir);
    let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    let since = state.borrow().library.loading_since.unwrap();
    let tick = |at: Duration| loading_tick(&mut state.borrow_mut(), &app, since + at);
    // Moving: a sidecar every 20 s, well past the give-up.
    tick(Duration::from_secs(1));
    read_so_far(&state, 1);
    tick(READ_GIVES_UP - Duration::from_secs(5));
    read_so_far(&state, 2);
    tick(READ_GIVES_UP + Duration::from_secs(5));
    tick(READ_GIVES_UP + LOADING_STALLED);
    assert!(state.borrow().library.loading, "still moving: kept");
    assert!(app.get_loading_shown());
    assert_eq!(app.get_loading_line(), "Waiting on a…", "ten seconds still");
    // Stopped: the count at 2 for longer than LOADING_STALLED.
    tick(READ_GIVES_UP + Duration::from_secs(5) + LOADING_STALLED + Duration::from_secs(1));
    {
        let st = state.borrow();
        assert!(!st.library.loading, "given up");
        assert!(!st.library.loading_timer.running());
    }
    assert!(!app.get_loading_shown());
    assert!(
        app.get_status().contains("did not come in"),
        "{}",
        app.get_status()
    );
    land(&state, &app, &worker, lost.run());
    assert_eq!(
        state.borrow().view,
        View::Folder,
        "the late read opens nothing"
    );
    assert!(!app.get_loading_shown());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A total that arrives after a slow listing is movement: the bar
/// shows the count first, and names the root only once both have
/// stood still.
#[test]
fn a_late_total_shows_the_count_before_waiting() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-late-total");
    open_three(&state, &app, &worker, &dir);
    let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    let since = state.borrow().library.loading_since.unwrap();
    let tick = |at: Duration| loading_tick(&mut state.borrow_mut(), &app, since + at);
    tick(Duration::from_secs(2));
    assert_eq!(app.get_loading_line(), "Waiting on a…", "nothing moved");
    let progress = state.borrow().library.progress.clone().unwrap();
    progress.total.store(2, Ordering::Relaxed);
    tick(Duration::from_millis(2100));
    assert_eq!(app.get_loading_line(), "Reading sidecars… 0 of 2");
    tick(Duration::from_millis(3200));
    assert_eq!(app.get_loading_line(), "Waiting on a…");
    drop(look);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A view's read given up on takes its bar with it, and the status
/// line says why; a folder opened over a view's read does too.
#[test]
fn a_view_given_up_on_or_overtaken_drops_its_bar() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-given-up");
    open_three(&state, &app, &worker, &dir);
    let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    state.borrow_mut().library.loading_since =
        Some(Instant::now() - READ_GIVES_UP - Duration::from_secs(1));
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!(app.get_loading_shown(), "still reading, and said so");
    assert_eq!(app.get_loading_line(), "Reading sidecars… 0 of 3");
    refresh_view(&state, &app, &worker);
    assert!(!app.get_loading_shown());
    assert!(!state.borrow().library.loading_timer.running());
    assert!(
        app.get_status().contains("did not come in"),
        "{}",
        app.get_status()
    );
    land(&state, &app, &worker, lost.run());
    land_all(&state, &app, &worker);
    assert!(!app.get_loading_shown(), "the late read brings no bar back");

    // A folder opened while a view's read is out.
    open_view(&state, &app, &worker, View::Roots(None));
    state.borrow_mut().library.loading_since =
        Some(Instant::now() - LOADING_SHOWN_AFTER - Duration::from_millis(10));
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!(app.get_loading_shown());
    let one = dir.join("a").join("x.tif");
    crate::panel::browser::open_paths(&state, &app, &worker, &[one]);
    assert!(
        !app.get_loading_shown(),
        "the view's bar went with its read"
    );
    assert!(
        state.borrow().library.loading_timer.running(),
        "the folder's own load has the timer now"
    );
    land_all(&state, &app, &worker);
    assert!(!app.get_loading_shown());
    assert!(!state.borrow().library.loading_timer.running());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A view's read given up on: the status says so rather than
/// "reading N frames..." for good, a capture waiting on it lets go,
/// and when the read does land it changes nothing, the roots found
/// offline among it.
#[test]
fn a_view_s_read_given_up_on_lets_go_and_lands_as_nothing() {
    let dir = scratch("open-given-up");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let files = frames(&a, &["x.tif"]);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&b).unwrap();
        st.library.awaiting = true;
    }
    std::fs::remove_dir(&b).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    assert!(app.get_status().contains("reading"), "{}", app.get_status());
    let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    state.borrow_mut().library.loading_since =
        Some(Instant::now() - READ_GIVES_UP - Duration::from_secs(1));
    refresh_view(&state, &app, &worker);
    {
        let st = state.borrow();
        assert!(!st.library.loading);
        assert!(!st.library.awaiting, "a capture lets go");
    }
    assert!(
        !app.get_status().contains("reading"),
        "{}",
        app.get_status()
    );
    land_all(&state, &app, &worker);
    land(&state, &app, &worker, lost.run());
    let st = state.borrow();
    assert!(st.library.offline.is_empty(), "the late read keeps nothing");
    assert_eq!(st.view, View::Folder, "nor opens its view");
    assert_eq!(st.files, files);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}
