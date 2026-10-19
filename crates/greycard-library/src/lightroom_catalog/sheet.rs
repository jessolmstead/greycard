//! A report as the window's sheet lays it out: a short summary, the
//! one warning that can lose work above it, a line of what a write did,
//! and everything else under Details, each line only where it applies.
//! The command prints the whole report instead ([`Report::lines`]); the
//! two say the same things, the sheet in fewer words.
//!
//! Paths are said through the caller's `path`, so the window can write
//! them as it writes paths everywhere else (`~` for the home folder).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{Found, Report, RootReport, Voice, Wal};
use crate::grouped;

/// What the sheet shows of a report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sheet {
    /// The catalog copied without its write-ahead log: what may be
    /// missing, then what to do about it.
    pub warning: Option<(String, String)>,
    /// What the catalog holds and the import carries, in two or three
    /// short sentences.
    pub summary: String,
    /// After a write, what it did, in one line.
    pub done: Option<String>,
    /// The rest of the report: a word for the line, or none for one
    /// that goes on from the line before, and the text.
    pub details: Vec<(String, String)>,
}

/// `n` and its noun, the thousands grouped: "1,240 frames", "1 frame".
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { one } else { many })
}

/// A count by name, the largest first: "Grain ×3, Masks ×1".
fn counted(map: &BTreeMap<String, usize>) -> String {
    let mut v: Vec<(&String, &usize)> = map.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    v.iter()
        .map(|(k, n)| format!("{k} ×{}", grouped(**n)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The parts with a count, joined: none of them when every count is
/// zero.
fn parts(of: &[(usize, String)]) -> String {
    of.iter()
        .filter(|(n, _)| *n > 0)
        .map(|(_, said)| said.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Default)]
struct Details(Vec<(String, String)>);

impl Details {
    fn line(&mut self, label: &str, text: impl Into<String>) {
        self.0.push((label.to_string(), text.into()));
    }

    fn more(&mut self, text: impl Into<String>) {
        self.0.push((String::new(), text.into()));
    }
}

impl RootReport {
    /// The root by its folder's own name: the last part of the
    /// catalog's path to it.
    pub fn short_name(&self) -> String {
        let path = self.catalog.trim_end_matches(['/', '\\']);
        match path.rsplit(['/', '\\']).next() {
            Some(last) if !last.is_empty() => last.to_string(),
            _ if !self.name.is_empty() => self.name.clone(),
            _ => self.catalog.clone(),
        }
    }

    /// Where the root stands, in a few words: found, with how many of
    /// its files; not found; or not found with the nearest folder of
    /// its name.
    pub fn state(&self, path: &dyn Fn(&Path) -> String) -> String {
        if self.confirmed {
            if self.files == 0 {
                "found, no files in the catalog".to_string()
            } else {
                format!(
                    "found, {} of {} checked",
                    grouped(self.found),
                    grouped(self.files)
                )
            }
        } else if let Some(m) = &self.near_miss {
            format!("not found; nearest: {}", path(&m.dir))
        } else if let Some(local) = &self.local {
            format!("not found in {}", path(local))
        } else {
            "not found".to_string()
        }
    }
}

impl Report {
    /// The report as the sheet shows it.
    pub fn sheet(&self, path: &dyn Fn(&Path) -> String) -> Sheet {
        Sheet {
            warning: (self.wal == Wal::Absent).then(|| {
                (
                    "This catalog has no -wal file beside it, so Lightroom's newest changes \
                     may be missing."
                        .to_string(),
                    "If Lightroom was open when you copied it, quit Lightroom and copy the \
                     catalog again with its -wal."
                        .to_string(),
                )
            }),
            summary: self.summary(),
            done: self.done(),
            details: self.details(path),
        }
    }

    fn summary(&self) -> String {
        let mut said = Vec::new();
        said.push(if self.images == 0 {
            "No frames.".to_string()
        } else {
            let found = match self.found {
                0 => "none found".to_string(),
                n if n == self.images => "all found".to_string(),
                n => format!("{} found", grouped(n)),
            };
            format!("{}, {found}.", count(self.images, "frame", "frames"))
        });
        if self.with_edits > 0 {
            said.push(format!("{} with edits.", grouped(self.with_edits)));
        }
        let collections = self.collections.len() + self.smart.len();
        let held = parts(&[
            (collections, count(collections, "collection", "collections")),
            (self.stacks, count(self.stacks, "stack", "stacks")),
            (
                self.copies,
                count(self.copies, "virtual copy", "virtual copies"),
            ),
        ]);
        if !held.is_empty() {
            said.push(format!("{held}."));
        }
        said.join(" ")
    }

    fn done(&self) -> Option<String> {
        let w = self.written.as_ref()?;
        let mut line = match w.sidecars {
            0 => "Wrote no sidecars".to_string(),
            n => format!("Wrote {}", count(n, "sidecar", "sidecars")),
        };
        if w.written_over > 0 {
            line.push_str(&format!(", {} written over", grouped(w.written_over)));
        }
        if w.left_alone > 0 {
            line.push_str(&format!(
                "; {} with edits here left alone",
                grouped(w.left_alone)
            ));
        }
        if !w.failed.is_empty() {
            line.push_str(&format!("; {} failed", grouped(w.failed.len())));
        }
        if self.missing > 0 {
            line.push_str(&format!(
                "; {} not found",
                count(self.missing, "frame", "frames")
            ));
        }
        line.push('.');
        Some(line)
    }

    fn details(&self, path: &dyn Fn(&Path) -> String) -> Vec<(String, String)> {
        let mut out = Details::default();
        if let Some(w) = &self.written {
            if w.in_folder > 0 {
                out.line(
                    "wrote",
                    format!(
                        "{} under their folder's .greycard/",
                        count(w.in_folder, "sidecar", "sidecars")
                    ),
                );
            }
            for (file, why) in &w.failed {
                out.line("not written", format!("{}: {why}", path(file)));
            }
            let collections = parts(&[
                (w.collections, format!("{} written", grouped(w.collections))),
                (
                    w.joined,
                    format!("{} joined to one of the same name", grouped(w.joined)),
                ),
                (
                    w.collections_there,
                    format!(
                        "{} there already with every frame",
                        grouped(w.collections_there)
                    ),
                ),
                (w.smart, format!("{} smart written", grouped(w.smart))),
                (
                    w.smart_there,
                    format!("{} smart there already", grouped(w.smart_there)),
                ),
            ]);
            if !collections.is_empty() {
                out.line("wrote", format!("collections: {collections}"));
            }
            let not_carried: usize = w.crops_not_carried.values().sum();
            if w.crops + not_carried > 0 {
                let mut crops = format!("crops: {} carried", grouped(w.crops));
                if w.crops_drawn_in > 0 {
                    crops.push_str(&format!(
                        ", {} drawn in to stay on the turned picture",
                        grouped(w.crops_drawn_in)
                    ));
                }
                if not_carried > 0 {
                    crops.push_str(&format!("; not: {}", counted(&w.crops_not_carried)));
                }
                out.line("wrote", crops);
            }
            let unread: usize = w.orientations_unread.values().sum();
            if w.turned + w.flips + unread > 0 {
                let mut turns = format!("turns: {} carried", grouped(w.turned));
                if w.flips > 0 {
                    turns.push_str(&format!(
                        "; {} flipped, the mirror not carried",
                        grouped(w.flips)
                    ));
                }
                if unread > 0 {
                    turns.push_str(&format!(
                        "; orientation not read: {}",
                        counted(&w.orientations_unread)
                    ));
                }
                out.line("wrote", turns);
            }
            if w.time_shifted + w.time_unread > 0 {
                let mut times = format!(
                    "capture times: {} keep the time corrected in Lightroom",
                    count(w.time_shifted, "frame", "frames")
                );
                if w.time_unread > 0 {
                    times.push_str(&format!(
                        "; {} not compared, the file's own time not read",
                        grouped(w.time_unread)
                    ));
                }
                out.line("wrote", times);
            }
            if self.stacks > 0 {
                let mut stacks = parts(&[
                    (w.stacks, format!("{} written", grouped(w.stacks))),
                    (
                        w.stacks_there,
                        format!("{} there already", grouped(w.stacks_there)),
                    ),
                ]);
                if !w.stacks_not_written.is_empty() {
                    if !stacks.is_empty() {
                        stacks.push_str("; ");
                    }
                    stacks.push_str(&format!("not: {}", counted(&w.stacks_not_written)));
                }
                if !stacks.is_empty() {
                    out.line("wrote", format!("stacks: {stacks}"));
                }
            }
            if w.copies_as_own > 0 {
                out.line(
                    "wrote",
                    format!(
                        "{} in collections as their file's own frame, their sidecar \
                         left alone",
                        count(w.copies_as_own, "virtual copy", "virtual copies")
                    ),
                );
            }
            for (dir, said) in &w.roots {
                out.line("root", format!("{}: {}", path(dir), said.said(path)));
            }
        }

        out.line("catalog", path(&self.catalog));
        out.line(
            "written by",
            format!("{}, schema {}", self.written_by, self.schema),
        );
        if self.wal == Wal::Read {
            out.line("log", "read with its -wal");
        }
        out.line(
            "frames",
            if self.copies > 0 {
                format!(
                    "{} and {}",
                    count(self.files, "file", "files"),
                    count(self.copies, "virtual copy", "virtual copies")
                )
            } else {
                count(self.files, "file", "files")
            },
        );
        if self.missing > 0 {
            out.line("not found", count(self.missing, "frame", "frames"));
        }
        for r in &self.roots {
            let files = count(r.files, "file", "files");
            match &r.local {
                Some(local) if r.confirmed => {
                    let how = match &r.how {
                        Found::Given => ", as chosen".to_string(),
                        Found::BesideCatalog => ", beside the catalog".to_string(),
                        Found::UnderRoot(root) => format!(", under {}", path(root)),
                        Found::AsCatalogSays | Found::Not => String::new(),
                    };
                    out.line(
                        "folder",
                        format!(
                            "{} at {}{how}: {} of {files} found",
                            r.catalog,
                            path(local),
                            grouped(r.found)
                        ),
                    );
                }
                Some(local) => out.line(
                    "folder",
                    format!(
                        "{} not at {}: none of {} looked for there; {files}",
                        r.catalog,
                        path(local),
                        grouped(r.checked)
                    ),
                ),
                None => out.line("folder", format!("{} not found; {files}", r.catalog)),
            }
            if let Some(m) = &r.near_miss {
                let mut m = m.clone();
                m.dir = PathBuf::from(path(&m.dir));
                out.more(m.said(Voice::Sheet));
            }
        }
        if self.with_edits > 0 {
            out.line(
                "edits",
                format!(
                    "{}, {} found; each comes across as one named state, its settings \
                     approximated",
                    count(self.with_edits, "frame", "frames"),
                    grouped(self.with_edits_found)
                ),
            );
        }
        if !self.not_carried.is_empty() {
            out.line("not carried", counted(&self.not_carried));
        }
        if self.old_process > 0 {
            out.line(
                "old process",
                format!(
                    "{} on the 2003 or 2010 process, further off still",
                    count(self.old_process, "edit", "edits")
                ),
            );
        }
        if self.unreadable_develop > 0 {
            out.line(
                "unreadable",
                format!(
                    "{} will not read",
                    count(
                        self.unreadable_develop,
                        "frame's develop settings",
                        "frames' develop settings"
                    )
                ),
            );
        }
        if self.crops > 0 {
            out.line(
                "crops",
                format!(
                    "{}, {} found; each carried where its file says its size",
                    grouped(self.crops),
                    grouped(self.crops_found)
                ),
            );
        }
        let mut turns = Vec::new();
        if self.oriented > 0 {
            turns.push(format!(
                "{} carried as Lightroom shows them",
                count(self.oriented, "frame", "frames")
            ));
        }
        if self.flipped > 0 {
            turns.push(format!(
                "{} flipped, {} found; the mirror and any crop not carried",
                grouped(self.flipped),
                grouped(self.flipped_found)
            ));
        }
        if !self.orientations_unread.is_empty() {
            turns.push(format!(
                "orientation not read: {}",
                counted(&self.orientations_unread)
            ));
        }
        if !turns.is_empty() {
            out.line("turns", turns.join("; "));
        }
        if self.with_meta > 0 {
            out.line(
                "meta",
                format!(
                    "{} with a rating, flag, label, keyword, title or caption, {} found",
                    grouped(self.with_meta),
                    grouped(self.with_meta_found)
                ),
            );
        }
        if !self.odd_labels.is_empty() {
            out.line(
                "labels",
                format!("no color for {}; left unlabeled", counted(&self.odd_labels)),
            );
        }
        if self.keywords > 0 {
            out.line(
                "keywords",
                format!(
                    "{} on {}",
                    grouped(self.keywords),
                    count(self.keyworded, "file", "files")
                ),
            );
        }
        if self.copies > 0 {
            let mut copies = format!(
                "{}, {} found, each a version named for it",
                grouped(self.copies),
                grouped(self.copies_found)
            );
            if self.copies_apart > 0 {
                copies.push_str(&format!(
                    "; {} rated or tagged apart from their file, which keeps its own",
                    grouped(self.copies_apart)
                ));
            }
            out.line("copies", copies);
        }
        if self.stacks > 0 {
            out.line(
                "stacks",
                format!(
                    "{}, {} with two frames or more found",
                    grouped(self.stacks),
                    grouped(self.stacks_found)
                ),
            );
        }
        if !self.collections.is_empty() {
            out.line(
                "collections",
                if self.sets > 0 {
                    format!(
                        "{} in {}",
                        grouped(self.collections.len()),
                        count(self.sets, "set", "sets")
                    )
                } else {
                    grouped(self.collections.len())
                },
            );
            for c in &self.collections {
                let creation = c
                    .creation
                    .as_deref()
                    .map(|k| format!(" (a {k}: its frames, not its layout)"))
                    .unwrap_or_default();
                out.more(format!(
                    "{}{creation}: {}, {} found",
                    c.name,
                    count(c.frames, "frame", "frames"),
                    grouped(c.found)
                ));
            }
        }
        if !self.creations_left.is_empty() {
            out.line("left out", self.creations_left.join(", "));
        }
        let smart = parts(&[
            (
                self.smart.len(),
                format!("{} translated", grouped(self.smart.len())),
            ),
            (
                self.smart_skipped.len(),
                format!("{} skipped", grouped(self.smart_skipped.len())),
            ),
            (
                self.smart_stock,
                format!("{} of Lightroom's own left out", grouped(self.smart_stock)),
            ),
        ]);
        if !smart.is_empty() {
            out.line("smart", smart);
            if self.exif_rules + self.exif_rules_skipped > 0 {
                out.more(format!(
                    "on a camera or a lens: {} translated, {} skipped",
                    grouped(self.exif_rules),
                    grouped(self.exif_rules_skipped)
                ));
            }
            for (name, filter) in &self.smart {
                out.more(format!("{name}: {filter}"));
            }
            for (name, why) in &self.smart_skipped {
                out.more(format!("{name}: skipped, {why}"));
            }
        }
        if self.already > 0 {
            out.line(
                "edits here",
                format!(
                    "{}, left alone unless written over",
                    count(self.already, "frame", "frames")
                ),
            );
        }
        for (file, why) in &self.unreadable_sidecars {
            out.line("unreadable", format!("{}: {why}; left alone", path(file)));
        }
        out.line(
            "sidecars",
            if self.sidecars_in_folder {
                "new ones under their folder's .greycard/"
            } else {
                "new ones beside their file, or under the folder's .greycard/ where it has one"
            },
        );
        out.0
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CollectionReport, NearMiss, Rooted, Written};
    use super::*;

    fn shown(p: &Path) -> String {
        p.display().to_string()
    }

    fn root(catalog: &str) -> RootReport {
        RootReport {
            catalog: catalog.into(),
            name: "Shoots".into(),
            local: None,
            how: Found::Not,
            checked: 0,
            checked_found: 0,
            confirmed: false,
            files: 0,
            found: 0,
            near_miss: None,
        }
    }

    /// A catalog of 1,240 frames, 42 of them not found.
    fn report() -> Report {
        Report {
            catalog: "/x/Shoot.lrcat".into(),
            schema: 1_504_001,
            written_by: "Lightroom Classic 15.4 and 15.5".into(),
            roots: vec![RootReport {
                local: Some("/here/Shoots".into()),
                how: Found::BesideCatalog,
                checked: 5,
                checked_found: 5,
                confirmed: true,
                files: 1233,
                found: 1191,
                ..root("D:/Away/Shoots/")
            }],
            images: 1240,
            files: 1233,
            copies: 7,
            found: 1198,
            missing: 42,
            with_edits: 312,
            with_edits_found: 300,
            stacks: 3,
            collections: (0..12)
                .map(|i| CollectionReport {
                    name: format!("Set/C{i}"),
                    frames: 2,
                    found: 2,
                    creation: None,
                })
                .collect(),
            smart: vec![("Five".into(), "rating >= 5".into())],
            smart_skipped: vec![("Odd".into(), "no term for it".into())],
            ..Report::default()
        }
    }

    #[test]
    fn the_summary_is_three_short_sentences_with_the_thousands_grouped() {
        let r = report();
        assert_eq!(
            r.sheet(&shown).summary,
            "1,240 frames, 1,198 found. 312 with edits. 13 collections, 3 stacks, 7 virtual copies."
        );
        // Nothing with edits and nothing held: one sentence.
        let bare = Report {
            images: 2,
            found: 2,
            ..Report::default()
        };
        assert_eq!(bare.sheet(&shown).summary, "2 frames, all found.");
        let one = Report {
            images: 1,
            with_edits: 1,
            copies: 1,
            ..Report::default()
        };
        assert_eq!(
            one.sheet(&shown).summary,
            "1 frame, none found. 1 with edits. 1 virtual copy."
        );
    }

    #[test]
    fn details_hold_the_rest_and_only_what_applies() {
        let r = report();
        let sheet = r.sheet(&shown);
        assert!(sheet.warning.is_none());
        assert!(sheet.done.is_none());
        let has = |label: &str, text: &str| {
            sheet
                .details
                .iter()
                .any(|(l, t)| l == label && t.contains(text))
        };
        assert!(has("catalog", "/x/Shoot.lrcat"));
        assert!(has(
            "written by",
            "Lightroom Classic 15.4 and 15.5, schema 1504001"
        ));
        assert!(has("not found", "42 frames"));
        assert!(has(
            "folder",
            "D:/Away/Shoots/ at /here/Shoots, beside the catalog: 1,191 of 1,233 files found"
        ));
        assert!(has("edits", "312 frames, 300 found"));
        assert!(has("sidecars", "beside their file"));
        assert!(has("", "Set/C0: 2 frames, 2 found"));
        assert!(has("smart", "1 translated, 1 skipped"));
        assert!(has("", "Odd: skipped, no term for it"));
        // Nothing of what the catalog does not have.
        let labels: Vec<&str> = sheet.details.iter().map(|(l, _)| l.as_str()).collect();
        for absent in [
            "log",
            "not carried",
            "old process",
            "unreadable",
            "crops",
            "turns",
            "meta",
            "labels",
            "keywords",
            "left out",
            "edits here",
            "wrote",
        ] {
            assert!(!labels.contains(&absent), "{absent}: {labels:?}");
        }
        assert!(!has("smart", "Lightroom's own"), "a zero count is not said");

        // What the command says only where it applies is said here too.
        let mut more = report();
        more.not_carried.insert("Grain".into(), 4);
        more.old_process = 2;
        more.already = 1;
        more.oriented = 4;
        more.flipped = 1;
        let sheet = more.sheet(&shown);
        let has = |label: &str, text: &str| {
            sheet
                .details
                .iter()
                .any(|(l, t)| l == label && t.contains(text))
        };
        assert!(has("not carried", "Grain ×4"));
        assert!(has("old process", "2 edits on the 2003 or 2010 process"));
        assert!(has("edits here", "1 frame, left alone unless written over"));
        assert!(has(
            "turns",
            "4 frames carried as Lightroom shows them; 1 flipped, 0 found"
        ));
    }

    #[test]
    fn a_catalog_without_its_log_warns_in_one_sentence_and_one_to_do() {
        let r = Report {
            wal: Wal::Absent,
            ..report()
        };
        let (warning, remedy) = r.sheet(&shown).warning.unwrap();
        assert_eq!(warning.matches(". ").count(), 0, "{warning}");
        assert!(warning.ends_with('.'));
        assert!(remedy.contains("-wal"), "{remedy}");
        let read = Report {
            wal: Wal::Read,
            ..report()
        };
        assert!(read.sheet(&shown).warning.is_none());
        assert!(
            read.sheet(&shown)
                .details
                .contains(&("log".into(), "read with its -wal".into()))
        );
    }

    #[test]
    fn a_write_is_said_in_one_line_and_its_counts_go_under_details() {
        let mut r = report();
        r.written = Some(Written {
            sidecars: 1198,
            collections: 12,
            smart: 1,
            stacks: 3,
            roots: vec![
                ("/here/Shoots".into(), Rooted::Added),
                ("/here/More".into(), Rooted::Under("/here".into())),
            ],
            ..Written::default()
        });
        let sheet = r.sheet(&shown);
        assert_eq!(
            sheet.done.as_deref(),
            Some("Wrote 1,198 sidecars; 42 frames not found.")
        );
        assert!(sheet.details.contains(&(
            "wrote".into(),
            "collections: 12 written, 1 smart written".into()
        )));
        assert!(
            sheet
                .details
                .contains(&("wrote".into(), "stacks: 3 written".into()))
        );
        assert!(
            sheet
                .details
                .contains(&("root".into(), "/here/Shoots: added".into()))
        );
        // The root it is under said as the window says paths.
        let tagged = r.sheet(&|p| format!("<{}>", p.display()));
        assert!(
            tagged
                .details
                .contains(&("root".into(), "</here/More>: under </here>".into())),
            "{:?}",
            tagged.details
        );
        // Forced over frames with edits here.
        let w = r.written.as_mut().unwrap();
        w.written_over = 30;
        assert_eq!(
            r.sheet(&shown).done.as_deref(),
            Some("Wrote 1,198 sidecars, 30 written over; 42 frames not found.")
        );
    }

    #[test]
    fn a_root_says_where_it_stands_in_a_few_words() {
        let found = RootReport {
            local: Some("/here/Shoots".into()),
            confirmed: true,
            files: 400,
            found: 5,
            ..root("D:/Away/Shoots/")
        };
        assert_eq!(found.short_name(), "Shoots");
        assert_eq!(found.state(&shown), "found, 5 of 400 checked");
        let empty = RootReport {
            files: 0,
            found: 0,
            ..found.clone()
        };
        assert_eq!(empty.state(&shown), "found, no files in the catalog");
        let lost = root("C:\\Photos\\2024");
        assert_eq!(lost.short_name(), "2024");
        assert_eq!(lost.state(&shown), "not found");
        let near = RootReport {
            near_miss: Some(NearMiss {
                dir: "/archive/2025-a/Shoots".into(),
                there: 1,
                looked_for: 3,
                other_sizes: false,
            }),
            ..root("D:/Away/Shoots/")
        };
        assert_eq!(
            near.state(&shown),
            "not found; nearest: /archive/2025-a/Shoots"
        );
        let mut report = report();
        report.roots = vec![near];
        assert!(report.sheet(&shown).details.contains(&(
            String::new(),
            "/archive/2025-a/Shoots holds 1 of 3 looked for; choose it to take it".into()
        )));
    }
}
