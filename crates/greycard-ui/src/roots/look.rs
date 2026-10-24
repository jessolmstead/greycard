//! The read off the window's thread, and sending it there.

use super::*;

impl Look {
    /// The read, on a thread that is not the window's: the roots and
    /// the folders not known yet looked at, the folder listed, the
    /// list's rows asked of the index, the new files held (from their
    /// rows where the index has them, else their sidecars read on the
    /// pool), and the frame on screen looked for.
    pub(crate) fn run(self) -> Found {
        let started = Instant::now();
        let offline = (!self.roots.is_empty()).then(|| offline_of(&self.roots, ROOT_WAIT));
        let away: Vec<PathBuf> = offline.iter().flatten().cloned().collect();
        let mut readable = Vec::new();
        let mut unreadable = Vec::new();
        let mut canonical: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut looked = 0;
        let mut said = None;
        let mut rows: Option<HashMap<PathBuf, RowMeta>> = None;
        let lib = self.index.as_deref().and_then(|p| {
            let mut lib = greycard_library::Library::open_read_only(p).ok()?;
            lib.set_jpegs_apart(self.apart);
            Some(lib)
        });
        let mut companions = Vec::new();
        // A collection's references found in the index, here off the
        // window's thread, and listed from there as a view of the roots
        // is: each picture once already, so nothing is hidden.
        let mut collection = None;
        let mut frames = None;
        let mut queue = None;
        let source = match self.source {
            // The queue's files, once each in the order first met, as
            // files given; its frames, a file twice when it is queued
            // twice, for the cells.
            Source::Queue {
                frames: keys,
                listed,
            } => {
                let mut seen: HashSet<&PathBuf> = HashSet::new();
                let files: Vec<PathBuf> = keys
                    .iter()
                    .filter(|k| seen.insert(&k.path))
                    .map(|k| k.path.clone())
                    .collect();
                frames = Some(keys);
                queue = Some(listed);
                Source::Files(files)
            }
            Source::Collection {
                id,
                refs,
                archives,
                shown,
            } => {
                let shown = match shown {
                    Some(shown) => Some(shown),
                    None => match lib.as_ref().map(|l| {
                        crate::collections::resolve(l, &refs, &away, &archives, self.order)
                    }) {
                        Some(Ok(shown)) => Some(shown),
                        Some(Err(e)) => {
                            tracing::warn!("library: {e}");
                            said = Some(format!("the library index could not be read: {e}"));
                            None
                        }
                        None => {
                            said = Some("the library index could not be read".to_string());
                            None
                        }
                    },
                };
                let listed = shown.map(|s| {
                    // What Forget missing may drop: the hints of those
                    // found nowhere, and the files of the versions gone.
                    let gone = s
                        .gone
                        .into_iter()
                        .map(|r| r.hint)
                        .chain(s.lost.iter().map(|(_, at)| crate::collections::hint_of(at)))
                        .collect();
                    collection = Some((id, s.missing, gone, s.changes));
                    frames = Some(
                        s.frames
                            .into_iter()
                            .map(|(path, v)| crate::versions::Key::of(path, v.as_deref()))
                            .collect(),
                    );
                    s.listed
                });
                Source::Roots {
                    roots: Vec::new(),
                    only: false,
                    rows: Some(listed.unwrap_or_default()),
                    hide: Vec::new(),
                }
            }
            source => source,
        };
        let from_disk_list = matches!(source, Source::Folder(_) | Source::Files(_));
        let files = match source {
            // Listed as rows or files above; nothing comes this way.
            Source::Collection { .. } | Source::Queue { .. } => None,
            Source::Folder(dir) => {
                // Looked at first, with the roots' wait: a folder on a
                // share that has gone away does not answer at all, and
                // nothing after this would come back. Said, and the
                // list left as it is.
                match answer_of(&dir, ROOT_WAIT) {
                    None => {
                        // Not "in 3 s": a look still out from before
                        // answers no at once.
                        said = Some(format!("{} isn't reachable", dir.display()));
                        None
                    }
                    Some(_) => {
                        let key = greycard_library::key_folder(&dir);
                        canonical.push((dir.clone(), key));
                        list_folder(&dir, self.apart, &mut said).map(|listed| {
                            companions = listed.companions;
                            listed.frames
                        })
                    }
                }
            }
            Source::Files(files) => {
                let mut dirs: HashSet<&Path> = HashSet::new();
                for f in &files {
                    if let Some(dir) = f.parent()
                        && dirs.insert(dir)
                    {
                        canonical.push((dir.to_path_buf(), greycard_library::key_folder(dir)));
                    }
                }
                if !self.apart {
                    companions = greycard_library::pairs::companions_of(&files);
                }
                Some(files)
            }
            Source::Roots {
                roots,
                only,
                rows: listed,
                hide,
            } => {
                let order = self.order.unwrap_or(greycard_library::Order::NAME);
                let listed = match listed {
                    Some(listed) => listed,
                    None => match lib.as_ref().map(|l| rows_listed(l, &roots, only, order)) {
                        Some(Ok(listed)) => listed,
                        Some(Err(e)) => {
                            tracing::warn!("library: {e}");
                            said = Some(format!("the library index could not be read: {e}"));
                            Vec::new()
                        }
                        None => {
                            said = Some("the library index could not be read".to_string());
                            Vec::new()
                        }
                    },
                };
                let mut seen: HashMap<PathBuf, bool> = HashMap::new();
                let mut kept = Vec::with_capacity(listed.len());
                let mut map = HashMap::with_capacity(listed.len());
                let is_away = |f: &Path| away.iter().any(|r| f.starts_with(r));
                // A folder under a root that is offline is not looked
                // at (nothing there would answer), and its frames are
                // kept, to be shown dimmed.
                let mut can_read = |f: &Path| {
                    let Some(dir) = f.parent() else {
                        return false;
                    };
                    is_away(f)
                        || self.known.contains(dir)
                        || match seen.get(dir) {
                            Some(&can) => can,
                            None => {
                                looked += 1;
                                let can = std::fs::read_dir(dir).is_ok();
                                seen.insert(dir.to_path_buf(), can);
                                if can {
                                    readable.push(dir.to_path_buf());
                                } else {
                                    unreadable.push(dir.to_path_buf());
                                }
                                can
                            }
                        }
                };
                // The local rows first, then the archives' with the
                // copies of local frames that are there left out, so a
                // hidden copy's folder is never looked at, and a frame
                // whose local copy is offline or gone is listed from
                // the archive, where it can be opened. The order is
                // the index's.
                let order: HashMap<PathBuf, usize> = listed
                    .iter()
                    .enumerate()
                    .map(|(i, (f, _))| (f.clone(), i))
                    .collect();
                let on_archive = |f: &Path| hide.iter().any(|a| f.starts_with(a));
                let (archived, local): (Vec<_>, Vec<_>) =
                    listed.into_iter().partition(|(f, _)| on_archive(f));
                for (f, row) in local {
                    if can_read(&f) {
                        kept.push(f.clone());
                        map.insert(f, row);
                    }
                }
                let there: HashSet<PathBuf> =
                    kept.iter().filter(|f| !is_away(f)).cloned().collect();
                let mut both: Vec<(PathBuf, RowMeta)> =
                    Vec::with_capacity(there.len() + archived.len());
                both.extend(there.iter().map(|f| (f.clone(), map[f].clone())));
                both.extend(archived);
                let shown = crate::archive::hide_local_copies(
                    both,
                    &hide,
                    |r| r.hash.as_str(),
                    |f| there.contains(f),
                );
                let mut from_archive: HashSet<String> = HashSet::new();
                for (f, row) in shown {
                    if on_archive(&f) && can_read(&f) {
                        from_archive.insert(row.hash.clone());
                        kept.push(f.clone());
                        map.insert(f, row);
                    }
                }
                // A local copy offline whose frame is listed from the
                // archive would be the frame twice, once dimmed: the
                // archive's is the one, and the stand-in goes.
                kept.retain(|f| {
                    let twice =
                        is_away(f) && map.get(f).is_some_and(|r| from_archive.contains(&r.hash));
                    if twice {
                        map.remove(f);
                    }
                    !twice
                });
                kept.sort_by_key(|f| order.get(f).copied().unwrap_or(usize::MAX));
                // The index's own paths: their folders are canonical.
                let mut folders: HashSet<&Path> = HashSet::new();
                for f in &kept {
                    if let Some(dir) = f.parent()
                        && folders.insert(dir)
                    {
                        canonical.push((dir.to_path_buf(), dir.to_path_buf()));
                    }
                }
                rows = Some(map);
                // The companions from the index's rows of the same
                // folders: a folder offline still has them, and none
                // is read for them.
                if let Some(lib) = &lib {
                    let listed: Vec<PathBuf> = folders.iter().map(|d| d.to_path_buf()).collect();
                    match lib.companions_in(&listed) {
                        Ok(found) => companions = found,
                        Err(e) => tracing::debug!("library: {e}; no companions marked"),
                    }
                }
                if said.is_some() { None } else { Some(kept) }
            }
        };
        let known: HashMap<&Path, &Path> = canonical
            .iter()
            .map(|(d, c)| (d.as_path(), c.as_path()))
            .collect();
        // A folder's or a given list's rows, where the index has them:
        // those frames stand in from their rows and read no sidecar.
        if rows.is_none()
            && let (Some(lib), Some(files)) = (&lib, &files)
        {
            let asked = lib.rows_of_with(files, &mut |dir| {
                known
                    .get(dir)
                    .map(|c| c.to_path_buf())
                    .unwrap_or_else(|| greycard_library::key_folder(dir))
            });
            match asked {
                Ok(found) => {
                    rows = Some(
                        files
                            .iter()
                            .cloned()
                            .zip(found)
                            .filter_map(|(f, r)| r.map(|r| (f, r)))
                            .collect(),
                    );
                }
                Err(e) => tracing::debug!("library: {e}; the sidecars read instead"),
            }
        }
        // A folder from the disk, or files given, in the view's order by
        // the same rule the index's query lists by, from the rows the
        // index has; a file it has no row for yet sorts as one with no
        // value, by name. A view of the index came in its order already.
        let mut files = files;
        if let (true, Some(order), Some(files)) = (from_disk_list, self.order, files.as_mut()) {
            sort_files(files, order, rows.as_ref());
        }
        let fresh: Vec<PathBuf> = files
            .iter()
            .flatten()
            .filter(|f| !self.have.contains(*f))
            .cloned()
            .collect();
        let fresh_rows: Vec<Option<RowMeta>> = fresh
            .iter()
            .map(|f| rows.as_ref().and_then(|m| m.get(f).cloned()))
            .collect();
        let from_disk = if self.write {
            rows::to_read(&fresh_rows)
        } else {
            0
        };
        if let Some(p) = &self.progress {
            p.total.store(from_disk, Ordering::Relaxed);
        }
        let held = rows::bring(
            &fresh,
            &fresh_rows,
            self.write,
            self.progress.as_deref().map(|p| &p.read),
        );
        // The frame to open: the file last open matched to the list by
        // its folder's canonical form, which is in hand here.
        let select = match (&self.purpose, &files) {
            (Purpose::Open { select, .. }, Some(files)) if !files.is_empty() => match select {
                Select::Row(i) => (*i).min(files.len() - 1),
                Select::Last(None) => 0,
                Select::Last(Some(last)) => {
                    let want = greycard_library::key_path(last);
                    files
                        .iter()
                        .position(|f| match (f.parent(), f.file_name()) {
                            (Some(dir), Some(name)) => {
                                known.get(dir).is_some_and(|c| c.join(name) == want)
                            }
                            _ => false,
                        })
                        .unwrap_or(0)
                }
            },
            _ => 0,
        };
        // The frame on screen under a root this read found offline is not
        // looked at: a stat under a share that does not answer does not
        // come back. It is taken as there, and stays listed.
        let on_screen = self
            .on_screen
            .filter(|(path, _)| !away.iter().any(|r| path.starts_with(r)))
            .map(|(path, id)| OnScreen::look(path, id, lib.as_ref()))
            .unwrap_or_default();
        let seconds = started.elapsed().as_secs_f64();
        tracing::debug!(
            "library: the list read off the window's thread in {:.1} ms: {} files, {} new, \
             {} sidecar(s) read from disk, {} folder(s) and {} root(s) looked at",
            seconds * 1e3,
            files.as_ref().map_or(0, Vec::len),
            fresh.len(),
            from_disk,
            looked,
            self.roots.len()
        );
        Found {
            purpose: self.purpose,
            generation: self.generation,
            token: self.token,
            epoch: self.epoch,
            files,
            said,
            fresh,
            held,
            from_disk,
            rows,
            select,
            offline,
            readable,
            unreadable,
            canonical,
            on_screen,
            seconds,
            collection,
            frames,
            queue,
            companions,
            order: self.order,
        }
    }
}

/// `files` put in `order` by [`greycard_library::Order::compare`], each
/// by its row where `rows` has one: the rule a view of the index is
/// listed by, so a folder read from the disk and the same folder as a
/// root's reads in one order.
pub(crate) fn sort_files(
    files: &mut [PathBuf],
    order: greycard_library::Order,
    rows: Option<&HashMap<PathBuf, RowMeta>>,
) {
    fn key<'a>(
        f: &'a PathBuf,
        rows: Option<&'a HashMap<PathBuf, RowMeta>>,
    ) -> greycard_library::SortKey<'a> {
        match rows.and_then(|r| r.get(f)) {
            Some(row) => row.sort_key(f),
            None => greycard_library::SortKey::bare(f),
        }
    }
    files.sort_by(|a, b| order.compare(&key(a, rows), &key(b, rows)));
}

/// A folder's pictures, with the camera's JPEGs beside its raws found
/// unless `apart`, or none with the reason in `said`: nothing in it,
/// or not a folder.
fn list_folder(
    dir: &Path,
    apart: bool,
    said: &mut Option<String>,
) -> Option<greycard_library::pairs::Paired> {
    match files::list(dir, apart) {
        Ok(listed) if listed.frames.is_empty() => {
            *said = Some(format!("no pictures in {}", dir.display()));
            None
        }
        Ok(listed) => Some(listed),
        Err(e) => {
            tracing::warn!("{e:#}");
            *said = Some(format!("{}: not a folder", dir.display()));
            None
        }
    }
}

/// Send a read off the window's thread; what it finds is landed on the
/// window's thread when it is done. False when it could not be sent.
#[cfg(not(test))]
pub(super) fn send_off(app: &App, look: Look) -> bool {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard roots read".into())
        .spawn(move || {
            let found = look.run();
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                land(&state, &app, &worker, found);
            });
        });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("library: {e}");
            false
        }
    }
}

/// In a test a read is queued, and the test runs and lands it
/// (`tests::land_all`): there is no event loop to land it on.
#[cfg(test)]
pub(super) fn send_off(_app: &App, look: Look) -> bool {
    tests::SENT.with(|s| s.borrow_mut().push(look));
    true
}
