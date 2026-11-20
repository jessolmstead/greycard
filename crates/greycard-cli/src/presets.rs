//! `greycard presets`: the store listed, preset files imported, and a
//! preset laid over files' edits.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// A preset file's refusal as `--import` prints it, with the way past
/// a name the store has.
fn not_imported(path: &std::path::Path, e: &greycard_edit_io::preset::ImportError) -> String {
    let hint = if matches!(e, greycard_edit_io::preset::ImportError::Taken(_)) {
        "; --replace writes over it"
    } else {
        ""
    };
    format!("not imported  {}: {e}{hint}", path.display())
}

/// The lens corrections on the developed picture, from the database's
/// profile when one is asked for and found, and the manual ones.
/// The presets subcommand: what the store holds, and imports and
/// applications asked for.
pub(crate) fn presets(
    files: &[PathBuf],
    import: &[PathBuf],
    replace: bool,
    apply: Option<&str>,
    dir: Option<PathBuf>,
) -> Result<()> {
    use greycard_edit::preset::Store;
    let store = match dir {
        Some(dir) => Store::at(dir),
        None => {
            let store = Store::user().context("finding the configuration directory")?;
            // As the editor does on its way up: the shipped presets,
            // once. Only the user's own store — a `--dir` is somewhere
            // the caller named, a staging directory or a copy, and
            // dropping three presets into it is not what was asked.
            store.seed();
            store
        }
    };
    // Every file tried, as the window tries them: a refusal is said
    // where it falls and the rest still come in, and the run fails at
    // the end if any was refused.
    // A folder is its preset files, each with the group it gives them.
    let gathered = greycard_edit_io::preset::gather(import);
    for dir in import.iter().filter(|p| p.is_dir()) {
        if !gathered.iter().any(|(p, _)| p.starts_with(dir)) {
            eprintln!(
                "no presets    {}: no .xmp, .lrtemplate, or .gcp files in it",
                dir.display()
            );
        }
    }
    let mut refused = 0;
    for (path, folder) in &gathered {
        let (
            written,
            greycard_edit_io::lightroom::Imported {
                preset, unmapped, ..
            },
        ) = match greycard_edit_io::preset::import_into_group(
            &store,
            path,
            replace,
            folder.as_deref(),
        ) {
            Ok(got) => got,
            Err(e) => {
                eprintln!("{}", not_imported(path, &e));
                refused += 1;
                continue;
            }
        };
        println!(
            "imported      {} ({}) into {}",
            preset.name,
            section_names(&preset),
            written.display()
        );
        if !unmapped.is_empty() {
            println!("              no place here for {}", unmapped.join(", "));
        }
    }
    if refused > 0 {
        anyhow::bail!("{refused} of {} preset files not imported", gathered.len());
    }
    if let Some(name) = apply {
        let entry = store
            .find(name)
            .with_context(|| format!("no preset called {name} in {}", store.dir().display()))?;
        if files.is_empty() {
            anyhow::bail!("--apply wants files to lay {} over", entry.preset.name);
        }
        // A raw with no sidecar starts as the editor's setting says.
        let fresh = greycard_engine::settings::Shared::load().fresh();
        for file in files {
            let mut sidecar = sidecar_for(file, fresh)?;
            // The tone curve's switch only on a DCP that carries one.
            let edit = entry
                .preset
                .settled(&greycard_edit_io::Stores::user())
                .applied(&sidecar.current);
            let label = greycard_edit::history::preset_label(&entry.preset.name);
            if sidecar.record_as(edit, Some(label)) {
                write_back(&mut sidecar, file)?;
                println!("applied       {} to {}", entry.preset.name, file.display());
                if !sidecar.versions.is_empty() {
                    println!(
                        "              on the original; its {} other versions are as they were",
                        sidecar.versions.len()
                    );
                }
            } else {
                println!(
                    "unchanged     {} has {} already",
                    file.display(),
                    entry.preset.name
                );
            }
        }
    } else {
        for file in files {
            list_versions(file)?;
        }
    }
    if import.is_empty() && apply.is_none() && files.is_empty() {
        let entries = store.list();
        println!("store         {}", store.dir().display());
        if entries.is_empty() {
            println!("no presets; save one from the editor, or --import a file");
        }
        for e in &entries {
            println!(
                "{:<13} {} ({})",
                e.preset.name,
                section_names(&e.preset),
                e.path.display()
            );
        }
    }
    Ok(())
}

/// `sidecar` saved back where `file`'s was found (the CLI has no
/// setting), asking the disk first: a save another writer made since
/// the read (the editor's window on the same frame) is never written
/// over unseen; it is kept beside the frame when both changed the
/// frame's lines ([`greycard_edit::Sidecar::save_checked`]).
fn write_back(
    sidecar: &mut greycard_edit::Sidecar,
    file: &std::path::Path,
) -> Result<greycard_edit::Checked> {
    sidecar
        .save_checked(
            file,
            greycard_edit::Placement::of(file),
            &greycard_edit::sync::Stamp::now(),
        )
        .with_context(|| format!("writing the sidecar of {}", file.display()))
}

/// A frame's sidecar to lay something over: the one it has, wherever
/// it is, or a new one at the edit the editor would open the frame at,
/// a raw's lens section as `fresh` says.
fn sidecar_for(
    file: &std::path::Path,
    fresh: greycard_edit_io::on_disk::Fresh,
) -> Result<greycard_edit::Sidecar> {
    use greycard_edit::Sidecar;
    let mut sidecar = match Sidecar::load(file)
        .with_context(|| format!("reading the sidecar of {}", file.display()))?
    {
        Some(s) => s,
        None if greycard_core::picture::is_picture_path(file) => Sidecar {
            line: greycard_edit::Line::of(greycard_edit::Edit::for_picture()),
            ..Sidecar::default()
        },
        // A raw's learned-denoiser blend starts from its ISO, probed
        // without a full decode; a probe that fails says so and
        // leaves it at the old constant.
        None => {
            let iso = match greycard_core::decode::probe_path(file) {
                Ok(p) => p.iso,
                Err(e) => {
                    tracing::warn!(
                        "no iso from {}, the learned blend keeps its default: {e}",
                        file.display()
                    );
                    None
                }
            };
            let mut sidecar = greycard_edit_io::on_disk::fresh_sidecar(file, fresh);
            sidecar.line.current.noise.learned_strength = greycard_edit::Noise::blend_for_iso(iso);
            sidecar
        }
    };
    // A sidecar from before schema 4 does not know which way up an
    // `Original` crop goes until the frame says; the file is here, so
    // ask it before writing anything back (see
    // `Edit::migrate_with_frame`).
    if sidecar.needs_frame() {
        match greycard_core::decode::stance_path(file)
            .ok()
            .and_then(|s| s.shown_size(sidecar.turn))
        {
            Some((w, h)) => sidecar.migrate_with_frame(h > w),
            None => tracing::warn!(
                "{}: will not say which way up it is, so its Original crop \
                 is left for the editor to bring up to date",
                file.display()
            ),
        }
    }
    Ok(sidecar)
}

/// What a frame's sidecar holds, a line a version: the original's
/// states and step, then each version's name, id and states, in the
/// order they were made.
fn list_versions(file: &std::path::Path) -> Result<()> {
    use greycard_edit::{Line, Sidecar};
    let Some(sidecar) = Sidecar::load(file)
        .with_context(|| format!("reading the sidecar of {}", file.display()))?
    else {
        println!("no sidecar    {}", file.display());
        return Ok(());
    };
    let said = |line: &Line| {
        let states = line.states();
        let plural = if states == 1 { "" } else { "s" };
        match line.current_label.as_deref() {
            Some(step) => format!("{states} state{plural}, at \"{step}\""),
            None => format!("{states} state{plural}"),
        }
    };
    println!("sidecar       {}", file.display());
    println!("original      {}", said(&sidecar.line));
    for v in &sidecar.versions {
        println!("version       {} ({}): {}", v.name, v.id, said(&v.line));
    }
    Ok(())
}

/// A preset's sections, in a row, then its parts each under its
/// section's name ("Detail: Dehaze").
fn section_names(preset: &greycard_edit::Preset) -> String {
    let names: Vec<String> = preset
        .sections
        .iter()
        .map(|s| s.title().to_string())
        .chain(
            preset
                .parts
                .iter()
                .map(|p| format!("{}: {}", p.section().title(), p.title())),
        )
        .collect();
    if names.is_empty() {
        "nothing".into()
    } else {
        names.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use greycard_edit_io::on_disk::Fresh;

    /// A raw with no sidecar starts with its lens section as the
    /// setting for new edits says; one with a sidecar keeps its own.
    #[test]
    fn a_preset_on_a_raw_with_no_sidecar_starts_as_the_setting_says() {
        let d =
            std::env::temp_dir().join(format!("greycard-cli-preset-fresh-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&d);
        std::fs::create_dir_all(&d).unwrap();
        let raw = d.join("IMG_0001.CR3");
        std::fs::write(&raw, b"not a raw").unwrap();
        let off = Fresh { lens: false };
        assert!(!super::sidecar_for(&raw, off).unwrap().current.lens.enabled);
        assert!(
            super::sidecar_for(&raw, Fresh::default())
                .unwrap()
                .current
                .lens
                .enabled
        );
        let mut written = greycard_edit::Sidecar::default();
        written.meta.rating = 4;
        written.save(&raw).unwrap();
        let kept = super::sidecar_for(&raw, off).unwrap();
        assert!(kept.current.lens.enabled, "the file's own");
        assert_eq!(kept.meta.rating, 4);
        greycard_core::testing::remove_dir_retry(&d);
    }

    /// The editor saving the frame between the CLI's read and its write:
    /// the preset written, and the editor's file, with its state and
    /// rating, kept beside the frame.
    #[test]
    fn a_save_between_the_read_and_the_write_is_kept() {
        let d =
            std::env::temp_dir().join(format!("greycard-cli-preset-gap-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&d);
        std::fs::create_dir_all(&d).unwrap();
        let raw = d.join("IMG_0001.CR3");
        std::fs::write(&raw, b"not a raw").unwrap();
        let mut first = greycard_edit::Sidecar::default();
        first.record(greycard_edit::Edit::default());
        first.save(&raw).unwrap();
        let mut sidecar = super::sidecar_for(&raw, Fresh::default()).unwrap();
        let mut grain = sidecar.current.clone();
        grain.grain.amount = 0.4;
        sidecar.record_as(grain, Some("Preset: Fine Grain".into()));
        // The editor, between the two.
        let mut editor = greycard_edit::Sidecar::load(&raw).unwrap().unwrap();
        let mut bright = editor.current.clone();
        bright.light.exposure = 1.0;
        editor.record(bright);
        editor.meta.rating = 2;
        editor.save(&raw).unwrap();
        let checked = super::write_back(&mut sidecar, &raw).unwrap();
        assert_eq!(checked, greycard_edit::Checked::Kept);
        let back = greycard_edit::Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back.current.grain.amount, 0.4);
        let path = greycard_edit::Sidecar::path_for(&raw);
        let kept = greycard_edit::Sidecar::read(&path.with_extension("gcd.other")).unwrap();
        assert_eq!(kept.current.light.exposure, 1.0);
        assert_eq!(kept.meta.rating, 2);
        greycard_core::testing::remove_dir_retry(&d);
    }

    /// `--import` refuses a name the store has and says how to
    /// replace it, tries every file whatever is refused, and fails at
    /// the end; `--replace` writes over the one there, in its file.
    #[test]
    fn a_preset_import_replaces_only_on_asking() {
        use std::path::PathBuf;
        let d =
            std::env::temp_dir().join(format!("greycard-cli-preset-import-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&d);
        std::fs::create_dir_all(&d).unwrap();
        let store = d.join("store");
        let gcp = d.join("grain.gcp");
        let write = |amount: f32| {
            std::fs::write(
                &gcp,
                format!(
                    r#"{{"name": "Fine Grain", "sections": ["grain"],
                        "edit": {{"grain": {{"amount": {amount}}}}}}}"#
                ),
            )
            .unwrap()
        };
        let open = || greycard_edit::preset::Store::at(store.clone());
        let amount = || open().find("Fine Grain").unwrap().preset.edit.grain.amount;
        let import = |paths: &[PathBuf], replace| {
            super::presets(&[], paths, replace, None, Some(store.clone()))
        };
        write(0.2);
        import(std::slice::from_ref(&gcp), false).unwrap();
        assert_eq!(amount(), 0.2);

        // Refused, and the files after the refusals still tried: a
        // file that is no preset first, the name the store has, then
        // a new one, which comes in. The run fails at the end.
        write(0.5);
        let junk = d.join("junk.xmp");
        std::fs::write(&junk, "not xml").unwrap();
        let other = d.join("other.gcp");
        std::fs::write(
            &other,
            r#"{"name": "Other", "sections": ["grain"], "edit": {"grain": {"amount": 0.1}}}"#,
        )
        .unwrap();
        let failed = import(&[junk, gcp.clone(), other], false).unwrap_err();
        assert_eq!(failed.to_string(), "2 of 3 preset files not imported");
        assert!(open().find("Other").is_some(), "tried after the refusals");
        assert_eq!(amount(), 0.2, "refused, the one there is as it was");
        // What it printed for the name the store has.
        let e = greycard_edit_io::preset::import_into(&open(), &gcp, false).unwrap_err();
        assert_eq!(
            super::not_imported(&gcp, &e),
            format!(
                "not imported  {}: there is a preset called \"Fine Grain\" already; \
                 --replace writes over it",
                gcp.display()
            )
        );

        import(std::slice::from_ref(&gcp), true).unwrap();
        assert_eq!(amount(), 0.5);
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 2);
        greycard_core::testing::remove_dir_retry(&d);
    }

    /// `--import` takes Lightroom's older `.lrtemplate` presets, named
    /// or in a folder, as it takes its XMP ones.
    #[test]
    fn a_preset_import_takes_an_lrtemplate() {
        let d = std::env::temp_dir().join(format!(
            "greycard-cli-preset-lrtemplate-{}",
            std::process::id()
        ));
        greycard_core::testing::remove_dir_retry(&d);
        let pack = d.join("Old Pack");
        std::fs::create_dir_all(&pack).unwrap();
        let template = |title: &str| {
            format!(
                "s = {{ title = \"{title}\", type = \"Develop\", \
                 value = {{ settings = {{ Exposure2012 = 0.5, ToneCurvePV2012 = {{ 0, 0, 255, 240 }}, }} }}, }}"
            )
        };
        let loose = d.join("loose.lrtemplate");
        std::fs::write(&loose, template("Loose")).unwrap();
        std::fs::write(pack.join("faded.lrtemplate"), template("Faded")).unwrap();
        let store = d.join("store");
        super::presets(&[], &[loose, pack], false, None, Some(store.clone())).unwrap();
        let open = greycard_edit::preset::Store::at(store);
        let faded = open.find("Faded").unwrap();
        assert_eq!(open.group_of(&faded.path).as_deref(), Some("Old Pack"));
        assert_eq!(faded.preset.edit.light.exposure, 0.5);
        assert_eq!(faded.preset.edit.curves.rgb[1][1], 240.0 / 255.0);
        assert!(open.find("Loose").is_some());
        greycard_core::testing::remove_dir_retry(&d);
    }
}
