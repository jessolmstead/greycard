//! Preset files brought in: a Lightroom `.xmp` or `.lrtemplate`, or
//! one of ours, read and written into a preset store.

use std::path::{Path, PathBuf};

use greycard_edit::preset::{EXTENSION, Preset, Store, group_folder, plain, slug, stem_name};
use greycard_edit::{Error, Result};

use crate::lightroom::{self, Imported};

/// The extension of a Lightroom preset from before 7.3.
pub const LRTEMPLATE: &str = "lrtemplate";

/// Whether `path`'s extension is `ext`, case aside.
fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| x.eq_ignore_ascii_case(ext))
}

/// The most of one of our presets read, `.gcp` or a file of another
/// name taken as one: its JSON holds no image and no look table (a
/// look is named, its file found on the machine), the most of it a
/// mask's brush strokes, some tens of bytes a point, so this is some
/// two million points, past any preset painted by hand.
pub const GCP_MOST: usize = 64 << 20;

/// The bytes of `path`, read no further than a byte past `most`; none
/// when there are more, whatever its size says (a pipe, a device, a
/// file still growing), and nothing past it held.
pub(crate) fn bytes_at_most(path: &Path, most: usize) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(most as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok((bytes.len() <= most).then_some(bytes))
}

/// The text of `path`, read as [`bytes_at_most`] reads it; one longer
/// than `most` is refused.
fn read_at_most(path: &Path, most: usize) -> Result<String> {
    let Some(bytes) = bytes_at_most(path, most)? else {
        return Err(lightroom::too_large(most));
    };
    String::from_utf8(bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        )
        .into()
    })
}

/// Read a preset file of any kind: Lightroom's `.xmp` or older
/// `.lrtemplate`, or this engine's; the name from inside, or the
/// file's stem.
pub fn import(path: &Path) -> Result<Imported> {
    if has_extension(path, LRTEMPLATE) {
        let text = read_at_most(path, lightroom::TEMPLATE_MOST)?;
        return lightroom::read_template(&text, &stem_name(path));
    }
    if has_extension(path, "xmp") {
        let text = read_at_most(path, lightroom::XMP_MOST)?;
        return lightroom::read(&text, &stem_name(path));
    }
    let text = read_at_most(path, GCP_MOST)?;
    let mut preset = Preset::from_json(&text)?;
    if preset.name.is_empty() {
        preset.name = stem_name(path);
    }
    Ok(Imported {
        preset,
        unmapped: Vec::new(),
        offset: 0.0,
        group: None,
    })
}

/// Why a preset file did not come into the store.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The file would not read.
    #[error("{0}")]
    Unreadable(std::io::Error),
    /// It read, and is not a preset: not XMP or a `.lrtemplate` with
    /// Camera Raw settings in it, not a preset's JSON, or one that
    /// carries nothing this engine has. Why, in words.
    #[error("not a preset: {0}")]
    NotAPreset(String),
    /// A preset of ours from a build newer than this one: its schema.
    #[error("from a newer greycard (schema {0})")]
    Newer(u32),
    /// A preset whose name is blank, which no row could show.
    #[error("a preset needs a name")]
    Nameless,
    /// The store has a preset of this name, in any case: the stored
    /// one's name, as the list shows it.
    #[error("there is a preset called \"{0}\" already")]
    Taken(String),
    /// It read, and could not be written into the store.
    #[error("saving {0}: {why}", why = plain(.1))]
    Unsaved(String, Error),
}

/// Bring a preset file into the store: Lightroom's `.xmp` or
/// `.lrtemplate`, or this engine's `.gcp`, read as [`import`] reads it, with what did not
/// come across. The editor's Import and the command line's
/// `--import` both come here.
///
/// A name the store has already is refused rather than written
/// over, where a save from the editor writes over it: a file
/// brought in is not necessarily a newer copy of the one there, and
/// the one there may be the user's own. A blank name is refused, as
/// a save refuses one. With `replace` the caller says it is, and the
/// one there is written over in its own file; the window refuses,
/// having a Remove, and the command line replaces on `--replace`.
/// A new name's file is the first free one, so a different name
/// with the same slug ("Warm, Film" beside "Warm Film") takes no
/// other preset's file either.
pub fn import_into(
    store: &Store,
    path: &Path,
    replace: bool,
) -> std::result::Result<(PathBuf, crate::lightroom::Imported), ImportError> {
    import_into_group(store, path, replace, None)
}

/// [`import_into`], a new preset going into a group: the one its
/// Lightroom file names (`crs:Group`), else `folder`, the group
/// [`gather`] found for the file, else none. A preset replaced stays
/// in its own file, wherever that is.
pub fn import_into_group(
    store: &Store,
    path: &Path,
    replace: bool,
    folder: Option<&str>,
) -> std::result::Result<(PathBuf, crate::lightroom::Imported), ImportError> {
    let imported = import(path).map_err(|e| match e {
        Error::Io(e) => ImportError::Unreadable(e),
        Error::Newer(v) => ImportError::Newer(v),
        Error::Json(e) => ImportError::NotAPreset(e.to_string()),
        Error::Preset(why) | Error::Xmp(why) | Error::Pp3(why) => ImportError::NotAPreset(why),
        // A sidecar's, never an import's.
        e @ (Error::Unread(_) | Error::ReadOnly(_)) => ImportError::NotAPreset(e.to_string()),
    })?;
    let preset = &imported.preset;
    if preset.is_empty() {
        let why = match imported.unmapped.as_slice() {
            [] => "it carries nothing".to_string(),
            passed => format!("nothing in it has a place here ({})", passed.join(", ")),
        };
        return Err(ImportError::NotAPreset(why));
    }
    let name = preset.name.trim();
    if name.is_empty() {
        return Err(ImportError::Nameless);
    }
    let there = store
        .list()
        .into_iter()
        .find(|e| e.preset.name.trim().eq_ignore_ascii_case(name));
    let written = match there {
        Some(entry) if replace => entry.path,
        Some(entry) => return Err(ImportError::Taken(entry.preset.name)),
        None => {
            let group = imported
                .group
                .as_deref()
                .or(folder)
                .and_then(group_folder)
                .map(|g| {
                    // A group there already in another case is that one.
                    let wanted = g.to_lowercase();
                    store
                        .groups()
                        .into_iter()
                        .find(|h| h.to_lowercase() == wanted)
                        .unwrap_or(g)
                });
            // A group no folder can be made for (a device name on
            // Windows, CON or NUL) is no group: the preset comes in
            // outside any rather than not at all.
            let mut dir = store.group_dir(group.as_deref());
            if let Err(e) = std::fs::create_dir_all(&dir) {
                if group.is_none() {
                    return Err(ImportError::Unsaved(name.to_string(), e.into()));
                }
                log::warn!(
                    "preset group {}: {e}; {name} put in no group",
                    dir.display()
                );
                dir = store.group_dir(None);
                std::fs::create_dir_all(&dir)
                    .map_err(|e| ImportError::Unsaved(name.to_string(), e.into()))?;
            }
            let slug = slug(name);
            std::iter::once(format!("{slug}.{EXTENSION}"))
                .chain((2..).map(|n| format!("{slug}-{n}.{EXTENSION}")))
                .map(|file| dir.join(file))
                .find(|p| !p.exists())
                .expect("a free file name")
        }
    };
    preset
        .save_to(&written)
        .map_err(|e| ImportError::Unsaved(name.to_string(), e))?;
    Ok((written, imported))
}

/// Whether a path names a preset file of any kind, by its
/// extension, case aside.
fn is_preset_file(path: &Path) -> bool {
    path.is_file()
        && ["xmp", LRTEMPLATE, EXTENSION]
            .iter()
            .any(|x| has_extension(path, x))
}

/// The files an import of `paths` brings in, each with the group its
/// folder gives it; a preset's own `crs:Group` comes before that
/// ([`import_into_group`]).
///
/// A folder named is a Lightroom preset folder: its preset files go
/// into a group of the folder's name, and each folder in it, as
/// Lightroom's own `Settings` folder holds one a group, into a group
/// of that folder's name; no deeper. A file named is itself, in no
/// group: files picked from a downloads folder are no pack. In the
/// order named, a folder's files by name.
pub fn gather(paths: &[PathBuf]) -> Vec<(PathBuf, Option<String>)> {
    let name_of = |dir: &Path| {
        dir.file_name()
            .and_then(|n| n.to_str())
            .and_then(group_folder)
    };
    let files_in = |dir: &Path| {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        files
    };
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            let group = name_of(path);
            let listed = files_in(path);
            out.extend(
                listed
                    .iter()
                    .filter(|p| is_preset_file(p))
                    .map(|p| (p.clone(), group.clone())),
            );
            for sub in listed.iter().filter(|p| p.is_dir()) {
                if sub
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
                {
                    continue;
                }
                let group = name_of(sub);
                out.extend(
                    files_in(sub)
                        .into_iter()
                        .filter(|p| is_preset_file(p))
                        .map(|p| (p, group.clone())),
                );
            }
            continue;
        }
        out.push((path.clone(), None));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::{Section, VERSION};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("greycard-preset-{tag}-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&d);
        d
    }

    #[test]
    fn import_reads_either_kind() {
        let d = dir("import");
        std::fs::create_dir_all(&d).unwrap();
        let xmp = d.join("Faded.XMP");
        std::fs::write(&xmp, r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+1.0" crs:Dehaze="+20"/></rdf:RDF></x:xmpmeta>"#).unwrap();
        let got = import(&xmp).unwrap();
        assert_eq!(got.preset.name, "Faded");
        assert_eq!(got.preset.edit.light.exposure, 1.0);
        assert!((got.preset.edit.detail.dehaze - 0.2).abs() < 1e-6);
        assert_eq!(got.preset.sections, vec![Section::Light, Section::Detail]);
        assert!(got.unmapped.is_empty());
        let gcp = d.join("Mine.gcp");
        std::fs::write(
            &gcp,
            r#"{"sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        let got = import(&gcp).unwrap();
        assert_eq!(got.preset.name, "Mine");
        assert_eq!(got.preset.edit.grain.amount, 0.2);
        assert!(got.unmapped.is_empty());
        assert!(import(&d.join("none.gcp")).is_err());
        greycard_core::testing::remove_dir_retry(&d);
    }

    /// The store's import: a preset in, under its slug, with what did
    /// not come across; a name the store has refused in any case, the
    /// one there untouched; a file that is no preset, or carries
    /// nothing, refused; and a name whose slug another preset's file
    /// has takes the next file, not that one.
    #[test]
    fn the_store_imports_and_refuses_a_name_it_has() {
        let d = dir("store-import");
        std::fs::create_dir_all(&d).unwrap();
        let store = Store::at(d.join("store"));
        let xmp = d.join("faded.xmp");
        std::fs::write(&xmp, r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+1.0" crs:RedHue="+10"/></rdf:RDF></x:xmpmeta>"#).unwrap();
        let (written, got) = import_into(&store, &xmp, false).unwrap();
        assert_eq!(written, store.dir().join("faded.gcp"));
        assert_eq!(got.preset.name, "faded");
        assert_eq!(got.unmapped, vec!["RedHue"]);
        assert_eq!(store.list().len(), 1);

        // Again, and as another case: refused, and the file the same.
        let before = std::fs::read_to_string(&written).unwrap();
        let other = d.join("FADED.gcp");
        std::fs::write(
            &other,
            r#"{"sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        for path in [&xmp, &other] {
            match import_into(&store, path, false) {
                // The stored one's name, not the incoming "FADED".
                Err(ImportError::Taken(name)) => assert_eq!(name, "faded"),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(std::fs::read_to_string(&written).unwrap(), before);

        // Asked to replace: the other case's file goes over the one
        // there, in that one's own file, and the store has one still.
        let (replaced, got) = import_into(&store, &other, true).unwrap();
        assert_eq!(replaced, written);
        assert_eq!(got.preset.name, "FADED");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].preset.edit.grain.amount, 0.2);
        // And back, for what follows.
        import_into(&store, &xmp, true).unwrap();
        assert_eq!(std::fs::read_to_string(&written).unwrap(), before);

        // Not a preset, nothing in it, not there.
        let junk = d.join("junk.xmp");
        std::fs::write(&junk, "not xml at all").unwrap();
        assert!(matches!(
            import_into(&store, &junk, false),
            Err(ImportError::NotAPreset(_))
        ));
        let masks = d.join("masks.xmp");
        std::fs::write(&masks, r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:RedHue="+10"/></rdf:RDF></x:xmpmeta>"#).unwrap();
        match import_into(&store, &masks, false) {
            Err(ImportError::NotAPreset(why)) => assert!(why.contains("RedHue"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            import_into(&store, &d.join("gone.xmp"), false),
            Err(ImportError::Unreadable(_))
        ));
        // A name of spaces alone, which no row could show, and one of
        // ours from a newer build.
        let blank = d.join("blank.gcp");
        std::fs::write(
            &blank,
            r#"{"name": "   ", "sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        assert!(matches!(
            import_into(&store, &blank, true),
            Err(ImportError::Nameless)
        ));
        let newer = d.join("newer.gcp");
        std::fs::write(
            &newer,
            format!(
                r#"{{"version": {}, "name": "Next", "sections": ["grain"], "edit": {{}}}}"#,
                VERSION + 1
            ),
        )
        .unwrap();
        match import_into(&store, &newer, false) {
            Err(e @ ImportError::Newer(v)) => {
                assert_eq!(v, VERSION + 1);
                assert_eq!(
                    e.to_string(),
                    format!("from a newer greycard (schema {})", VERSION + 1)
                );
            }
            other => panic!("{other:?}"),
        }
        // A write that fails says so in the file system's words.
        let unsaved = ImportError::Unsaved(
            "Other".into(),
            Error::Io(std::io::Error::other("disk full")),
        );
        assert_eq!(unsaved.to_string(), "saving Other: disk full");
        assert_eq!(store.list().len(), 1);

        // "Faded!" is another name with the same slug: its own file.
        let bang = d.join("bang.gcp");
        std::fs::write(
            &bang,
            r#"{"name": "Faded!", "sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        let (written, _) = import_into(&store, &bang, false).unwrap();
        assert_eq!(written, store.dir().join("faded-2.gcp"));
        assert_eq!(
            std::fs::read_to_string(store.dir().join("faded.gcp")).unwrap(),
            before
        );
        assert_eq!(store.list().len(), 2);
        greycard_core::testing::remove_dir_retry(&d);
    }

    /// A Lightroom file with the exposure at `ev`, and a `crs:Group`
    /// when `group` names one.
    fn lightroom(ev: &str, group: Option<&str>) -> String {
        let group = group
            .map(|g| format!(r#"<crs:Group><rdf:Alt><rdf:li xml:lang="x-default">{g}</rdf:li></rdf:Alt></crs:Group>"#))
            .unwrap_or_default();
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="{ev}">{group}</rdf:Description></rdf:RDF></x:xmpmeta>"#
        )
    }

    /// A `.lrtemplate` with the exposure at `ev`, titled `title`.
    fn template(title: &str, ev: &str) -> String {
        format!(
            "s = {{\n\ttitle = \"{title}\",\n\ttype = \"Develop\",\n\tvalue = {{\n\t\tsettings = {{\n\
             \t\t\tExposure2012 = {ev},\n\t\t\tRedHue = 10,\n\t\t}},\n\t}},\n\tversion = 0,\n}}\n"
        )
    }

    /// Lightroom's older presets come in wherever its XMP ones do: a
    /// file named, in any case, and a folder's, beside XMP and ours;
    /// one of another type is no preset; one past the most a template
    /// may be is refused unread.
    #[test]
    fn an_lrtemplate_imports_as_an_xmp_does() {
        let d = dir("lrtemplate");
        let pack = d.join("Old Pack");
        std::fs::create_dir_all(pack.join("Mono")).unwrap();
        std::fs::write(pack.join("warm.lrtemplate"), template("Warm", "0.5")).unwrap();
        std::fs::write(pack.join("cool.xmp"), lightroom("-0.5", None)).unwrap();
        std::fs::write(
            pack.join("Mono").join("Tri-X.LRTEMPLATE"),
            template("Tri-X", "1"),
        )
        .unwrap();
        let got = import(&pack.join("warm.lrtemplate")).unwrap();
        assert_eq!(got.preset.name, "Warm");
        assert_eq!(got.preset.edit.light.exposure, 0.5);
        assert_eq!(got.unmapped, vec!["RedHue"]);
        let g = |s: &str| Some(s.to_string());
        let gathered = gather(std::slice::from_ref(&pack));
        assert_eq!(
            gathered,
            [
                (pack.join("cool.xmp"), g("Old Pack")),
                (pack.join("warm.lrtemplate"), g("Old Pack")),
                (pack.join("Mono").join("Tri-X.LRTEMPLATE"), g("Mono")),
            ]
        );
        let store = Store::at(d.join("store"));
        for (path, folder) in &gathered {
            import_into_group(&store, path, false, folder.as_deref()).unwrap();
        }
        let listed: Vec<(Option<String>, String)> = store
            .list()
            .into_iter()
            .map(|e| (store.group_of(&e.path), e.preset.name))
            .collect();
        assert_eq!(
            listed,
            [
                (g("Mono"), "Tri-X".to_string()),
                (g("Old Pack"), "cool".into()),
                (g("Old Pack"), "Warm".into()),
            ]
        );

        let export = d.join("export.lrtemplate");
        std::fs::write(
            &export,
            "s = { title = \"Web\", type = \"Export\", value = { settings = { Exposure2012 = 1 } } }",
        )
        .unwrap();
        match import_into(&store, &export, false) {
            Err(ImportError::NotAPreset(why)) => {
                assert_eq!(why, "no Camera Raw settings in the file")
            }
            other => panic!("{other:?}"),
        }
        let large = d.join("large.lrtemplate");
        let file = std::fs::File::create(&large).unwrap();
        file.set_len(crate::lightroom::TEMPLATE_MOST as u64 + 1)
            .unwrap();
        drop(file);
        match import_into(&store, &large, false) {
            Err(ImportError::NotAPreset(why)) => assert_eq!(why, "larger than 2 MB"),
            other => panic!("{other:?}"),
        }
        greycard_core::testing::remove_dir_retry(&d);
    }

    /// A preset file is read no further than a byte past its kind's
    /// most, whatever its size says: an XMP past its own, a template
    /// one byte past, one that is a pipe fed without end, and one that
    /// is a device saying it is empty are refused there; one at the
    /// most still reads.
    #[test]
    fn a_preset_file_is_read_no_further_than_its_most() {
        let d = dir("bounded");
        std::fs::create_dir_all(&d).unwrap();
        let refused = |path: &Path, most: usize| match import(path) {
            Err(Error::Preset(why)) => assert_eq!(why, format!("larger than {} MB", most >> 20)),
            other => panic!("{}: {other:?}", path.display()),
        };
        let xmp = d.join("long.xmp");
        let mut text = lightroom("+0.5", None);
        text.push_str(&" ".repeat(crate::lightroom::XMP_MOST + 1 - text.len()));
        std::fs::write(&xmp, &text).unwrap();
        refused(&xmp, crate::lightroom::XMP_MOST);
        text.pop();
        std::fs::write(&xmp, &text).unwrap();
        assert_eq!(import(&xmp).unwrap().preset.edit.light.exposure, 0.5);

        let most = crate::lightroom::TEMPLATE_MOST;
        let at = d.join("at.lrtemplate");
        let mut text = template("At", "1");
        text.push_str(&" ".repeat(most - text.len()));
        std::fs::write(&at, &text).unwrap();
        assert_eq!(import(&at).unwrap().preset.name, "At");
        text.push(' ');
        std::fs::write(&at, &text).unwrap();
        refused(&at, most);

        #[cfg(unix)]
        {
            // A device that says it is empty and never ends.
            let zero = d.join("zero.lrtemplate");
            std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
            refused(&zero, most);
            // Ours, and a file of no kind taken as ours, the same.
            for name in ["zero.gcp", "zero"] {
                let zero = d.join(name);
                std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
                refused(&zero, GCP_MOST);
            }
            // A pipe, fed four times the most by a writer that stops
            // when the reading end goes.
            let fifo = d.join("fifo.lrtemplate");
            let made = std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap();
            assert!(made.success());
            let writer = {
                let fifo = fifo.clone();
                std::thread::spawn(move || {
                    use std::io::Write;
                    let mut pipe = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
                    let chunk = vec![b' '; 1 << 16];
                    let mut sent = 0;
                    while sent < 4 * most && pipe.write_all(&chunk).is_ok() {
                        sent += chunk.len();
                    }
                    sent
                })
            };
            refused(&fifo, most);
            assert!(writer.join().unwrap() < 4 * most, "stopped by the reader");
        }
        greycard_core::testing::remove_dir_retry(&d);
    }

    /// A Lightroom preset folder imported is a group of its name, and
    /// each folder in it one of that folder's; a file's own
    /// `crs:Group` comes first. Files picked from one folder are its
    /// group when two or more are Lightroom's; one alone is in none.
    #[test]
    fn a_folder_imported_is_a_group() {
        let d = dir("folder-import");
        let pack = d.join("Film Pack");
        std::fs::create_dir_all(pack.join("Mono")).unwrap();
        std::fs::create_dir_all(pack.join(".hidden")).unwrap();
        std::fs::write(pack.join("warm.xmp"), lightroom("+0.5", None)).unwrap();
        std::fs::write(pack.join("cool.XMP"), lightroom("-0.5", Some("Cold Ones"))).unwrap();
        std::fs::write(pack.join("readme.txt"), "not a preset").unwrap();
        std::fs::write(pack.join("Mono").join("tri-x.xmp"), lightroom("+1", None)).unwrap();
        std::fs::write(pack.join(".hidden").join("no.xmp"), lightroom("+1", None)).unwrap();
        let gathered = gather(std::slice::from_ref(&pack));
        let g = |s: &str| Some(s.to_string());
        assert_eq!(
            gathered,
            [
                (pack.join("cool.XMP"), g("Film Pack")),
                (pack.join("warm.xmp"), g("Film Pack")),
                (pack.join("Mono").join("tri-x.xmp"), g("Mono")),
            ]
        );
        let store = Store::at(d.join("store"));
        for (path, folder) in &gathered {
            import_into_group(&store, path, false, folder.as_deref()).unwrap();
        }
        let listed: Vec<(Option<String>, String)> = store
            .list()
            .into_iter()
            .map(|e| (store.group_of(&e.path), e.preset.name))
            .collect();
        assert_eq!(
            listed,
            [
                (g("Cold Ones"), "cool".to_string()),
                (g("Film Pack"), "warm".into()),
                (g("Mono"), "tri-x".into()),
            ]
        );

        // Picked as files, from one folder or not: in no group.
        let loose = d.join("Downloads");
        std::fs::create_dir_all(&loose).unwrap();
        let (a, b, c) = (
            loose.join("a.xmp"),
            loose.join("b.xmp"),
            loose.join("c.gcp"),
        );
        let picked = gather(&[a.clone(), b.clone(), c.clone()]);
        assert_eq!(picked, [(a.clone(), None), (b, None), (c, None)]);
        assert_eq!(gather(std::slice::from_ref(&a)), [(a, None)]);
        // Adobe's localized group: its English.
        std::fs::write(
            loose.join("adobe.xmp"),
            lightroom("+0.1", Some("$$$/AgSettingNames/Group=Portraits")),
        )
        .unwrap();
        let (written, got) =
            import_into_group(&store, &loose.join("adobe.xmp"), false, None).unwrap();
        assert_eq!(got.group.as_deref(), Some("Portraits"));
        assert_eq!(written, store.dir().join("Portraits").join("adobe.gcp"));
        // A group there in another case is that group.
        let other = d.join("Other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("soft.xmp"), lightroom("+0.2", Some("mono"))).unwrap();
        let (written, _) = import_into_group(&store, &other.join("soft.xmp"), false, None).unwrap();
        assert_eq!(written, store.dir().join("Mono").join("soft.gcp"));
        greycard_core::testing::remove_dir_retry(&d);
    }
}
