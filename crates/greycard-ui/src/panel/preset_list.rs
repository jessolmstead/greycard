//! The PRESETS list in sections: the favorites, under a header of
//! their own; the presets in no group, under none; then each group,
//! under its name. A preset's star makes it a favorite, and a favorite
//! stays in its own section too. A header folds its section. A preset
//! moves into a group, a new one or one there, and out of any; a group
//! is renamed from its header.
//!
//! A group is a folder of the preset store (`preset::Store`). The
//! favorites and the folds are the settings', written as they change:
//! a favorite by its file's path under the store (`Store::key_of`),
//! which a move or a group renamed here carries along, and a fold by
//! its section's key ([`FAVORITES`], [`group_key`]).

use std::path::PathBuf;
use std::rc::Rc;

use greycard_edit::Error;
use greycard_edit::preset::{self, Entry, Store};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::panel::assets::refresh_presets;
use crate::{App, PresetRow, Shared, State, settings, tr};

/// The Favorites section's fold key.
pub(crate) const FAVORITES: &str = "favorites";

/// A group's section's fold key.
pub(crate) fn group_key(group: &str) -> String {
    format!("group:{group}")
}

/// What the group sheet is asking for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GroupAsked {
    /// A new group's name, for the preset whose file this is.
    New(PathBuf),
    /// A group's new name.
    Rename(String),
}

/// The list's rows: the Favorites header and the favorites by name
/// (none of either without a favorite), the presets in no group, and
/// each group's header and presets, in `presets`' order, which is the
/// store's ([`Store::list`]). A folded section is its header alone. A
/// group with no preset in it has no header.
pub(crate) fn rows(
    store: Option<&Store>,
    presets: &[Entry],
    favorites: &[String],
    folds: &[String],
) -> Vec<PresetRow> {
    let group_of = |e: &Entry| store.and_then(|s| s.group_of(&e.path));
    let favorite = |e: &Entry| {
        store
            .and_then(|s| s.key_of(&e.path))
            .is_some_and(|k| favorites.contains(&k))
    };
    let folded = |key: &str| folds.iter().any(|f| f == key);
    let preset = |i: usize, e: &Entry, under: bool| PresetRow {
        header: false,
        text: e.preset.name.as_str().into(),
        index: i as i32,
        group: group_of(e).unwrap_or_default().into(),
        favorite: favorite(e),
        folded: false,
        key: SharedString::new(),
        under,
    };
    let header = |group: &str, key: &str| PresetRow {
        header: true,
        text: group.into(),
        index: -1,
        group: group.into(),
        favorite: false,
        folded: folded(key),
        key: key.into(),
        under: false,
    };
    let mut out = Vec::new();
    let mut favs: Vec<(usize, &Entry)> = presets
        .iter()
        .enumerate()
        .filter(|(_, e)| favorite(e))
        .collect();
    if !favs.is_empty() {
        out.push(header("", FAVORITES));
        if !folded(FAVORITES) {
            favs.sort_by_key(|(i, e)| (e.preset.name.to_lowercase(), *i));
            out.extend(favs.into_iter().map(|(i, e)| preset(i, e, true)));
        }
    }
    out.extend(
        presets
            .iter()
            .enumerate()
            .filter(|(_, e)| group_of(e).is_none())
            .map(|(i, e)| preset(i, e, false)),
    );
    let mut current: Option<String> = None;
    for (i, e) in presets.iter().enumerate() {
        let Some(group) = group_of(e) else {
            continue;
        };
        let key = group_key(&group);
        if current.as_deref() != Some(group.as_str()) {
            out.push(header(&group, &key));
            current = Some(group);
        }
        if !folded(&key) {
            out.push(preset(i, e, true));
        }
    }
    out
}

/// Put the rows and the groups on the panel.
pub(crate) fn show(st: &State, app: &App) {
    let store = st.develop.preset_store.as_ref();
    let rows = rows(
        store,
        &st.develop.presets,
        &st.develop.preset_favorites,
        &st.develop.preset_folds,
    );
    app.set_preset_list_rows(ModelRc::new(VecModel::from(rows)));
    let groups: Vec<SharedString> = store
        .map(|s| s.groups())
        .unwrap_or_default()
        .into_iter()
        .map(SharedString::from)
        .collect();
    app.set_preset_groups(ModelRc::new(VecModel::from(groups)));
}

/// Drop the favorites no preset file has and the folds of groups
/// that are gone, and write what is left: a star of a preset removed
/// by hand would otherwise pass to a later file of its path, and a
/// dead group's fold start a new group of its name folded. Not while
/// the store's folder cannot be read, when every key would look dead.
pub(crate) fn prune(st: &mut State) {
    let Some(store) = st.develop.preset_store.as_ref() else {
        return;
    };
    if !store.dir().is_dir() {
        return;
    }
    // The files on disk, read or not: a preset that fails to load (one
    // from a newer build, a typo by hand) is still there and keeps its
    // star. A folder that cannot be read keeps every star in it.
    let mut keys: Vec<String> = Vec::new();
    let mut unread: Vec<String> = Vec::new();
    let store_groups = store.groups();
    for group in std::iter::once(None).chain(store_groups.iter().map(Some)) {
        let Ok(dir) = std::fs::read_dir(store.group_dir(group.map(String::as_str))) else {
            unread.push(group.cloned().unwrap_or_default());
            continue;
        };
        keys.extend(
            dir.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == preset::EXTENSION))
                .filter_map(|p| store.key_of(&p)),
        );
    }
    let folder_of = |key: &str| key.rsplit_once('/').map_or("", |(g, _)| g).to_string();
    let keys: Vec<String> = keys
        .into_iter()
        .chain(
            st.develop
                .preset_favorites
                .iter()
                .filter(|f| unread.contains(&folder_of(f)))
                .cloned(),
        )
        .collect();
    let groups: Vec<String> = store_groups.iter().map(|g| group_key(g)).collect();
    let (favorites, folds) = (
        st.develop.preset_favorites.len(),
        st.develop.preset_folds.len(),
    );
    st.develop.preset_favorites.retain(|f| keys.contains(f));
    st.develop
        .preset_folds
        .retain(|f| f == FAVORITES || groups.contains(f));
    if favorites != st.develop.preset_favorites.len() || folds != st.develop.preset_folds.len() {
        keep(st);
    }
}

/// Write the favorites and the folds to the settings, as they are
/// now; a batch run writes none.
fn keep(st: &State) {
    let Some(file) = st.window.settings_file.clone().filter(|_| !st.window.batch) else {
        return;
    };
    let favorites = st.develop.preset_favorites.clone();
    let folds = st.develop.preset_folds.clone();
    settings::Settings::change_at(&file, |kept| {
        kept.preset_favorites = favorites;
        kept.preset_folds = folds;
    });
}

/// A preset's key in the store, by its place in the list.
fn key_at(st: &State, i: i32) -> Option<String> {
    let entry = st.develop.presets.get(usize::try_from(i).ok()?)?;
    st.develop.preset_store.as_ref()?.key_of(&entry.path)
}

/// The preset at `i` made a favorite, or one no longer.
pub(crate) fn toggle_favorite(st: &mut State, app: &App, i: i32) {
    let Some(key) = key_at(st, i) else {
        return;
    };
    let favorites = &mut st.develop.preset_favorites;
    if let Some(at) = favorites.iter().position(|f| *f == key) {
        favorites.remove(at);
    } else {
        favorites.push(key);
    }
    keep(st);
    show(st, app);
}

/// A section folded, or opened again.
pub(crate) fn toggle_fold(st: &mut State, app: &App, key: &str) {
    let folds = &mut st.develop.preset_folds;
    if let Some(at) = folds.iter().position(|f| f == key) {
        folds.remove(at);
    } else {
        folds.push(key.to_string());
    }
    keep(st);
    show(st, app);
}

/// A preset removed from the store: no favorite any more. Called with
/// its key before it goes.
pub(crate) fn forget(st: &mut State, key: &str) {
    let before = st.develop.preset_favorites.len();
    st.develop.preset_favorites.retain(|f| f != key);
    if st.develop.preset_favorites.len() != before {
        keep(st);
    }
}

/// An error's own words: a refusal's, without the "preset: " before it.
fn why(e: &Error) -> String {
    match e {
        Error::Preset(why) => why.clone(),
        e => preset::plain(e),
    }
}

/// Move the preset whose file is `path` into `group`, or out of any
/// for none, its star going along. Says why not, when not.
fn move_preset(
    st: &mut State,
    app: &App,
    path: &std::path::Path,
    group: Option<&str>,
) -> Result<(), String> {
    let Some(store) = st.develop.preset_store.clone() else {
        return Err(tr!("no configuration directory to keep presets in"));
    };
    // The list read again under the sheet: the preset is not where
    // it was asked for, and saying so beats a sheet that closes on
    // nothing done.
    let Some(entry) = st.develop.presets.iter().find(|e| e.path == path).cloned() else {
        return Err(tr!("{} is no longer in the list", preset::stem_name(path)));
    };
    let old = store.key_of(&entry.path);
    let to = store.move_to(&entry, group).map_err(|e| {
        tracing::warn!("preset {}: not moved: {e}", entry.preset.name);
        why(&e)
    })?;
    tracing::info!("preset {} moved to {}", entry.preset.name, to.display());
    if let (Some(old), Some(new)) = (old, store.key_of(&to))
        && let Some(f) = st.develop.preset_favorites.iter_mut().find(|f| **f == old)
    {
        *f = new;
        keep(st);
    }
    refresh_presets(st, app);
    Ok(())
}

/// A group renamed, the stars and the fold of its presets going along.
fn rename_group(st: &mut State, app: &App, old: &str, new: &str) -> Result<(), String> {
    let Some(store) = st.develop.preset_store.clone() else {
        return Err(tr!("no configuration directory to keep presets in"));
    };
    let new = store.rename_group(old, new).map_err(|e| {
        tracing::warn!("preset group {old}: not renamed: {e}");
        why(&e)
    })?;
    tracing::info!("preset group {old} renamed {new}");
    let (from, to) = (format!("{old}/"), format!("{new}/"));
    for f in &mut st.develop.preset_favorites {
        if let Some(file) = f.strip_prefix(&from) {
            *f = format!("{to}{file}");
        }
    }
    let (from, to) = (group_key(old), group_key(&new));
    for f in &mut st.develop.preset_folds {
        if *f == from {
            f.clone_from(&to);
        }
    }
    keep(st);
    refresh_presets(st, app);
    Ok(())
}

/// Put the group sheet up for `asked`, unless another sheet is up.
fn ask(st: &mut State, app: &App, asked: GroupAsked) {
    if app.invoke_sheet_open() {
        return;
    }
    let (name, verb) = match &asked {
        GroupAsked::New(_) => (String::new(), tr!("Create")),
        GroupAsked::Rename(group) => (group.clone(), tr!("Rename")),
    };
    app.set_preset_group_sheet_name(name.into());
    app.set_preset_group_sheet_verb(verb.into());
    app.set_preset_group_sheet_note("".into());
    st.develop.preset_group_asked = Some(asked);
    app.set_preset_group_sheet_open(true);
}

/// The group sheet answered: the preset moved into the group named,
/// or the group renamed. A name that is nothing as a folder's keeps
/// the sheet up, and so does a refusal, which it then says.
fn sheet_done(st: &mut State, app: &App) {
    if !app.get_preset_group_sheet_open() {
        return;
    }
    let typed = app.get_preset_group_sheet_name().trim().to_string();
    let Some(asked) = st.develop.preset_group_asked.take() else {
        app.set_preset_group_sheet_open(false);
        return;
    };
    if preset::group_folder(&typed).is_none() {
        if !typed.is_empty() {
            app.set_preset_group_sheet_note(tr!("a group needs a name").into());
        }
        st.develop.preset_group_asked = Some(asked);
        return;
    }
    let done = match &asked {
        GroupAsked::New(path) => move_preset(st, app, path, Some(&typed)),
        GroupAsked::Rename(old) => rename_group(st, app, old, &typed),
    };
    match done {
        Ok(()) => app.set_preset_group_sheet_open(false),
        Err(why) => {
            app.set_preset_group_sheet_note(why.into());
            st.develop.preset_group_asked = Some(asked);
        }
    }
}

pub(crate) fn install(app: &App, state: &Rc<Shared>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_favorite_toggled(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            toggle_favorite(&mut state.borrow_mut(), &app, i);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_fold_toggled(move |key| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            toggle_fold(&mut state.borrow_mut(), &app, &key);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_moved(move |i, group| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(path) = usize::try_from(i)
                .ok()
                .and_then(|i| st.develop.presets.get(i))
                .map(|e| e.path.clone())
            else {
                return;
            };
            match usize::try_from(group)
                .ok()
                .and_then(|g| app.get_preset_groups().row_data(g))
            {
                Some(group) => {
                    if let Err(why) = move_preset(&mut st, &app, &path, Some(&group)) {
                        app.set_status(why.into());
                    }
                }
                None => ask(&mut st, &app, GroupAsked::New(path)),
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_ungrouped(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(path) = usize::try_from(i)
                .ok()
                .and_then(|i| st.develop.presets.get(i))
                .map(|e| e.path.clone())
            else {
                return;
            };
            if let Err(why) = move_preset(&mut st, &app, &path, None) {
                app.set_status(why.into());
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_group_rename(move |group| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ask(
                &mut state.borrow_mut(),
                &app,
                GroupAsked::Rename(group.to_string()),
            );
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_group_sheet_done(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            sheet_done(&mut state.borrow_mut(), &app);
        });
    }
}

#[cfg(test)]
mod tests {
    use greycard_edit::{Edit, Preset, Section};

    use super::*;

    /// The rows as (header, text, under, favorite), for a test to read.
    fn shown(app: &App) -> Vec<(bool, String, bool, bool)> {
        app.get_preset_list_rows()
            .iter()
            .map(|r| (r.header, r.text.to_string(), r.under, r.favorite))
            .collect()
    }

    fn save_in(store: &Store, group: Option<&str>, name: &str) {
        let dir = store.group_dir(group);
        std::fs::create_dir_all(&dir).unwrap();
        Preset::from_edit(name, &Edit::default(), &[Section::Light])
            .save_to(&dir.join(format!("{}.gcp", preset::slug(name))))
            .unwrap();
    }

    /// The star toggles a favorite, which then heads the list under
    /// Favorites and stays in its own group too; the settings file has
    /// it, and a window started from them shows it again. A fold hides
    /// a section's presets and is kept. Moved into a new group by the
    /// sheet, and out again, a preset keeps its star; a group renamed
    /// keeps its presets' stars and its fold.
    #[test]
    fn the_star_toggles_and_favorites_head_the_list() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let d = crate::testing::scratch_dir("preset-favorites");
        std::fs::create_dir_all(&d).unwrap();
        let store = Store::at(d.join("presets"));
        save_in(&store, None, "Zed");
        save_in(&store, None, "Alpha");
        save_in(&store, Some("Film"), "Portra");
        let file = d.join("settings.json");
        {
            let mut st = state.borrow_mut();
            st.develop.preset_store = Some(store.clone());
            st.window.settings_file = Some(file.clone());
        }
        refresh_presets(&mut state.borrow_mut(), &app);
        let row = |header: bool, text: &str, under: bool, fav: bool| {
            (header, text.to_string(), under, fav)
        };
        assert_eq!(
            shown(&app),
            [
                row(false, "Alpha", false, false),
                row(false, "Zed", false, false),
                row(true, "Film", false, false),
                row(false, "Portra", true, false),
            ]
        );
        let names: Vec<String> = app
            .get_preset_names()
            .iter()
            .map(|n| n.to_string())
            .collect();
        assert_eq!(names, ["Alpha", "Zed", "Portra"]);

        // The star itself, clicked where the layout put it: Alpha's,
        // the first row's. Its favorite is then listed twice, and its
        // star there takes it off again.
        const STAR: &str = "PresetsSection::favorite-star";
        // The stars, filled or not, as a screen reader is told.
        let stars = |filled: bool| {
            crate::testing::all_by_id(&app, STAR)
                .iter()
                .filter(|e| e.accessible_checked() == Some(filled))
                .count()
        };
        assert_eq!(stars(false), 3);
        // Alpha's star, the topmost of its two rows when listed twice.
        let click_alpha = || crate::testing::click_first_key(&app, STAR, 0);
        click_alpha();
        assert_eq!(
            settings::Settings::load_from(&file).preset_favorites,
            ["alpha.gcp"]
        );
        assert_eq!(
            shown(&app)[..2],
            [row(true, "", false, false), row(false, "Alpha", true, true)]
        );
        assert_eq!(stars(true), 2);
        click_alpha();
        assert!(
            settings::Settings::load_from(&file)
                .preset_favorites
                .is_empty()
        );
        assert_eq!(stars(false), 3);

        // Portra's star.
        app.invoke_preset_favorite_toggled(2);
        assert_eq!(
            shown(&app),
            [
                row(true, "", false, false),
                row(false, "Portra", true, true),
                row(false, "Alpha", false, false),
                row(false, "Zed", false, false),
                row(true, "Film", false, false),
                row(false, "Portra", true, true),
            ]
        );
        let favorites_header = app.get_preset_list_rows().row_data(0).unwrap();
        assert_eq!(favorites_header.key, FAVORITES);
        assert_eq!(app.get_preset_list_rows().row_data(1).unwrap().index, 2);
        assert_eq!(
            settings::Settings::load_from(&file).preset_favorites,
            ["Film/portra.gcp"]
        );
        // Another, and the favorites by name.
        app.invoke_preset_favorite_toggled(1);
        let favs: Vec<String> = shown(&app)[1..3].iter().map(|r| r.1.clone()).collect();
        assert_eq!(favs, ["Portra", "Zed"]);
        app.invoke_preset_favorite_toggled(1);

        // Folded: the header alone, and kept.
        app.invoke_preset_fold_toggled("group:Film".into());
        assert_eq!(
            shown(&app).last().unwrap(),
            &row(true, "Film", false, false)
        );
        assert!(app.get_preset_list_rows().iter().last().unwrap().folded);
        assert_eq!(
            settings::Settings::load_from(&file).preset_folds,
            ["group:Film"]
        );
        app.invoke_preset_fold_toggled("group:Film".into());

        // A window started from the file has the star.
        let kept = settings::Settings::load_from(&file);
        let app2 = crate::testing::window(1);
        let (state2, _worker2) = crate::testing::retouch_state(&app2);
        {
            let mut st = state2.borrow_mut();
            st.develop.preset_store = Some(store.clone());
            st.develop.preset_favorites = kept.preset_favorites;
            st.develop.preset_folds = kept.preset_folds;
        }
        refresh_presets(&mut state2.borrow_mut(), &app2);
        assert_eq!(shown(&app2)[1], row(false, "Portra", true, true));

        // Into a new group by the sheet: the star goes along, and the
        // empty group it left goes.
        app.invoke_preset_moved(2, -1);
        assert!(app.get_preset_group_sheet_open());
        assert_eq!(app.get_preset_group_sheet_verb(), "Create");
        app.set_preset_group_sheet_name("  ".into());
        app.invoke_preset_group_sheet_done();
        assert!(
            app.get_preset_group_sheet_open(),
            "a blank name keeps it up"
        );
        assert_eq!(app.get_preset_group_sheet_note(), "");
        app.set_preset_group_sheet_name(" .. ".into());
        app.invoke_preset_group_sheet_done();
        assert!(app.get_preset_group_sheet_open());
        assert_eq!(app.get_preset_group_sheet_note(), "a group needs a name");
        app.set_preset_group_sheet_name("Color".into());
        app.invoke_preset_group_sheet_done();
        assert!(!app.get_preset_group_sheet_open());
        assert!(store.dir().join("Color").join("portra.gcp").is_file());
        assert!(!store.dir().join("Film").exists());
        assert_eq!(
            settings::Settings::load_from(&file).preset_favorites,
            ["Color/portra.gcp"]
        );
        let groups: Vec<String> = app
            .get_preset_groups()
            .iter()
            .map(|g| g.to_string())
            .collect();
        assert_eq!(groups, ["Color"]);

        // Zed into it by the menu's group, then the group renamed;
        // a name another group has is refused on the sheet.
        app.invoke_preset_moved(1, 0);
        assert_eq!(
            store.group_of(&store.find("Zed").unwrap().path).as_deref(),
            Some("Color")
        );
        std::fs::create_dir_all(store.dir().join("Mono")).unwrap();
        app.invoke_preset_fold_toggled("group:Color".into());
        app.invoke_preset_group_rename("Color".into());
        assert_eq!(app.get_preset_group_sheet_name(), "Color");
        assert_eq!(app.get_preset_group_sheet_verb(), "Rename");
        app.set_preset_group_sheet_name("mono".into());
        app.invoke_preset_group_sheet_done();
        assert!(app.get_preset_group_sheet_open());
        assert_eq!(
            app.get_preset_group_sheet_note(),
            "there is a group called \"Mono\" already"
        );
        app.set_preset_group_sheet_name("Color Film".into());
        app.invoke_preset_group_sheet_done();
        assert!(!app.get_preset_group_sheet_open());
        let kept = settings::Settings::load_from(&file);
        assert_eq!(kept.preset_favorites, ["Color Film/portra.gcp"]);
        assert_eq!(kept.preset_folds, ["group:Color Film"]);

        // Out of the group: the star still on, and Zed with it.
        let at = |name: &str| {
            state
                .borrow()
                .develop
                .presets
                .iter()
                .position(|e| e.preset.name == name)
                .unwrap() as i32
        };
        app.invoke_preset_ungrouped(at("Portra"));
        app.invoke_preset_ungrouped(at("Zed"));
        assert_eq!(
            shown(&app),
            [
                row(true, "", false, false),
                row(false, "Portra", true, true),
                row(false, "Alpha", false, false),
                row(false, "Portra", false, true),
                row(false, "Zed", false, false),
            ]
        );
        assert_eq!(
            settings::Settings::load_from(&file).preset_favorites,
            ["portra.gcp"]
        );

        // Removed: no favorite left behind.
        app.invoke_preset_deleted(at("Portra"));
        assert!(
            settings::Settings::load_from(&file)
                .preset_favorites
                .is_empty()
        );
        assert_eq!(
            shown(&app),
            [
                row(false, "Alpha", false, false),
                row(false, "Zed", false, false),
            ]
        );

        // A star and a fold that match nothing on disk, a preset gone
        // by hand and a group gone, are dropped and the file with them.
        // A preset that will not read is still there: its star stays.
        std::fs::write(store.dir().join("typo.gcp"), "{ not json").unwrap();
        {
            let mut st = state.borrow_mut();
            st.develop.preset_favorites =
                vec!["zed.gcp".into(), "gone.gcp".into(), "typo.gcp".into()];
            st.develop.preset_folds = vec![FAVORITES.into(), "group:Gone".into()];
        }
        refresh_presets(&mut state.borrow_mut(), &app);
        let kept = settings::Settings::load_from(&file);
        assert_eq!(kept.preset_favorites, ["zed.gcp", "typo.gcp"]);
        assert_eq!(kept.preset_folds, [FAVORITES]);
        std::fs::remove_file(store.dir().join("typo.gcp")).unwrap();

        // The list read again under the sheet, the preset gone: the
        // sheet says so rather than close on nothing done.
        app.invoke_preset_moved(at("Alpha"), -1);
        std::fs::remove_file(store.dir().join("alpha.gcp")).unwrap();
        refresh_presets(&mut state.borrow_mut(), &app);
        app.set_preset_group_sheet_name("Later".into());
        app.invoke_preset_group_sheet_done();
        assert!(app.get_preset_group_sheet_open());
        assert_eq!(
            app.get_preset_group_sheet_note(),
            "alpha is no longer in the list"
        );
        assert!(!store.dir().join("Later").exists());
        app.set_preset_group_sheet_open(false);
        crate::testing::remove_dir_retry(&d);
    }

    /// An import of a Lightroom preset folder in the window makes the
    /// group, and the list shows it under its header.
    #[test]
    fn a_folder_imported_in_the_window_is_a_group_in_the_list() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let d = crate::testing::scratch_dir("preset-folder-window");
        let pack = d.join("Soft Pack");
        std::fs::create_dir_all(&pack).unwrap();
        let xmp = |ev: &str| {
            format!(
                r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="{ev}"/></rdf:RDF></x:xmpmeta>"#
            )
        };
        std::fs::write(pack.join("soft one.xmp"), xmp("+0.3")).unwrap();
        std::fs::write(pack.join("soft two.xmp"), xmp("-0.3")).unwrap();
        // Lightroom's older kind in the same pack, its title its name.
        std::fs::write(
            pack.join("old.lrtemplate"),
            "s = { title = \"soft three\", type = \"Develop\", \
             value = { settings = { Exposure2012 = 0.1 } } }",
        )
        .unwrap();
        let store = Store::at(d.join("presets"));
        state.borrow_mut().develop.preset_store = Some(store.clone());
        crate::panel::assets::import_presets(&mut state.borrow_mut(), &app, &[pack]);
        assert_eq!(store.groups(), ["Soft Pack"]);
        assert_eq!(
            shown(&app),
            [
                (true, "Soft Pack".to_string(), false, false),
                (false, "soft one".into(), true, false),
                (false, "soft three".into(), true, false),
                (false, "soft two".into(), true, false),
            ]
        );
        crate::testing::remove_dir_retry(&d);
    }
}
