//! `greycard library`: the index brought up to date, listed by filter,
//! and a Lightroom catalog brought across.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum LibraryAction {
    /// Index folders: a file the index knows by size and mtime is not
    /// read, a changed sidecar refreshes only its meta, a file found
    /// under a new path by its content hash is a move, a folder gone
    /// marks its files missing. Prints what was done and how long it
    /// took
    Index {
        /// The folders to index
        dirs: Vec<PathBuf>,
        /// Their subfolders too, hidden ones left alone and links to
        /// folders not followed
        #[arg(long)]
        tree: bool,
        /// Afterwards, forget every file in the whole library last
        /// found gone from its path, not only under the folders given
        #[arg(long)]
        prune: bool,
    },
    /// List the files that pass a filter. Each argument is one term,
    /// so quote a term with a space or a shell character in it:
    /// `list camera:R6 'iso>=3200' 'rating>=3' flag:pick
    /// keyword:wedding date:2026-09 'lens:RF 24'`. A bare word tests
    /// the name and the keywords; `:` on a text field is contains,
    /// `=` and `!=` the whole value; numbers take `< <= > >=`; a date
    /// is a prefix, `2026-09`; `kind:raw` or `kind!=jpeg` tests what
    /// the file is; `missing:yes` lists what is gone. A camera's JPEG
    /// beside its raw is the raw's and not listed, unless the editor's
    /// settings list such JPEGs apart
    List {
        /// The filter's terms, one an argument
        filter: Vec<String>,
        /// Only the paths, one a line
        #[arg(long)]
        paths: bool,
        /// Only how many
        #[arg(long)]
        count: bool,
        /// A stack shows its top alone, `collapsed`, as the editor's
        /// grid shows a closed one, or every frame of it, `expanded`;
        /// either way its top says how many it holds
        #[arg(long, value_name = "SHOWN", default_value = "collapsed",
              value_parser = ["collapsed", "expanded"])]
        stacks: String,
        /// The order, as the editor's grid has it over every root: by
        /// `capture` time, `name`, `rating` or date `edited`, and
        /// `-desc` after it for the other way up (`capture-desc` is the
        /// newest first). Ties go by name, then path; a frame with no
        /// capture time, or no develop for `edited`, comes last
        #[arg(long, value_name = "ORDER", default_value = "capture",
              value_parser = clap::builder::PossibleValuesParser::new(SORT_KEYS))]
        sort: String,
    },
    /// Bring a Lightroom Classic catalog across: the ratings, flags,
    /// labels, keywords, titles and captions exactly, the develop
    /// settings as an approximation in one named state with the crop,
    /// virtual copies as versions, the collections and the smart
    /// collections that translate, the stacks, the root folders as
    /// roots (a root not mapped is looked for where the catalog says,
    /// then in a folder of its name beside the catalog or under the
    /// library's roots). Without --write it
    /// only reports what it would do. Work on a copy of the catalog,
    /// made after Lightroom quit or with the -wal beside it: without
    /// its -wal, a catalog copied while Lightroom ran lacks the newest
    /// changes
    ImportLightroom {
        /// The .lrcat
        catalog: PathBuf,
        /// Where a root folder of the catalog is now: its path as the
        /// catalog has it (or a folder above it, or its name), `=`,
        /// and the folder here. With none, each root is looked for
        /// where the catalog says, then beside the catalog and under
        /// the library's roots by its name
        #[arg(long = "root", value_name = "CATALOG=HERE", value_parser = root_mapping)]
        roots: Vec<(String, PathBuf)>,
        /// Write the sidecars, the collections and the roots
        #[arg(long)]
        write: bool,
        /// Write over sidecars that have history, versions or meta
        #[arg(long, requires = "write")]
        force: bool,
        /// The set the collections go under; the catalog's name when
        /// not given, none when empty
        #[arg(long, value_name = "NAME")]
        set: Option<String>,
        /// Leave out the books, prints, slideshows and web pages, which
        /// otherwise come as collections of their frames
        #[arg(long)]
        no_creations: bool,
    },
}

/// One of the editor's switches, read from its settings file
/// (`$XDG_CONFIG_HOME/greycard/settings.json`, else under the
/// platform's configuration folder), so that what the command line does
/// is what the window would: `sidecars_in_folder`, where a new sidecar
/// goes; `jpegs_apart`, whether a camera's JPEG beside its raw is a
/// frame of its own. Off when there is no file, it will not read, or it
/// does not say, as the editor's defaults are.
fn editor_setting(key: &str) -> bool {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::config_dir)
        .is_some_and(|base| editor_setting_in(&base.join("greycard").join("settings.json"), key))
}

/// [`editor_setting`] as the settings file at `path` has it.
fn editor_setting_in(path: &std::path::Path, key: &str) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get(key)?.as_bool())
        .unwrap_or(false)
}

/// A `--root` as typed: the catalog's side up to the first `=`, the
/// folder here after it.
fn root_mapping(text: &str) -> Result<(String, PathBuf), String> {
    match text.split_once('=') {
        Some((from, to)) if !from.trim().is_empty() && !to.trim().is_empty() => {
            Ok((from.trim().to_string(), PathBuf::from(to.trim())))
        }
        _ => Err(format!(
            "want <catalog path>=<folder here>, as A:/Photos=/photos, not {text}"
        )),
    }
}

/// The library subcommand: the index brought up to date, or listed.
pub(crate) fn library(db: Option<PathBuf>, action: LibraryAction) -> Result<()> {
    use greycard_library::{Filter, Library};
    let path = match db {
        Some(path) => path,
        None => Library::user_path().context("no data directory for the library")?,
    };
    // The import needs no index: it writes sidecars, the collections
    // and the roots beside the library, and its dry run writes nothing,
    // the library itself included.
    if let LibraryAction::ImportLightroom {
        catalog,
        roots,
        write,
        force,
        set,
        no_creations,
    } = action
    {
        let options = greycard_library::lightroom_catalog::Options {
            roots,
            write,
            force,
            set,
            library: path,
            sidecars_in_folder: editor_setting("sidecars_in_folder"),
            no_creations,
        };
        let report = greycard_library::lightroom_catalog::import(&catalog, &options)
            .with_context(|| format!("importing {}", catalog.display()))?;
        print!("{report}");
        return Ok(());
    }
    // A listing reads only, so it opens read-only and never waits
    // for an index that is running; a library not there yet is made,
    // so the first listing says "0 of 0" rather than "no such file".
    let reading = matches!(action, LibraryAction::List { .. }) && path.is_file();
    let mut lib = if reading {
        Library::open_read_only(&path)
    } else {
        Library::open(&path)
    }
    .with_context(|| format!("opening the library at {}", path.display()))?;
    lib.set_jpegs_apart(editor_setting("jpegs_apart"));
    match action {
        LibraryAction::Index { dirs, tree, prune } => {
            if dirs.is_empty() && !prune {
                anyhow::bail!("say which folders to index");
            }
            for dir in &dirs {
                let start = std::time::Instant::now();
                let report = if tree {
                    lib.index_tree(dir, &mut |_| {})
                } else {
                    lib.index_folder(dir, &mut |_| {})
                }
                .with_context(|| format!("indexing {}", dir.display()))?;
                let took = start.elapsed();
                println!(
                    "{:<13} {} files in {:.2} s: {} added, {} moved, {} changed, {} meta \
                     refreshed, {} unchanged, {} returned, {} missing",
                    "indexed",
                    report.seen(),
                    took.as_secs_f32(),
                    report.added,
                    report.moved,
                    report.changed,
                    report.meta_refreshed,
                    report.unchanged,
                    report.returned,
                    report.missing
                );
                for (path, why) in &report.errors {
                    println!("{:<13} {}: {why}", "not read", path.display());
                }
                if !report.unavailable.is_empty() {
                    println!(
                        "{:<13} {} folders empty on disk, left as they were:",
                        "unavailable",
                        report.unavailable.len()
                    );
                    for folder in &report.unavailable {
                        println!("              {}", folder.display());
                    }
                }
                for link in &report.skipped {
                    println!(
                        "{:<13} {} is a link; not followed",
                        "skipped",
                        link.display()
                    );
                }
            }
            if prune {
                let gone = lib.prune_missing().context("pruning")?;
                println!("{:<13} {gone} missing files forgotten", "pruned");
            }
            lib.checkpoint().ok();
            println!(
                "{:<13} {} ({} files, {} bytes)",
                "library",
                lib.path().display(),
                lib.len()?,
                lib.size_on_disk().unwrap_or(0)
            );
            Ok(())
        }
        LibraryAction::List {
            filter,
            paths,
            count,
            stacks,
            sort,
        } => {
            let filter = Filter::from_terms(&filter)?;
            let order = greycard_library::Order::from_key(&sort)
                .with_context(|| format!("--sort {sort}: not an order"))?;
            let stacked = stacks_over(&lib, &path, stacks == "expanded");
            if count && stacked.hidden.is_empty() {
                println!("{}", lib.count(&filter)?);
                return Ok(());
            }
            let entries = listed(&lib, &filter, order, &stacked)?;
            if count {
                println!("{}", entries.len());
                return Ok(());
            }
            for e in &entries {
                if paths {
                    print_path(&e.path)?;
                    continue;
                }
                let summary = e.exif.shot().summary(&e.exif.make, &e.exif.model);
                let mut marks = Vec::new();
                if e.meta.rating > 0 {
                    marks.push(format!("{}*", e.meta.rating));
                }
                if e.meta.flag != greycard_edit::meta::Flag::None {
                    marks.push(e.meta.flag.name().to_lowercase());
                }
                if e.meta.label != greycard_edit::meta::Label::None {
                    marks.push(e.meta.label.name().to_lowercase());
                }
                if !e.meta.keywords.is_empty() {
                    marks.push(format!("[{}]", e.meta.keywords.join(", ")));
                }
                if e.missing {
                    marks.push("missing".into());
                }
                if let Some(n) = stacked.tops.get(&e.path) {
                    marks.push(format!("stack of {n}"));
                }
                println!("{}", e.path.display());
                let line: Vec<String> = [
                    e.exif.taken.clone().unwrap_or_default(),
                    summary.camera,
                    summary.exposure,
                    marks.join(" "),
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
                if !line.is_empty() {
                    println!("              {}", line.join("  "));
                }
            }
            if !paths {
                println!("{:<13} {} of {}", "listed", entries.len(), lib.len()?);
            }
            Ok(())
        }
        LibraryAction::ImportLightroom { .. } => unreachable!("taken before the library opens"),
    }
}

/// The orders `library list --sort` takes: each of the grid's, and
/// each the other way up.
const SORT_KEYS: [&str; 8] = [
    "capture",
    "capture-desc",
    "name",
    "name-desc",
    "rating",
    "rating-desc",
    "edited",
    "edited-desc",
];

/// The files that pass `filter`, in `order`, as the editor's grid lists
/// them: a closed stack's members behind its top (a stack passes the
/// filter when its top does), an open one's gathered behind its top in
/// the stack's own order, the stack standing where its top sorts. As in
/// the window, the stacks are laid over the whole list before the
/// filter is asked (the grid's cells are made, then filtered), so an
/// open stack's members that pass stand at their top's place even when
/// the top does not.
fn listed(
    lib: &greycard_library::Library,
    filter: &greycard_library::Filter,
    order: greycard_library::Order,
    stacked: &greycard_library::stacks::Collapsed,
) -> Result<Vec<greycard_library::Entry>> {
    use greycard_library::filter::{Filter, Term};
    let passing: std::collections::HashSet<PathBuf> = lib
        .query_sorted(filter, order)?
        .into_iter()
        .map(|e| e.path)
        .collect();
    // The whole list under the same rule for missing files as the
    // filter's, so every frame that passes is in it.
    let whole = Filter {
        terms: filter
            .terms
            .iter()
            .filter(|t| matches!(t, Term::Missing(_)))
            .cloned()
            .collect(),
    };
    let mut entries = lib.query_sorted(&whole, order)?;
    entries.retain(|e| !stacked.hidden.contains(&e.path));
    let mut entries =
        greycard_library::stacks::gather(entries, |e| e.path.as_path(), &stacked.open);
    entries.retain(|e| passing.contains(&e.path));
    Ok(entries)
}

/// The stacks kept beside the library at `library` over its rows, by
/// the editor's rule (`stacks::shown`): a member is there when the
/// index has a row at its path that is not missing and, for a version,
/// when that row's sidecar keeps it. With `open`, every stack is taken
/// as open.
fn stacks_over(
    lib: &greycard_library::Library,
    library: &std::path::Path,
    open: bool,
) -> greycard_library::stacks::Collapsed {
    let dir = greycard_library::stacks::dir_in(&greycard_library::collections::dir_beside(library));
    let folder = greycard_library::stacks::read_folder(&dir, greycard_library::stacks::VERSION);
    if folder.list.is_empty() {
        return Default::default();
    }
    // Each member's folder read from the index once: its files there
    // and the versions each sidecar keeps.
    let folders: std::collections::BTreeSet<PathBuf> = folder
        .list
        .iter()
        .flat_map(|s| s.refs.iter())
        .filter_map(|r| std::path::Path::new(&r.hint).parent().map(PathBuf::from))
        .collect();
    let mut kept: std::collections::HashMap<String, Vec<String>> = Default::default();
    for dir in &folders {
        for (path, row) in lib
            .rows_in_canonical(dir, greycard_library::Order::NAME)
            .unwrap_or_default()
        {
            let ids = row.versions.into_iter().map(|v| v.id).collect();
            kept.insert(greycard_library::collections::hint_of(&path), ids);
        }
    }
    let versions = |hint: &str| kept.get(hint).cloned();
    greycard_library::stacks::collapsed(&folder.list, open, &versions)
}

/// A path on its own line, byte for byte on Unix so that a name that
/// is not UTF-8 comes out as the name and not as a replacement
/// character: `--paths` is for feeding to another tool. Windows
/// paths are Unicode and print as they display.
fn print_path(path: &std::path::Path) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        out.write_all(path.as_os_str().as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        out.write_all(path.display().to_string().as_bytes())?;
    }
    out.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `library list` shows a closed stack's top alone, saying how many
    /// it holds, and every frame with `--stacks expanded`; a member the
    /// index has no file for is not counted, and the first member there
    /// is the top.
    #[test]
    fn a_stack_lists_its_top_alone() {
        use greycard_library::collections::{Reference, dir_beside, hint_of};
        use greycard_library::stacks::{Stack, dir_in};
        let d = std::env::temp_dir().join(format!("greycard-cli-stacks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let shoot = d.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        for (i, n) in ["a.jpg", "b.jpg", "c.jpg"].iter().enumerate() {
            std::fs::write(shoot.join(n), vec![i as u8; 64]).unwrap();
        }
        let db = d.join("index").join("library.sqlite");
        let mut lib = greycard_library::Library::open(&db).unwrap();
        lib.index_folder(&shoot, &mut |_| {}).unwrap();
        let key = |n: &str| lib.by_path(&shoot.join(n)).unwrap().unwrap().path;
        // One hash for all: members are told apart by their paths, as
        // two identical copies in one folder are two frames.
        let r = |path: std::path::PathBuf| Reference {
            hash: "h".into(),
            hint: hint_of(&path),
            version: None,
        };
        Stack::new(vec![
            r(shoot.join("gone.jpg")),
            r(key("b.jpg")),
            r(key("a.jpg")),
        ])
        .write(&dir_in(&dir_beside(&db)))
        .unwrap();
        let shown = super::stacks_over(&lib, &db, false);
        assert_eq!(shown.hidden, [key("a.jpg")].into());
        assert_eq!(shown.tops, [(key("b.jpg"), 2)].into());
        let all = super::stacks_over(&lib, &db, true);
        assert!(all.hidden.is_empty());
        assert_eq!(all.tops, shown.tops);
        // A later stack claiming a, as another editor's file might: a is
        // the first's, and the second, left with c alone, is none, as the
        // window has it, so nothing more is hidden.
        let mut later = Stack::new(vec![r(key("a.jpg")), r(key("c.jpg"))]);
        later.made = u64::MAX;
        later.write(&dir_in(&dir_beside(&db))).unwrap();
        assert_eq!(super::stacks_over(&lib, &db, false), shown);
        drop(lib);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// `library list --sort` takes every order the grid has, by the
    /// names the editor's settings keep it by, capture time the
    /// default; the listing is the grid's order, an open stack's
    /// members gathered behind its top in the stack's own order.
    #[test]
    fn the_listing_sorts_as_the_grid_does() {
        use clap::Parser;
        use greycard_library::collections::{Reference, dir_beside, hint_of};
        use greycard_library::fixture::{A7, Frame, R5, write_frame};
        use greycard_library::stacks::{Stack, dir_in};
        use greycard_library::{Order, SortBy};
        for by in SortBy::ALL {
            for descending in [false, true] {
                let o = Order::new(by, descending);
                assert!(super::SORT_KEYS.contains(&o.key().as_str()), "{o:?}");
            }
        }
        for key in super::SORT_KEYS {
            assert!(Order::from_key(key).is_some(), "{key}");
        }
        let sort_of = |args: &[&str]| -> String {
            let cli = crate::Cli::try_parse_from(args).unwrap();
            match cli.command {
                crate::Command::Library {
                    action: super::LibraryAction::List { sort, .. },
                    ..
                } => sort,
                _ => panic!("not a listing"),
            }
        };
        assert_eq!(sort_of(&["greycard", "library", "list"]), "capture");
        assert_eq!(
            sort_of(&["greycard", "library", "list", "--sort", "rating-desc"]),
            "rating-desc"
        );
        assert!(
            crate::Cli::try_parse_from(["greycard", "library", "list", "--sort", "size"]).is_err()
        );

        // Two folders, two bodies, the times interleaved.
        let d = std::env::temp_dir().join(format!("greycard-cli-sort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (a, b) = (d.join("a"), d.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let shots = [
            (a.join("IMG_0001.tif"), "2026:05:01 10:00:01", &R5),
            (a.join("IMG_0002.tif"), "2026:05:01 10:00:03", &R5),
            (b.join("_DSC0001.tif"), "2026:05:01 10:00:00", &A7),
            (b.join("_DSC0002.tif"), "2026:05:01 10:00:02", &A7),
        ];
        for (i, (path, taken, body)) in shots.iter().enumerate() {
            write_frame(path, &Frame { taken, ..**body }, i as u16);
        }
        let db = d.join("index").join("library.sqlite");
        let mut lib = greycard_library::Library::open(&db).unwrap();
        lib.index_tree(&d, &mut |_| {}).unwrap();
        let filtered = |terms: &[&str], order: Order, expanded: bool| -> Vec<String> {
            let stacked = super::stacks_over(&lib, &db, expanded);
            let filter = greycard_library::Filter::from_terms(terms).unwrap();
            super::listed(&lib, &filter, order, &stacked)
                .unwrap()
                .iter()
                .map(|e| e.name())
                .collect::<Vec<_>>()
        };
        let names = |order: Order, expanded: bool| -> Vec<String> {
            let stacked = super::stacks_over(&lib, &db, expanded);
            super::listed(&lib, &greycard_library::Filter::default(), order, &stacked)
                .unwrap()
                .iter()
                .map(|e| e.name())
                .collect()
        };
        let by_time = [
            "_DSC0001.tif",
            "IMG_0001.tif",
            "_DSC0002.tif",
            "IMG_0002.tif",
        ];
        assert_eq!(names(Order::TAKEN, false), by_time);
        assert_eq!(
            names(Order::new(SortBy::Taken, true), false),
            [
                "IMG_0002.tif",
                "_DSC0002.tif",
                "IMG_0001.tif",
                "_DSC0001.tif"
            ]
        );
        assert_eq!(
            names(Order::NAME, false),
            [
                "IMG_0001.tif",
                "IMG_0002.tif",
                "_DSC0001.tif",
                "_DSC0002.tif"
            ]
        );
        // A stack in b, its top _DSC0002: closed, the top alone where
        // it sorts; open, its member behind it.
        let key = |p: &std::path::Path| lib.by_path(p).unwrap().unwrap();
        let r = |p: &std::path::Path| Reference {
            hash: key(p).hash,
            hint: hint_of(&key(p).path),
            version: None,
        };
        Stack::new(vec![r(&b.join("_DSC0002.tif")), r(&b.join("_DSC0001.tif"))])
            .write(&dir_in(&dir_beside(&db)))
            .unwrap();
        assert_eq!(
            names(Order::TAKEN, false),
            ["IMG_0001.tif", "_DSC0002.tif", "IMG_0002.tif"]
        );
        assert_eq!(
            names(Order::TAKEN, true),
            [
                "IMG_0001.tif",
                "_DSC0002.tif",
                "_DSC0001.tif",
                "IMG_0002.tif"
            ]
        );
        // The top filtered out of an open stack: its member stands at
        // the top's place, as the window's grid lays it, not at the end.
        let desc = Order::new(SortBy::Name, true);
        assert_eq!(
            filtered(&["name!=_DSC0002.tif"], desc, true),
            ["_DSC0001.tif", "IMG_0002.tif", "IMG_0001.tif"]
        );
        // Closed, the stack passes with its top or not at all.
        assert_eq!(
            filtered(&["name!=_DSC0002.tif"], desc, false),
            ["IMG_0002.tif", "IMG_0001.tif"]
        );
        drop(lib);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The editor's switches, read from its settings file: on only when
    /// the file says so, off for a file without the key, one that will
    /// not read, or none.
    #[test]
    fn the_editors_switches_are_read_from_its_settings() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-cli-apart-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        assert!(!editor_setting_in(&file, "jpegs_apart"));
        std::fs::write(&file, r#"{"xmp_sidecars": true, "jpegs_apart": true}"#).unwrap();
        assert!(editor_setting_in(&file, "jpegs_apart"));
        assert!(!editor_setting_in(&file, "sidecars_in_folder"));
        std::fs::write(&file, r#"{"jpegs_apart": false}"#).unwrap();
        assert!(!editor_setting_in(&file, "jpegs_apart"));
        std::fs::write(&file, b"not json").unwrap();
        assert!(!editor_setting_in(&file, "jpegs_apart"));
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
