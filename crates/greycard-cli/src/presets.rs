//! `greycard presets`: the store listed, preset files imported, and a
//! preset laid over files' edits.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// A preset file's refusal as `--import` prints it, with the way past
/// a name the store has.
fn not_imported(path: &std::path::Path, e: &greycard_edit::preset::ImportError) -> String {
    let hint = if matches!(e, greycard_edit::preset::ImportError::Taken(_)) {
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
    let mut refused = 0;
    for path in import {
        let (
            written,
            greycard_edit::lightroom::Imported {
                preset, unmapped, ..
            },
        ) = match store.import(path, replace) {
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
        anyhow::bail!("{refused} of {} preset files not imported", import.len());
    }
    if let Some(name) = apply {
        let entry = store
            .find(name)
            .with_context(|| format!("no preset called {name} in {}", store.dir().display()))?;
        if files.is_empty() {
            anyhow::bail!("--apply wants files to lay {} over", entry.preset.name);
        }
        for file in files {
            let mut sidecar = sidecar_for(file)?;
            let edit = entry.preset.applied(&sidecar.current);
            let label = greycard_edit::history::preset_label(&entry.preset.name);
            if sidecar.record_as(edit, Some(label)) {
                // Back where it was found: the CLI has no setting.
                sidecar
                    .save_in(file, greycard_edit::Placement::of(file))
                    .with_context(|| format!("writing the sidecar of {}", file.display()))?;
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

/// A frame's sidecar to lay something over: the one it has, wherever
/// it is, or a new one at the edit the editor would open the frame at.
fn sidecar_for(file: &std::path::Path) -> Result<greycard_edit::Sidecar> {
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
            let mut edit = greycard_edit::Edit::default();
            edit.noise.learned_strength = greycard_edit::Noise::blend_for_iso(iso);
            Sidecar {
                line: greycard_edit::Line::of(edit),
                ..Sidecar::default()
            }
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
    /// `--import` refuses a name the store has and says how to
    /// replace it, tries every file whatever is refused, and fails at
    /// the end; `--replace` writes over the one there, in its file.
    #[test]
    fn a_preset_import_replaces_only_on_asking() {
        use std::path::PathBuf;
        let d =
            std::env::temp_dir().join(format!("greycard-cli-preset-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
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
        let e = open().import(&gcp, false).unwrap_err();
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
        std::fs::remove_dir_all(&d).unwrap();
    }
}
