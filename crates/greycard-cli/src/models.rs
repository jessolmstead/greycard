//! `greycard models`: the learned models' registry, and fetching from
//! it.

use anyhow::{Context, Result};

use crate::common::{denoiser_tiers, megabytes};

/// The models `what` names: a denoiser tier, a model's id, or `all`.
fn models_named(what: &str) -> Result<Vec<&'static greycard_ai::Model>> {
    if what == "all" {
        return Ok(greycard_ai::MODELS.iter().collect());
    }
    if let Some(model) = greycard_ai::denoiser(what).or_else(|| greycard_ai::model(what)) {
        return Ok(vec![model]);
    }
    let ids: Vec<&str> = greycard_ai::MODELS.iter().map(|m| m.id).collect();
    anyhow::bail!(
        "want all, a denoiser tier ({}) or a model id ({}), not {what}",
        denoiser_tiers(),
        ids.join(", ")
    )
}

/// One model's two lines in the listing: the id and what it is for,
/// then its size, its license and whether the store has it.
fn model_lines(
    model: &greycard_ai::Model,
    store: &greycard_ai::Store,
    width: usize,
) -> [String; 2] {
    let tier = greycard_ai::tier_of(model)
        .map(|t| format!("tier {t}, "))
        .unwrap_or_default();
    [
        format!("{:width$}  {}", model.id, model.purpose),
        format!(
            "{:width$}  {tier}{} MB, {}, {}",
            "",
            megabytes(model.bytes()),
            model.license.name,
            if store.have(model) {
                "in the store"
            } else {
                "not fetched"
            }
        ),
    ]
}

/// What `--fetch` takes for Microsoft's CPU-only ONNX Runtime, besides
/// its id.
const RUNTIME_WORD: &str = "onnxruntime";

/// Whether `--fetch WHAT` names the CPU-only runtime.
fn names_runtime(what: &str) -> bool {
    what == RUNTIME_WORD || greycard_ai::registry::runtime(what).is_some()
}

/// The CPU-only runtime's two lines in the listing.
fn runtime_lines(
    runtime: &greycard_ai::Runtime,
    store: &greycard_ai::Store,
    width: usize,
) -> [String; 2] {
    [
        format!(
            "{:width$}  {}, on the CPU only",
            runtime.id, runtime.purpose
        ),
        format!(
            "{:width$}  `--fetch {RUNTIME_WORD}`, {} MB, {}, {}",
            "",
            megabytes(runtime.archive.bytes),
            runtime.license.name,
            if store.have_runtime(runtime) {
                "in the store"
            } else {
                "not fetched"
            }
        ),
    ]
}

/// The `models` command: the registry, and fetching from it.
pub(crate) fn models(fetch: Option<&str>) -> Result<()> {
    let store = greycard_ai::Store::user().context("finding the model store")?;
    // The models can be listed and fetched anywhere, to carry to
    // another machine; this one is told it cannot run them.
    if let Err(why) = greycard_ai::runtime::status() {
        eprintln!("{why}");
    }
    // The CPU-only runtime is listed where it is wanted or had, and
    // `all` takes it where it is wanted.
    let runtime = greycard_ai::cpu_runtime()
        .filter(|r| greycard_ai::runtime::fetchable().is_some() || store.runtime_dir(r).is_dir());
    println!("store         {}", store.root().display());
    let Some(what) = fetch else {
        let width = greycard_ai::MODELS
            .iter()
            .map(|m| m.id.len())
            .chain(runtime.map(|r| r.id.len()))
            .max()
            .unwrap_or(0);
        for model in greycard_ai::MODELS {
            println!();
            for line in model_lines(model, &store, width) {
                println!("{line}");
            }
        }
        if let Some(runtime) = runtime {
            println!();
            for line in runtime_lines(runtime, &store, width) {
                println!("{line}");
            }
        }
        if greycard_ai::MODELS.iter().any(|m| !store.have(m)) {
            println!();
            println!("fetch one with `greycard models --fetch <tier|id|all>`; nothing is bundled");
        }
        return Ok(());
    };
    if names_runtime(what) {
        return fetch_runtime(&store, runtime_for(what)?);
    }
    let mut failed = fetch_models(&store, &models_named(what)?);
    if what == "all"
        && greycard_ai::runtime::fetchable().is_some()
        && let Err(e) = runtime_for(RUNTIME_WORD).and_then(|r| fetch_runtime(&store, r))
    {
        failed.push((RUNTIME_WORD, e.to_string()));
    }
    if !failed.is_empty() {
        anyhow::bail!(
            "{} of the models could not be fetched: {}",
            failed.len(),
            failed
                .iter()
                .map(|(id, why)| format!("{id} ({why})"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    Ok(())
}

/// Fetch each of `models` the store lacks, printing what each is as
/// it goes. A model that fails is reported and passed over, so one
/// unreachable file does not keep the rest from arriving; the
/// failures come back, by id, with why.
fn fetch_models(
    store: &greycard_ai::Store,
    models: &[&'static greycard_ai::Model],
) -> Vec<(&'static str, String)> {
    let mut failed = Vec::new();
    for model in models {
        println!();
        println!("{}", model.id);
        println!("  for         {}", model.purpose);
        println!(
            "  license     {} <{}>",
            model.license.name, model.license.url
        );
        println!("  source      {}", model.source);
        println!(
            "  size        {} MB in {} file{}",
            megabytes(model.bytes()),
            model.files.len(),
            if model.files.len() == 1 { "" } else { "s" }
        );
        if store.have(model) {
            println!("  already in  {}", store.dir(model).display());
            continue;
        }
        let start = std::time::Instant::now();
        let mut reporter = FetchReport::new();
        let fetched = store.fetch(model, |p| reporter.report(p));
        // Close the rewritten line before anything else is printed,
        // so a failure does not land on the end of it.
        reporter.done();
        match fetched {
            Ok(()) => println!(
                "  fetched     into {} in {:.1} s",
                store.dir(model).display(),
                start.elapsed().as_secs_f32()
            ),
            Err(e) => {
                println!("  failed      {e}");
                failed.push((model.id, e.to_string()));
            }
        }
    }
    failed
}

/// The CPU-only runtime `--fetch WHAT` asks for, which must be this
/// platform's: another platform's id is refused by name rather than
/// taken as this one's.
fn runtime_for(what: &str) -> Result<&'static greycard_ai::Runtime> {
    let ours = greycard_ai::cpu_runtime();
    match ours {
        Some(r) if what == RUNTIME_WORD || what == r.id => Ok(r),
        _ if what != RUNTIME_WORD => anyhow::bail!(
            "{what} is ONNX Runtime for another platform; {}",
            match ours {
                Some(r) => format!("this one's is {} (`--fetch {RUNTIME_WORD}`)", r.id),
                None => "this one has none to fetch".to_string(),
            }
        ),
        _ if greycard_ai::runtime::SHIPS => anyhow::bail!(
            "no CPU-only ONNX Runtime is offered for this platform: greycard's own runs on \
             every processor it is built for"
        ),
        _ => anyhow::bail!(
            "no CPU-only ONNX Runtime is offered for this platform, and greycard ships none \
             of its own for it; GREYCARD_ORT_DYLIB can name one"
        ),
    }
}

/// Fetch Microsoft's CPU-only ONNX Runtime for this platform into the
/// store, printing what it is first as a model's fetch does, then load
/// it where it is the one to use, and say whether the AI features now
/// run here. One that will not load is an error, and is removed from
/// the store again (`runtime::load_fetched`).
fn fetch_runtime(store: &greycard_ai::Store, runtime: &'static greycard_ai::Runtime) -> Result<()> {
    println!();
    println!("{}", runtime.id);
    println!("  is          {}", runtime.name);
    println!("  for         {}, on the CPU only, slower", runtime.purpose);
    println!(
        "  license     {} <{}>",
        runtime.license.name, runtime.license.url
    );
    println!("  source      {}", runtime.source);
    println!(
        "  size        {} MB to download, {} MB kept",
        megabytes(runtime.archive.bytes),
        megabytes(runtime.bytes())
    );
    // Fetched on request wherever it is asked for, to carry to
    // another machine, but said first when it would not be used here.
    if let Ok(first) = greycard_ai::runtime::status()
        && !greycard_ai::runtime::cpu_only()
    {
        println!(
            "  note        {} comes first on this machine, so this one will not be used here",
            first.display()
        );
    }
    // Asked every time: it downloads nothing when everything is here,
    // and puts back a license or notices file that has gone.
    let start = std::time::Instant::now();
    let mut reporter = FetchReport::new();
    let mut downloaded = false;
    let fetched = store.fetch_runtime(runtime, |p| {
        downloaded = true;
        reporter.report(p)
    });
    reporter.done();
    let path = fetched.context("fetching ONNX Runtime")?;
    if downloaded {
        println!(
            "  fetched     {} in {:.1} s",
            path.display(),
            start.elapsed().as_secs_f32()
        );
    } else {
        println!("  already in  {}", store.runtime_dir(runtime).display());
    }
    match greycard_ai::runtime::load_fetched(store, runtime) {
        Ok(true) => println!("the AI features run here now, on the CPU only, slower"),
        Ok(false) => println!("another ONNX Runtime comes first here; this one is kept unused"),
        Err(why @ greycard_ai::runtime::Unavailable::WouldNotLoad { .. }) => {
            anyhow::bail!("{why}; removed from the store again")
        }
        Err(why) => anyhow::bail!("{why}"),
    }
    Ok(())
}

/// The download's progress on stderr: a line rewritten in place at a
/// terminal, a line a file otherwise, and never faster than the eye.
struct FetchReport {
    terminal: bool,
    last: std::time::Instant,
    open: bool,
}

impl FetchReport {
    fn new() -> Self {
        use std::io::IsTerminal;
        Self {
            terminal: std::io::stderr().is_terminal(),
            last: std::time::Instant::now(),
            open: false,
        }
    }

    fn report(&mut self, p: greycard_ai::Progress) {
        use std::io::Write;
        let first = p.done == 0;
        let last = p.total > 0 && p.done == p.total;
        if !self.terminal {
            if first {
                eprintln!("  fetching    {} ({} MB)", p.file, megabytes(p.total));
            }
            return;
        }
        if !(first || last || self.last.elapsed().as_millis() > 200) {
            return;
        }
        self.last = std::time::Instant::now();
        let percent = if p.total > 0 {
            format!(" {:3.0}%", 100.0 * p.done as f64 / p.total as f64)
        } else {
            String::new()
        };
        eprint!(
            "\r  fetching    {} {} of {} MB{percent}",
            p.file,
            megabytes(p.done),
            megabytes(p.total)
        );
        let _ = std::io::stderr().flush();
        self.open = true;
    }

    /// Close the line the reports were written over.
    fn done(&mut self) {
        if self.open {
            eprintln!();
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that cannot be fetched is reported and passed over:
    /// the ones after it are still tried, and each failure comes back.
    #[test]
    fn a_failed_fetch_does_not_stop_the_rest() {
        macro_rules! unreachable_model {
            ($id:literal, $file:literal) => {
                greycard_ai::Model {
                    id: $id,
                    name: "unreachable test model",
                    purpose: "a test",
                    source: "nowhere",
                    license: greycard_ai::registry::License {
                        name: "none",
                        url: "https://example.invalid/",
                    },
                    modified: None,
                    beside: None,
                    files: &[greycard_ai::registry::File {
                        name: $file,
                        // Port 9 (discard) on the loopback: refused at once.
                        url: concat!("http://127.0.0.1:9/", $file),
                        bytes: 1,
                        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
                    }],
                }
            };
        }
        static FIRST: greycard_ai::Model = unreachable_model!("unreachable-first", "first.onnx");
        static SECOND: greycard_ai::Model = unreachable_model!("unreachable-second", "second.onnx");
        let store = greycard_ai::Store::at(
            std::env::temp_dir().join(format!("greycard-fetch-failures-{}", std::process::id())),
        );
        let failed = fetch_models(&store, &[&FIRST, &SECOND]);
        let ids: Vec<_> = failed.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["unreachable-first", "unreachable-second"]);
        let _ = std::fs::remove_dir_all(store.root());
    }

    /// `--fetch onnxruntime`, or the runtime's id, is the CPU-only
    /// runtime and not a model.
    #[test]
    fn the_fetch_argument_names_the_cpu_runtime_by_word_or_id() {
        assert!(names_runtime(RUNTIME_WORD));
        for r in greycard_ai::registry::RUNTIMES {
            assert!(names_runtime(r.id), "{}", r.id);
        }
        for other in ["all", "fast", greycard_ai::SKY.id, "onnx"] {
            assert!(!names_runtime(other), "{other}");
        }
        assert!(
            greycard_ai::runtime::FETCH_RUNTIME.ends_with(&format!("--fetch {RUNTIME_WORD}")),
            "the line names the word this takes"
        );
    }

    /// Another platform's runtime is refused by its id, not fetched
    /// as this platform's in its place.
    #[test]
    fn another_platforms_runtime_is_refused_by_name() {
        let ours = greycard_ai::cpu_runtime();
        for r in greycard_ai::registry::RUNTIMES {
            match runtime_for(r.id) {
                Ok(got) => assert_eq!(Some(got), ours, "{}", r.id),
                Err(e) => {
                    assert_ne!(Some(r), ours);
                    let e = e.to_string();
                    assert!(e.starts_with(r.id) && e.contains("another platform"), "{e}");
                }
            }
        }
        assert_eq!(runtime_for(RUNTIME_WORD).ok(), ours);
    }

    /// `--fetch` takes a tier, an id or `all`, and says so when it is
    /// given none of them.
    #[test]
    fn the_fetch_argument_takes_a_tier_an_id_or_all() {
        let ids = |what| {
            models_named(what)
                .unwrap()
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("all").len(), greycard_ai::MODELS.len());
        assert_eq!(ids("fast"), vec![greycard_ai::DENOISE_FAST.id]);
        assert_eq!(
            ids(greycard_ai::SUBJECT.id),
            vec![greycard_ai::SUBJECT.id],
            "a model with no tier is reachable by its id"
        );
        let complaint = models_named("everything").unwrap_err().to_string();
        assert!(complaint.contains("everything"), "{complaint}");
        assert!(complaint.contains("fast, balanced, best"), "{complaint}");
        assert!(complaint.contains(greycard_ai::SUBJECT.id), "{complaint}");
    }

    /// The listing says, for every model, what it is for, what it
    /// weighs, its license, and whether the store has it.
    #[test]
    fn the_listing_covers_every_model() {
        let store = greycard_ai::Store::at(
            std::env::temp_dir().join(format!("greycard-models-{}", std::process::id())),
        );
        let width = greycard_ai::MODELS
            .iter()
            .map(|m| m.id.len())
            .max()
            .unwrap();
        for model in greycard_ai::MODELS {
            let [first, second] = model_lines(model, &store, width);
            assert!(first.starts_with(model.id), "{first}");
            assert!(first.contains(model.purpose), "{first}");
            assert!(second.contains(model.license.name), "{second}");
            assert!(
                second.contains(&format!("{} MB", megabytes(model.bytes()))),
                "{second}"
            );
            assert!(second.contains("not fetched"), "{second}");
            match greycard_ai::tier_of(model) {
                Some(tier) => assert!(second.contains(&format!("tier {tier}")), "{second}"),
                None => assert!(!second.contains("tier "), "{second}"),
            }
        }
        assert_eq!(megabytes(greycard_ai::DENOISE_FAST.bytes()), 5);
    }
}
