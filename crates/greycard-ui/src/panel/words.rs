//! The METADATA section's Title and Keywords fields: what the
//! selection's frames say, shown, and what is typed written over every
//! frame chosen, by the culling keys' own path (`meta_on_selection`):
//! the sidecar, the XMP beside it when those are written, the index's
//! row, the filter's chips, and in culling its undo and its word over
//! the picture.
//!
//! Over a set of frames the fields show what the set says together
//! (`meta::shared`): every keyword any of them has, those on only some
//! of them dimmed, and the title they share or none, with "Mixed" in
//! the field when two differ. A word added goes on all of them, a word
//! taken off comes off all of them, and a title typed is every frame's,
//! as Lightroom has it.
//!
//! The Title field's text is the frames' the field was showing when it
//! was typed ([`Words::shown_for`]), not whatever is chosen when it is
//! sent: the selection can move under a field with the focus (a click
//! in the strip, a right-click, a folder picked), and Slint tells the
//! field it lost the focus only after the move. So the window keeps
//! which frames the field shows, and when the selection leaves them
//! with the field typed in, the text is sent to them, once the state
//! is free, and the field shows the new frames' title.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use greycard_edit::meta::{self, Change};
use slint::{Model, ModelRc, VecModel};

use crate::panel::browser::{chosen_frames, chosen_keys, meta_on_keys, meta_on_selection};
use crate::versions::Key;
use crate::*;

/// How many completions the Keywords field offers at once.
pub(crate) const OFFERED: usize = 8;

/// The METADATA section's part of the window's state.
#[derive(Debug, Default)]
pub(crate) struct Words {
    /// The frames the fields show, by key: the ones a title typed is
    /// for, whatever has been chosen since.
    pub(crate) shown_for: Vec<Key>,
    /// Whether the Title field showed Mixed over them: an empty title
    /// sent then is a field typed in and erased, not a title to put on
    /// every frame.
    shown_mixed: bool,
    /// The keywords every one of them has, folded ([`meta::fold`]),
    /// which the completions leave out.
    on_all: HashSet<String>,
    /// Titles typed for frames the selection has since left, each
    /// with the frames it is for, to be sent once the state is free.
    pending: Vec<(Vec<Key>, String)>,
    /// The words the Keywords field completes from, the most held
    /// first; read when the field is first wanted, and again after a
    /// pass of the index's, which can bring words the window never
    /// wrote.
    vocabulary: Option<Vec<Word>>,
}

/// A word to complete with.
#[derive(Debug, Clone)]
struct Word {
    folded: String,
    spelling: String,
    count: usize,
}

/// Put the selection's title and keywords on the window, for the
/// METADATA section. The keywords are every frame's, read or standing
/// in, since a row carries them; the title is only the frames' whose
/// sidecars are read, since a row does not, and a title not yet read
/// would otherwise show as none. Each property is set only when it
/// moved, so a chip row that did not change is not built again.
///
/// When the selection is no longer the frames the fields show, a
/// title typed for those and not yet sent goes to them (on the next
/// turn of the event loop, since the state is borrowed here), and the
/// fields start again on the new frames: the title theirs, the
/// keyword half typed dropped.
pub(crate) fn show_frame_words(st: &State, app: &App) {
    let cells = chosen_frames(st);
    let keys: Vec<Key> = cells.iter().filter_map(|&c| st.key_of(c)).collect();
    let files = st.files_of(&cells);
    let metas = |read_only: bool| {
        files
            .iter()
            .filter(move |&&f| !read_only || st.from_row.get(f).is_none_or(|r| r.read))
            .filter_map(|&f| st.sidecars.get(f))
            .map(|s| &s.meta)
    };
    let keywords = meta::shared(metas(false)).keywords;
    let (title, mixed) = match meta::shared(metas(true)).title {
        Some(title) => (title, false),
        None => (String::new(), true),
    };
    let mut words = st.views.words.borrow_mut();
    if !same_frames(&words.shown_for, &keys) {
        if app.get_title_dirty() {
            let text = app.get_title_text().to_string();
            let was = std::mem::take(&mut words.shown_for);
            if !was.is_empty() && !(words.shown_mixed && text.trim().is_empty()) {
                words.pending.push((was, text));
                send_pending_later(app);
            }
        }
        words.shown_for = keys;
        app.set_title_dirty(false);
        app.set_title_text(title.as_str().into());
        if !app.get_keyword_text().is_empty() {
            app.set_keyword_text("".into());
        }
        if app.get_keyword_completions().row_count() > 0 {
            app.set_keyword_completions(ModelRc::default());
        }
        app.set_keyword_pick(-1);
    } else if !app.get_title_dirty() && app.get_title_text() != title.as_str() {
        app.set_title_text(title.as_str().into());
    }
    words.shown_mixed = mixed;
    words.on_all = keywords
        .iter()
        .filter(|(_, all)| *all)
        .map(|(w, _)| meta::fold(w))
        .collect();
    drop(words);
    let on = !files.is_empty();
    if app.get_frame_meta_on() != on {
        app.set_frame_meta_on(on);
    }
    if app.get_frame_title() != title.as_str() {
        app.set_frame_title(title.into());
    }
    if app.get_frame_title_mixed() != mixed {
        app.set_frame_title_mixed(mixed);
    }
    let chips: Vec<KeywordChip> = keywords
        .into_iter()
        .map(|(word, all)| KeywordChip {
            word: word.into(),
            all,
        })
        .collect();
    let model = app.get_frame_keywords();
    let same = model.row_count() == chips.len()
        && chips
            .iter()
            .enumerate()
            .all(|(i, c)| model.row_data(i).as_ref() == Some(c));
    if !same {
        app.set_frame_keywords(ModelRc::new(VecModel::from(chips)));
    }
}

/// The titles left pending sent, on the next turn of the event loop.
fn send_pending_later(app: &App) {
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let (Some(app), Some(state), Some(worker)) = (
            weak.upgrade(),
            crate::STATE.with(|s| s.borrow().clone()),
            crate::WORKER.with(|w| w.borrow().clone()),
        ) else {
            return;
        };
        send_pending(&state, &app, &worker);
    });
}

/// Each title left pending, onto the frames it was typed for.
fn send_pending(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) {
    let pending = std::mem::take(&mut state.borrow().views.words.borrow_mut().pending);
    for (keys, text) in pending {
        meta_on_keys(state, app, worker, keys, Change::Title(text));
    }
}

/// Whatever title is typed and not yet sent, written now: the pending
/// ones, and the field's own text when it was typed in. For the quit,
/// which closes the window before a click away or Enter could send it,
/// and runs once the event loop has ended: so nothing waits for a turn
/// of it. A frame still standing in from its row is read here and
/// now, as a key's frames are (`rows::load_frames`), rather than
/// queued for a read that could only land on a loop that is over.
/// Called before the worker stops and the index closes, so the saves
/// are told to the index as any other.
pub(crate) fn send_title(state: &Rc<RefCell<State>>, app: &App) {
    let mut sends = std::mem::take(&mut state.borrow().views.words.borrow_mut().pending);
    if app.get_title_dirty() {
        app.set_title_dirty(false);
        let text = app.get_title_text().to_string();
        if let Some(keys) = shown_keys(&state.borrow(), &text) {
            sends.push((keys, text));
        }
    }
    let mut st = state.borrow_mut();
    for (keys, text) in sends {
        let cells: Vec<usize> = crate::rows::resolve(&st, &keys, crate::rows::Scope::File)
            .into_iter()
            .flatten()
            .collect();
        crate::panel::browser::set_meta(&mut st, app, &cells, Change::Title(text));
    }
}

/// The frames a title `text` from the field is for: the ones it shows,
/// or none when it is an empty field over a Mixed set.
fn shown_keys(st: &State, text: &str) -> Option<Vec<Key>> {
    let words = st.views.words.borrow();
    if words.shown_mixed && text.trim().is_empty() {
        return None;
    }
    // Nothing shown yet (no selection has been put out): the
    // selection is what the field stands over.
    Some(if words.shown_for.is_empty() {
        chosen_keys(st)
    } else {
        words.shown_for.clone()
    })
}

/// `text` as the title of the frames the field shows, unless it is an
/// empty field over a Mixed set.
fn title_to_shown(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, text: String) {
    let Some(keys) = shown_keys(&state.borrow(), &text) else {
        return;
    };
    meta_on_keys(state, app, worker, keys, Change::Title(text));
}

/// Whether `a` and `b` name the same frames, in whatever order: a
/// selection put in another order (a sort, a set rebuilt) is still the
/// one the field is typed for.
fn same_frames(a: &[Key], b: &[Key]) -> bool {
    a.len() == b.len() && a.iter().collect::<HashSet<_>>() == b.iter().collect::<HashSet<_>>()
}

/// The library's words and the frames' in hand, each once under
/// [`meta::fold`] in the spelling most files hold it in, the most
/// held first. Read once and kept ([`Words::vocabulary`]): one pass
/// over the index's keywords table, then one over the sidecars in
/// hand for words the index does not have yet, which are all there
/// is with no index.
fn read_vocabulary(st: &State) -> Vec<Word> {
    // Per fold: the place in `out`, and how many files the spelling
    // kept holds, for the spelling the most of them have.
    let mut at: HashMap<String, (usize, usize)> = HashMap::new();
    let mut out: Vec<Word> = Vec::new();
    let indexed = st.views.index_reader.as_ref().map_or_else(Vec::new, |lib| {
        lib.keyword_counts().unwrap_or_else(|e| {
            tracing::warn!("library index: no keywords to offer: {e}");
            Vec::new()
        })
    });
    for (spelling, count) in indexed {
        let folded = meta::fold(&spelling);
        match at.get_mut(&folded) {
            Some((i, best)) => {
                out[*i].count += count;
                if count > *best {
                    *best = count;
                    out[*i].spelling = spelling;
                }
            }
            None => {
                at.insert(folded.clone(), (out.len(), count));
                out.push(Word {
                    folded,
                    spelling,
                    count,
                });
            }
        }
    }
    let known: HashSet<String> = at.into_keys().collect();
    let mut in_hand: HashMap<String, usize> = HashMap::new();
    for word in st.sidecars.iter().flat_map(|s| &s.meta.keywords) {
        let folded = meta::fold(word);
        if known.contains(&folded) {
            continue;
        }
        match in_hand.get(&folded) {
            Some(&i) => out[i].count += 1,
            None => {
                in_hand.insert(folded.clone(), out.len());
                out.push(Word {
                    folded,
                    spelling: word.clone(),
                    count: 1,
                });
            }
        }
    }
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.folded.cmp(&b.folded)));
    out
}

/// The vocabulary read, if it is not in hand.
fn want_vocabulary(st: &State) {
    if st.views.words.borrow().vocabulary.is_none() {
        let read = read_vocabulary(st);
        st.views.words.borrow_mut().vocabulary = Some(read);
    }
}

/// The vocabulary dropped, to be read again when next wanted: after an
/// index pass, which may have brought words from files the window
/// never wrote.
pub(crate) fn forget_vocabulary(st: &mut State) {
    st.views.words.get_mut().vocabulary = None;
}

/// A word about to come off the frames chosen, out of the vocabulary
/// in hand by as many of them as hold it: gone from it when no file
/// is left holding it, so it is not offered from memory after it is
/// gone from the library.
fn unlearn(st: &State, word: &str) {
    let folded = meta::fold(word);
    let held = st
        .files_of(&chosen_frames(st))
        .into_iter()
        .filter_map(|f| st.sidecars.get(f))
        .filter(|s| s.meta.keywords.iter().any(|k| meta::fold(k) == folded))
        .count();
    let mut kept = st.views.words.borrow_mut();
    let Some(vocabulary) = kept.vocabulary.as_mut() else {
        return;
    };
    if let Some(i) = vocabulary.iter().position(|w| w.folded == folded) {
        vocabulary[i].count = vocabulary[i].count.saturating_sub(held);
        if vocabulary[i].count == 0 {
            vocabulary.remove(i);
        }
    }
}

/// Words the window just put on frames, into the vocabulary in hand,
/// so the next completion offers them without reading it again.
/// Each is counted for the frames chosen that do not hold it yet.
fn learn(st: &State, words: &[String]) {
    let files = st.files_of(&chosen_frames(st));
    let mut kept = st.views.words.borrow_mut();
    let Some(vocabulary) = kept.vocabulary.as_mut() else {
        return;
    };
    for word in words {
        let folded = meta::fold(word);
        let gaining = files
            .iter()
            .filter_map(|&f| st.sidecars.get(f))
            .filter(|s| !s.meta.keywords.iter().any(|k| meta::fold(k) == folded))
            .count();
        match vocabulary.iter_mut().find(|w| w.folded == folded) {
            Some(w) => w.count += gaining,
            None if gaining > 0 => vocabulary.push(Word {
                folded,
                spelling: word.trim().to_string(),
                count: gaining,
            }),
            None => {}
        }
    }
}

/// The keywords to offer for `typed`, the start of a word: the
/// vocabulary's words that begin with it under [`meta::fold`], the
/// most held first, none that every frame shown has already, at most
/// [`OFFERED`]. Nothing for nothing typed. In memory: no query per key.
pub(crate) fn completions(st: &State, typed: &str) -> Vec<String> {
    let typed = meta::fold(typed);
    if typed.is_empty() {
        return Vec::new();
    }
    want_vocabulary(st);
    let words = st.views.words.borrow();
    words
        .vocabulary
        .iter()
        .flatten()
        .filter(|w| w.folded.starts_with(&typed) && !words.on_all.contains(&w.folded))
        .take(OFFERED)
        .map(|w| w.spelling.clone())
        .collect()
}

/// The words in `text`, split at its commas, trimmed, the empty ones
/// dropped.
fn words_of(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Put the completions for `typed` on the window, the pick off them.
fn offer(st: &State, app: &App, typed: &str) {
    let offered: Vec<slint::SharedString> =
        completions(st, typed).into_iter().map(Into::into).collect();
    app.set_keyword_pick(-1);
    app.set_keyword_completions(ModelRc::new(VecModel::from(offered)));
}

/// `words` onto the selection, and into the vocabulary.
fn add_words(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, words: Vec<String>) {
    learn(&state.borrow(), &words);
    meta_on_selection(state, app, worker, Change::AddKeywords(words));
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // The Keywords field taken: its words read now, if they are not in
    // hand, rather than at the first key.
    {
        let state = state.clone();
        app.on_keyword_focused(move || want_vocabulary(&state.borrow()));
    }
    // What is typed in the Keywords field: a comma ends a word, which
    // goes on at once, the rest left in the field; what is left is
    // completed from the library's words.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_keyword_typed(move |text| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut typed = text.as_str();
            if let Some(cut) = typed.rfind(',') {
                let done = words_of(&typed[..cut]);
                typed = typed[cut + 1..].trim_start();
                app.set_keyword_text(typed.into());
                if !done.is_empty() {
                    add_words(&state, &app, &worker, done);
                }
            }
            offer(&state.borrow(), &app, typed);
        });
    }
    // A word taken: Enter on what is typed, Tab or a click on a
    // completion, a click on a word only some of the frames have.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_keyword_added(move |text| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let words = words_of(&text);
            offer(&state.borrow(), &app, "");
            if !words.is_empty() {
                add_words(&state, &app, &worker, words);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_keyword_removed(move |word| {
            if let Some(app) = app_weak.upgrade() {
                unlearn(&state.borrow(), &word);
                meta_on_selection(&state, &app, &worker, Change::RemoveKeyword(word.into()));
            }
        });
    }
    // The title, once the field is done with: Enter, the focus gone
    // elsewhere with the text changed, or a click on a frame. It is the
    // frames' the field shows, which a click in the strip has not moved
    // yet when it sends.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_title_edited(move |text| {
            if let Some(app) = app_weak.upgrade() {
                title_to_shown(&state, &app, &worker, text.into());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::browser::show_set;
    use crate::testing::{self, folder, press, state_for, type_text, window};
    use slint::platform::Key;
    use std::path::PathBuf;

    /// Frames `0..n` of `folder(n)` open, the first on screen and all
    /// of them chosen.
    fn chosen(n: usize) -> (App, Rc<RefCell<State>>, Rc<Worker>) {
        let app = window(n);
        let (state, worker) = state_for(&app, folder(n));
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            st.views.picked = (0..n).collect();
            show_set(&mut st, &app);
        }
        (app, state, worker)
    }

    fn keywords_of(st: &State, i: usize) -> Vec<String> {
        st.sidecars[i].meta.keywords.clone()
    }

    fn chips(app: &App) -> Vec<(String, bool)> {
        app.get_frame_keywords()
            .iter()
            .map(|c| (c.word.to_string(), c.all))
            .collect()
    }

    /// A keyword typed goes on every frame chosen, as a rating key's
    /// stars do, and a word taken off comes off all of them whatever
    /// its case there. One that only some of the frames carry is shown
    /// dimmed, and taken again it goes on the rest.
    #[test]
    fn a_keyword_goes_on_every_frame_chosen_and_comes_off_them_all() {
        let (app, state, _worker) = chosen(3);
        state.borrow_mut().sidecars[2].meta.add_keyword("Boat");
        crate::panel::browser::show_frame_tags(&state.borrow(), &app);
        assert_eq!(chips(&app), [("Boat".to_string(), false)]);

        app.invoke_keyword_added("Harbor, dusk".into());
        for i in 0..3 {
            let words = keywords_of(&state.borrow(), i);
            assert!(
                words.ends_with(&["Harbor".into(), "dusk".into()]),
                "{i}: {words:?}"
            );
        }
        assert_eq!(
            chips(&app),
            [
                ("Harbor".to_string(), true),
                ("dusk".to_string(), true),
                ("Boat".to_string(), false)
            ]
        );

        app.invoke_keyword_removed("HARBOR".into());
        for i in 0..3 {
            assert!(!keywords_of(&state.borrow(), i).contains(&"Harbor".to_string()));
        }
        // The dimmed word, taken: on all three now.
        app.invoke_keyword_added("Boat".into());
        assert!(chips(&app).iter().all(|(_, all)| *all), "{:?}", chips(&app));
        assert_eq!(keywords_of(&state.borrow(), 0), ["dusk", "Boat"]);
        assert_eq!(keywords_of(&state.borrow(), 2), ["Boat", "dusk"]);
    }

    /// A title typed is every frame's. Over frames whose titles
    /// differ the field is empty and says Mixed, and nothing is
    /// written until a title is sent.
    #[test]
    fn a_title_is_every_frames_and_a_mixed_one_says_so() {
        let (app, state, _worker) = chosen(2);
        state.borrow_mut().sidecars[1].meta.set_title("Dawn");
        crate::panel::browser::show_frame_tags(&state.borrow(), &app);
        assert!(app.get_frame_title_mixed());
        assert_eq!(app.get_frame_title(), "");

        app.invoke_title_edited("  Dusk ".into());
        for i in 0..2 {
            assert_eq!(state.borrow().sidecars[i].meta.title, "Dusk");
        }
        assert!(!app.get_frame_title_mixed());
        assert_eq!(app.get_frame_title(), "Dusk");

        // One frame alone: its own title.
        app.invoke_select(1);
        {
            let mut st = state.borrow_mut();
            st.views.picked.clear();
            show_set(&mut st, &app);
        }
        app.invoke_title_edited("".into());
        assert_eq!(state.borrow().sidecars[1].meta.title, "");
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");
    }

    /// A comma ends a word, which goes on at once, and what follows it
    /// stays in the field and is completed: from the frames' words
    /// in hand, each once whatever its case, none that every frame
    /// chosen has already, and nothing for nothing typed.
    #[test]
    fn a_comma_ends_a_word_and_the_rest_is_completed() {
        let app = window(3);
        let (state, _worker) = state_for(&app, folder(3));
        {
            let mut st = state.borrow_mut();
            st.sidecars[2]
                .meta
                .set_keywords(vec!["seaside".into(), "Sea".into()]);
        }
        app.invoke_select(0);
        app.invoke_keyword_typed("harbor, SE".into());
        assert_eq!(keywords_of(&state.borrow(), 0), ["harbor"]);
        assert_eq!(app.get_keyword_text(), "SE");
        let offered: Vec<String> = app
            .get_keyword_completions()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            offered,
            ["Sea", "seaside"],
            "by how many hold them, then by the word"
        );

        // The frame has "Sea" now: it is not offered again.
        app.invoke_keyword_added("sea".into());
        assert_eq!(completions(&state.borrow(), "s"), ["seaside"]);
        assert!(completions(&state.borrow(), " ").is_empty());
        assert!(completions(&state.borrow(), "x").is_empty());
    }

    /// The completions are the library's words first, the most held
    /// at the top, so a word on frames in another folder is offered;
    /// the frames in hand add what the index has not yet seen.
    #[test]
    fn the_completions_are_the_librarys_words_first() {
        use greycard_library::fixture::{R5, R6, write_frame};
        let dir = testing::scratch_dir("words-index");
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        for (i, (name, words)) in [
            ("a.tif", vec!["Harvest"]),
            ("b.tif", vec!["Harbor", "Harvest"]),
        ]
        .into_iter()
        .enumerate()
        {
            let f = other.join(name);
            write_frame(&f, if i == 0 { &R5 } else { &R6 }, i as u16 + 1);
            let mut s = greycard_edit::Sidecar::default();
            s.meta
                .set_keywords(words.into_iter().map(String::from).collect());
            s.save(&f).unwrap();
        }
        let mut lib = greycard_library::Library::open_in_memory().unwrap();
        lib.index_folder(&other, &mut |_| {}).unwrap();

        let app = window(2);
        let (state, _worker) = state_for(&app, folder(2));
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            st.sidecars[1].meta.add_keyword("Harlem");
            st.views.index_reader = Some(lib);
        }
        assert_eq!(
            completions(&state.borrow(), "har"),
            ["Harvest", "Harbor", "Harlem"]
        );
        drop(state);
        testing::remove_dir_retry(&dir);
    }

    /// In culling, a keyword and a title join the undo as a rating
    /// does, and the word over the picture says what the frame
    /// carries; and with Move on switched on a keyword leaves the
    /// selection where it is, since the next word typed is for the
    /// same frame.
    #[test]
    fn culling_undoes_a_keyword_and_a_title_and_does_not_move_on_for_them() {
        let app = window(3);
        let (state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            crate::panel::cull::enter_cull(&mut st, &app, 1);
            st.culling.cull_move_on = true;
        }
        app.invoke_keyword_added("sea".into());
        assert_eq!(app.get_notice(), "Keyword sea");
        app.invoke_title_edited("Dusk".into());
        assert_eq!(state.borrow().current, Some(0), "not moved on");
        assert_eq!(keywords_of(&state.borrow(), 0), ["sea"]);
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");

        app.invoke_undo();
        assert_eq!(state.borrow().sidecars[0].meta.title, "");
        assert_eq!(app.get_notice(), "Undo: No title");
        assert_eq!(app.get_frame_title(), "");
        app.invoke_undo();
        assert!(keywords_of(&state.borrow(), 0).is_empty());
        assert!(chips(&app).is_empty(), "the chip went with it");
        app.invoke_redo();
        assert_eq!(keywords_of(&state.borrow(), 0), ["sea"]);

        // A rating key still moves on, as ever.
        app.invoke_meta_key("3".into());
        assert_eq!(state.borrow().current, Some(1));
    }

    /// The words reach the disk as a rating does: the sidecar, the
    /// XMP beside it with XMPs on, and the index's row, whose words
    /// the next completion offers; the filter's keyword chips count
    /// them at once.
    #[test]
    fn the_words_reach_the_sidecar_the_xmp_and_the_index() {
        use crate::library::{Indexer, Told};
        use greycard_library::fixture::{R5, R6, write_frame};
        use std::time::Duration;
        let dir = testing::scratch_dir("words-disk");
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let files: Vec<PathBuf> = vec![shoot.join("a.tif"), shoot.join("b.tif")];
        write_frame(&files[0], &R5, 1);
        write_frame(&files[1], &R6, 2);
        let db = dir.join("data").join("library.sqlite");
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = Indexer::start(db.clone(), move |told| {
            let _ = tx.send(told);
        })
        .expect("the indexer starts");
        let wait = |want: &dyn Fn(&Told) -> bool| loop {
            let told = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("the indexer answers");
            if want(&told) {
                return;
            }
        };
        wait(&|t| matches!(t, Told::Opened(_)));
        indexer.folders(vec![shoot.clone()], 1);
        wait(&|t| matches!(t, Told::Indexed { .. }));

        let app = window(2);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.xmp_sidecars = true;
            st.views.index = Some(indexer);
            crate::panel::browser::rebuild_browser(&mut st, &app);
        }
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            st.views.picked = vec![0, 1];
            show_set(&mut st, &app);
        }
        app.invoke_keyword_added("Harbor".into());
        app.invoke_title_edited("Dusk".into());
        for f in &files {
            let s = greycard_edit::Sidecar::load(f).unwrap().expect("a sidecar");
            assert_eq!(s.meta.keywords, ["Harbor"], "{}", f.display());
            assert_eq!(s.meta.title, "Dusk");
            let xmp = greycard_edit::xmp::path_of(f).expect("an XMP");
            let packet = greycard_edit::xmp::read(&std::fs::read_to_string(&xmp).unwrap()).unwrap();
            assert_eq!(packet.meta.keywords, ["Harbor"]);
            assert_eq!(packet.meta.title, "Dusk");
        }
        // The filter's keyword chip counts both.
        let harbor = app
            .get_filter_facets()
            .iter()
            .flat_map(|row| row.chips.iter().collect::<Vec<_>>())
            .find(|c| c.text == "Harbor")
            .map(|c| c.count);
        assert_eq!(harbor, Some(2));

        // The index's rows, once the saves are in.
        wait(&|t| matches!(t, Told::FilesIndexed(_)));
        let reader = greycard_library::Library::open_read_only(&db).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !reader
            .keyword_counts()
            .unwrap()
            .contains(&("Harbor".to_string(), 2))
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the rows never had it"
            );
            wait(&|t| matches!(t, Told::FilesIndexed(_)));
        }
        assert!(
            reader
                .keyword_counts()
                .unwrap()
                .contains(&("Harbor".to_string(), 2))
        );
        let indexer = state.borrow_mut().views.index.take().unwrap();
        indexer.stop(Duration::from_secs(20));
        drop(worker);
        testing::remove_scratch(state, &dir);
    }

    /// The fields typed in, as a hand does: a click in the Keywords
    /// field, a few letters, Tab for the completion, a word and Enter,
    /// and Escape to give the keys back, after which 3 is a rating
    /// again. Then the Title field: typed and Enter is every frame's,
    /// and Escape puts what was there back. Not focused, the fields
    /// take no key at all.
    #[test]
    fn the_fields_take_typing_tab_enter_and_escape_and_give_the_keys_back() {
        let app = window(2);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(2));
        state.borrow_mut().sidecars[1].meta.add_keyword("seaside");
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());

        // Not focused: a 2 is the rating key's.
        press(&app, "2");
        assert_eq!(state.borrow().sidecars[0].meta.rating, 2);
        assert!(keywords_of(&state.borrow(), 0).is_empty());

        let (at, size) = testing::labeled(&app, "Add keywords");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        type_text(&app, "sea");
        assert_eq!(app.get_keyword_text(), "sea");
        assert_eq!(app.get_keyword_completions().row_count(), 1);
        press(&app, Key::Tab);
        assert_eq!(keywords_of(&state.borrow(), 0), ["seaside"]);
        assert_eq!(app.get_keyword_text(), "", "emptied for the next");
        type_text(&app, "dusk");
        press(&app, Key::Return);
        assert_eq!(keywords_of(&state.borrow(), 0), ["seaside", "dusk"]);
        assert_eq!(
            state.borrow().sidecars[0].meta.rating,
            2,
            "typed, not rated"
        );
        type_text(&app, "half");
        press(&app, Key::Escape);
        assert_eq!(app.get_keyword_text(), "", "Escape drops what was typed");
        press(&app, "3");
        assert_eq!(
            state.borrow().sidecars[0].meta.rating,
            3,
            "the keys are back"
        );

        let (at, size) = testing::labeled(&app, "Frame title");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        type_text(&app, "Dusk");
        press(&app, Key::Return);
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");
        let (at, size) = testing::labeled(&app, "Frame title");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        type_text(&app, "x");
        press(&app, Key::Escape);
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");
        assert_eq!(app.get_title_text(), "Dusk", "what was there, back");
        press(&app, "4");
        assert_eq!(state.borrow().sidecars[0].meta.rating, 4);
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");
    }

    /// The fields are where a hand looks for them in each view: on the
    /// Develop tab, under CULLING in culling, and in the left pane
    /// beside the grid, which covers the panel; once in each.
    #[test]
    fn the_fields_are_on_the_develop_tab_in_culling_and_beside_the_grid() {
        let app = window(2);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(2));
        app.invoke_select(0);
        testing::labeled(&app, "Add keywords");
        assert_eq!(testing::count_labeled(&app, "Frame title"), 1);
        crate::panel::cull::enter_cull(&mut state.borrow_mut(), &app, 1);
        assert_eq!(app.get_panel_tab(), "Cull");
        testing::labeled(&app, "Add keywords");
        assert_eq!(testing::count_labeled(&app, "Add keywords"), 1);
        app.set_grid_open(true);
        // The panel's own are under the grid, and one of the panel's
        // tabs is always up: the left pane's is the second.
        assert_eq!(testing::count_labeled(&app, "Add keywords"), 2);
    }

    /// A title typed and not yet sent is the frame's it was typed
    /// for when another frame is clicked in the strip: sent before the
    /// selection moves, not dropped and not put on the frame clicked.
    #[test]
    fn a_title_typed_goes_on_its_frame_when_another_is_clicked() {
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        let (at, size) = testing::labeled(&app, "Frame title");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        type_text(&app, "Dusk");
        let (x, y) = testing::strip_cell(1);
        testing::click(&app, x, y);
        settle();
        assert_eq!(state.borrow().current, Some(1), "the click chose it");
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");
        assert_eq!(state.borrow().sidecars[1].meta.title, "");
        assert_eq!(app.get_frame_title(), "");
        // The field shows the frame clicked, untyped: the next key is
        // a title of its own, not "Dusk" and more.
        assert_eq!(app.get_title_text(), "");
        assert!(!app.get_title_dirty());
        let (at, size) = testing::labeled(&app, "Frame title");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        type_text(&app, "x");
        press(&app, Key::Return);
        settle();
        assert_eq!(state.borrow().sidecars[1].meta.title, "x");
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");
    }

    /// Timers due now, run: what the window does on its next turn.
    fn settle() {
        slint::platform::update_timers_and_animations();
    }

    /// A right-click on a frame not chosen opens it at once, before
    /// the field hears it lost the focus: the title typed is still the
    /// first frame's, sent to it on the next turn, and neither put on
    /// the frame right-clicked nor left in the field over it.
    #[test]
    fn a_title_typed_stays_with_its_frame_through_a_right_click() {
        use slint::platform::{PointerEventButton, WindowEvent};
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        let (at, size) = testing::labeled(&app, "Frame title");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        type_text(&app, "Dusk");
        let (x, y) = testing::strip_cell(2);
        let position = slint::LogicalPosition::new(x, y);
        let button = PointerEventButton::Right;
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed { position, button },
            WindowEvent::PointerReleased { position, button },
        ] {
            app.window().dispatch_event(event);
        }
        press(&app, Key::Escape);
        settle();
        assert_eq!(state.borrow().current, Some(2), "the right-click opened it");
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");
        assert_eq!(state.borrow().sidecars[2].meta.title, "");
        assert_eq!(app.get_title_text(), "");
        assert!(!app.get_title_dirty());
    }

    /// The selection moved by a path that sends nothing first, two
    /// frames whose titles are both empty: the text typed goes to the
    /// frame it was typed for once the state is free, and the field
    /// starts again clean on the new one, the keyword half typed
    /// dropped with it.
    #[test]
    fn a_selection_moved_under_the_field_hands_the_title_to_its_frames() {
        let app = window(2);
        let (state, _worker) = state_for(&app, folder(2));
        app.invoke_select(0);
        app.set_title_text("Dusk".into());
        app.set_title_dirty(true);
        app.set_keyword_text("har".into());
        app.invoke_select(1);
        assert_eq!(app.get_title_text(), "", "the new frame's");
        assert!(!app.get_title_dirty());
        assert_eq!(app.get_keyword_text(), "");
        settle();
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");
        assert_eq!(state.borrow().sidecars[1].meta.title, "");
        // A blur that comes late sends nothing more.
        app.invoke_title_edited("".into());
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dusk");
    }

    /// The same frames in another order are the frames the field is
    /// typed for: a set put in another order does not start it again.
    #[test]
    fn the_same_frames_in_another_order_keep_the_field() {
        let key = |p: &str| crate::versions::Key {
            path: p.into(),
            version: None,
            name: None,
        };
        let (a, b, c) = (key("a"), key("b"), key("c"));
        assert!(same_frames(
            &[a.clone(), b.clone()],
            &[b.clone(), a.clone()]
        ));
        assert!(!same_frames(&[a.clone(), b.clone()], &[a.clone(), c]));
        assert!(!same_frames(std::slice::from_ref(&a), &[a.clone(), b]));

        let (app, state, _worker) = chosen(3);
        app.set_title_text("Dus".into());
        app.set_title_dirty(true);
        {
            let mut st = state.borrow_mut();
            st.views.picked = vec![2, 1, 0];
            show_set(&mut st, &app);
        }
        assert!(app.get_title_dirty(), "still being typed");
        assert_eq!(app.get_title_text(), "Dus");
    }

    /// Over a Mixed set, a field typed in and erased is not a title:
    /// nothing is sent, and every frame keeps its own.
    #[test]
    fn an_erased_field_over_a_mixed_set_clears_nothing() {
        let (app, state, _worker) = chosen(2);
        state.borrow_mut().sidecars[0].meta.set_title("Dawn");
        crate::panel::browser::show_frame_tags(&state.borrow(), &app);
        assert!(app.get_frame_title_mixed());
        app.invoke_title_edited("  ".into());
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dawn");
        // Typed and erased, then the selection gone: nothing either.
        app.set_title_dirty(true);
        app.set_title_text("".into());
        app.invoke_select(1);
        settle();
        assert_eq!(state.borrow().sidecars[0].meta.title, "Dawn");
        // Over a set that shares one, an empty title is a title.
        app.invoke_title_edited("".into());
        assert_eq!(state.borrow().sidecars[1].meta.title, "");
    }

    /// The window closed with a title typed and not sent, over two
    /// frames, one of them still standing in from its row: the quit, in
    /// the order `startup::main` makes it (the title, the import left,
    /// the worker stopped, the index closed), reads the stand-in there
    /// and then, and both frames have the title on disk, the stand-in
    /// its own stars beside it.
    #[test]
    fn a_title_typed_is_kept_when_the_window_closes() {
        use crate::library::{Indexer, Told};
        use greycard_library::fixture::{R5, R6, write_frame};
        use std::time::Duration;
        let dir = testing::scratch_dir("words-quit");
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let files = vec![shoot.join("a.tif"), shoot.join("b.tif")];
        write_frame(&files[0], &R5, 1);
        write_frame(&files[1], &R6, 2);
        let mut s = greycard_edit::Sidecar::default();
        s.meta.set_rating(3);
        s.meta.set_caption("kept");
        s.save(&files[1]).unwrap();
        let db = dir.join("data").join("library.sqlite");
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = Indexer::start(db, move |told| {
            let _ = tx.send(told);
        })
        .expect("the indexer starts");
        let wait = |want: &dyn Fn(&Told) -> bool| loop {
            let told = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("the indexer answers");
            if want(&told) {
                return;
            }
        };
        wait(&|t| matches!(t, Told::Opened(_)));
        indexer.folders(vec![shoot.clone()], 1);
        wait(&|t| matches!(t, Told::Indexed { .. }));

        let app = window(2);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.views.index = Some(indexer);
            // b stands in from its row, which carries its stars.
            st.from_row[1] = crate::rows::FromRow {
                read: false,
                ..crate::rows::FromRow::read()
            };
            st.sidecars[1].meta.set_rating(3);
            crate::panel::browser::rebuild_browser(&mut st, &app);
        }
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            st.views.picked = vec![0, 1];
            show_set(&mut st, &app);
            assert!(!crate::rows::is_loaded(&st, 1), "b still stands in");
        }
        app.set_title_text("Dusk".into());
        app.set_title_dirty(true);

        // The quit, after the event loop: no turn of it is left.
        send_title(&state, &app);
        crate::panel::import::leave(
            &mut state.borrow_mut().import,
            crate::panel::import::CLOSE_WAIT,
        );
        worker.stop();
        {
            let mut st = state.borrow_mut();
            st.views.index_reader = None;
            if let Some(indexer) = st.views.index.take() {
                indexer.stop(crate::worker::LEAVING);
            }
        }
        assert!(!app.get_title_dirty());
        for f in &files {
            let s = greycard_edit::Sidecar::load(f).unwrap().expect("a sidecar");
            assert_eq!(s.meta.title, "Dusk", "{}", f.display());
        }
        let b = greycard_edit::Sidecar::load(&files[1]).unwrap().unwrap();
        assert_eq!(b.meta.rating, 3);
        assert_eq!(b.meta.caption, "kept", "read before it was written");
        drop(state);
        testing::remove_dir_retry(&dir);
    }

    /// The vocabulary is read once and kept: a key typed does not read
    /// the index again, a word put on in the window is offered from
    /// memory, and an index pass has it read again. Spellings that
    /// differ in ASCII's case alone are one word, as on a frame.
    #[test]
    fn the_vocabulary_is_read_once_and_learns_the_words_put_on() {
        let app = window(2);
        let (state, _worker) = state_for(&app, folder(2));
        app.invoke_select(0);
        state.borrow_mut().sidecars[1]
            .meta
            .set_keywords(vec!["Harbor".into()]);
        assert_eq!(completions(&state.borrow(), "HAR"), ["Harbor"]);
        // Read: a word that arrives behind the window's back is not
        // seen until the vocabulary is read again.
        state.borrow_mut().sidecars[1].meta.add_keyword("Harvest");
        assert_eq!(completions(&state.borrow(), "har"), ["Harbor"]);
        // A word the window puts on is offered from memory.
        app.invoke_keyword_added("Hart".into());
        app.invoke_select(1);
        assert_eq!(completions(&state.borrow(), "har"), ["Hart"]);
        // An index pass: read again, the late word with it, over the
        // first frame, which has Hart.
        forget_vocabulary(&mut state.borrow_mut());
        app.invoke_select(0);
        assert_eq!(completions(&state.borrow(), "har"), ["Harbor", "Harvest"]);
        // Taken off the one frame that held it: gone from memory too,
        // and not offered over the other frame.
        app.invoke_keyword_removed("hart".into());
        app.invoke_select(1);
        assert!(completions(&state.borrow(), "hart").is_empty());
        // Harbor, on one frame and taken off it, is gone; on two, it
        // would be offered still.
        app.invoke_keyword_removed("Harbor".into());
        app.invoke_select(0);
        assert_eq!(completions(&state.borrow(), "har"), ["Harvest"]);
    }
}
