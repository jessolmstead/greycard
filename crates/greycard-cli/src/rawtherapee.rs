//! `greycard rawtherapee`: RawTherapee's .pp3 profiles brought across.

use std::path::PathBuf;

use anyhow::Result;

/// The rawtherapee subcommand: each frame's profile read and reported,
/// and written only when asked, then the totals. The import is the
/// library's, the window's sheet's too
/// ([`greycard_library::rawtherapee::import`]).
pub(crate) fn rawtherapee(paths: &[PathBuf], write: bool, force: bool) -> Result<()> {
    use greycard_library::rawtherapee;
    if paths.is_empty() {
        anyhow::bail!("give the raws, or the folders they are in");
    }
    let options = rawtherapee::Options {
        write,
        force,
        // Back where it was found: the command has no setting.
        sidecars_in_folder: None,
    };
    // A frame's lines as it is done, then the totals, as the report's
    // `Display` lays them out.
    let say = |lines: Vec<(String, String)>| {
        for (label, text) in lines {
            println!("{label:<13} {text}");
        }
    };
    let report = rawtherapee::import_each(paths, &options, &mut |step| match step {
        rawtherapee::Step::Listed { passed } => say(passed
            .iter()
            .map(|p| ("not a frame".to_string(), p.display().to_string()))
            .collect()),
        rawtherapee::Step::Frame {
            report: Some(frame),
            ..
        } => say(frame.lines(rawtherapee::Voice::Command)),
        rawtherapee::Step::Frame { report: None, .. } => {}
    })
    .map_err(|e| anyhow::Error::new(e.source).context(format!("reading {}", e.path.display())))?;
    say(report.totals(rawtherapee::Voice::Command));
    let failed = report.failed();
    if failed > 0 {
        anyhow::bail!("{failed} sidecars would not write");
    }
    Ok(())
}
