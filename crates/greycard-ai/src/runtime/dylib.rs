//! Where ONNX Runtime comes from, and whether it can run here, decided
//! before anything of it is in the process.
//!
//! The runtime is a shared library loaded the first time a model is
//! asked for (`ort`'s `load-dynamic`), never linked into the
//! executables: the one greycard ships is linked by
//! `scripts/ort-dylib.sh` out of pyke's prebuilt archive, which is
//! compiled for x86-64-v3, and linked in, its static initializers ran
//! before `main` and an older processor died at exec with an illegal
//! instruction. So the processor is asked first, by the CPUID bits
//! the standard library already reads, and the library is looked for
//! where the build and the package put it; only then is it loaded.
//!
//! `GREYCARD_ORT_DYLIB` names a library by hand and is taken without
//! the processor check: the check is about the library greycard
//! ships, and one named by hand (Microsoft's own CPU-only build, say,
//! which runs on any x86-64) is the user's call. `ORT_DYLIB_PATH`,
//! `ort`'s own variable, is not read: it is anybody's, set for some
//! other program in a user's environment, and taken on its word it
//! would hand an x86-64-v3 library to a processor that cannot run it.
//! `ort` itself reads it only when nothing was loaded through
//! `init_from`, which `runtime::init` always is.
//!
//! Where the processor cannot run the shipped library, Microsoft's own
//! release for the CPU can (`registry::cpu_runtime`): fetched into the
//! model store as a model is, and taken from there once it is, its
//! hash checked again first since it is code to be run. Until then the
//! AI features are off with a line that says it can be fetched. So the
//! order is: the library named by hand, the shipped one where the
//! processor can run it, a fetched one, and else off.
//!
//! On a platform greycard ships no runtime for at all ([`SHIPS`] is
//! false: Windows on ARM, where pyke's one build has DirectML, which
//! greycard does not use, and none has WebGPU), there is no shipped
//! step and no processor to ask, and the order is the library named
//! by hand, a fetched one, and else off, with the same line that it
//! can be fetched where the registry has one for the platform.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::registry::Runtime;

/// The library the decision found, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub path: PathBuf,
    pub from: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// `GREYCARD_ORT_DYLIB`.
    Named,
    /// The one greycard ships, beside the executable.
    Shipped,
    /// Microsoft's CPU-only release, fetched into the model store.
    Fetched,
}

/// The library's file name on this platform: what `scripts/ort-dylib.sh`
/// makes and `scripts/package.sh` ships beside the executables.
pub const LIBRARY: &str = if cfg!(windows) {
    "onnxruntime.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime.dylib"
} else {
    "libonnxruntime.so"
};

/// Whether greycard ships an ONNX Runtime of its own for this build's
/// platform: true exactly where `crates/ort-dylib` has an archive of
/// pyke's to make one from (Linux on x86-64, macOS on Apple silicon,
/// and Windows on x86-64), so where a package carries one beside the
/// executables.
pub const SHIPS: bool = cfg!(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(windows, target_arch = "x86_64", target_env = "msvc"),
));

/// Why the AI features are off on this machine: one plain line, for
/// the log, the status line and the command line's stderr alike.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unavailable {
    /// The processor lacks instructions the shipped library was
    /// compiled to use.
    Processor {
        missing: Vec<&'static str>,
        /// The processor's own name, from CPUID, when it gives one.
        name: Option<String>,
    },
    /// The same, where Microsoft's CPU-only runtime, which this
    /// processor can run, can be fetched and has not been yet.
    Fetchable {
        missing: Vec<&'static str>,
        name: Option<String>,
        runtime: &'static Runtime,
    },
    /// greycard ships no ONNX Runtime for this platform, and nothing
    /// was named or fetched: `runtime` is Microsoft's CPU-only one,
    /// where the registry has one for the platform to fetch.
    NotShipped { runtime: Option<&'static Runtime> },
    /// No library beside the executable (or, for a test or an
    /// example, in the profile directory above it): every place
    /// looked, empty when where greycard runs from is not known.
    NotBeside { looked: Vec<PathBuf> },
    /// `GREYCARD_ORT_DYLIB` names a file that is not there.
    NotAt { path: PathBuf },
    /// It is there and would not load.
    WouldNotLoad { path: PathBuf, why: String },
}

/// The command line's word for fetching the CPU-only runtime, which
/// `greycard models` takes.
pub const FETCH_RUNTIME: &str = "greycard models --fetch onnxruntime";

impl Unavailable {
    /// The CPU-only runtime that would turn the AI features on, where
    /// that is why they are off.
    pub fn fetchable(&self) -> Option<&'static Runtime> {
        match self {
            Unavailable::Fetchable { runtime, .. } => Some(runtime),
            Unavailable::NotShipped { runtime } => *runtime,
            _ => None,
        }
    }

    /// Why, without the opening "the AI features are off": for a sheet
    /// that says the rest itself.
    pub fn reason(&self) -> String {
        let line = self.to_string();
        let (_, why) = line.split_once(": ").unwrap_or(("", &line));
        why.to_string()
    }
}

/// "this processor (NAME) has no A, B or C".
fn processor(f: &mut fmt::Formatter<'_>, missing: &[&str], name: &Option<String>) -> fmt::Result {
    f.write_str("this processor")?;
    if let Some(name) = name {
        write!(f, " ({name})")?;
    }
    write!(f, " has no {}", listed(missing))
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A fetchable line opens with its remedy; the rest, plainly.
        if self.fetchable().is_some() {
            write!(
                f,
                "the AI features are off until Microsoft's ONNX Runtime for the CPU is \
                 downloaded ({FETCH_RUNTIME}): "
            )?;
        } else {
            f.write_str("the AI features are off: ")?;
        }
        match self {
            Unavailable::Fetchable { missing, name, .. } => {
                processor(f, missing, name)?;
                f.write_str(", which greycard's ONNX Runtime is built for")
            }
            Unavailable::NotShipped { runtime } => {
                f.write_str("no ONNX Runtime of greycard's own is made for this platform")?;
                if runtime.is_none() {
                    f.write_str("; GREYCARD_ORT_DYLIB can name one")?;
                }
                Ok(())
            }
            Unavailable::Processor { missing, name } => {
                processor(f, missing, name)?;
                f.write_str(", which greycard's ONNX Runtime is built for")
            }
            Unavailable::NotBeside { looked } if looked.is_empty() => {
                write!(f, "no {LIBRARY}, and no knowing where greycard runs from")
            }
            Unavailable::NotBeside { looked } => {
                let places: Vec<String> = looked
                    .iter()
                    .map(|p| p.parent().unwrap_or(p).display().to_string())
                    .collect();
                write!(
                    f,
                    "no {LIBRARY} beside greycard (looked in {}); a build from source makes it \
                     with scripts/ort-dylib.sh",
                    places.join(" and ")
                )
            }
            Unavailable::NotAt { path } => {
                write!(
                    f,
                    "GREYCARD_ORT_DYLIB names {}, which is not there",
                    path.display()
                )
            }
            Unavailable::WouldNotLoad { path, why } => {
                write!(
                    f,
                    "ONNX Runtime at {} would not load: {why}",
                    path.display()
                )
            }
        }
    }
}

/// "AVX2", "AVX2 or BMI2", "AVX2, BMI2 or FMA".
fn listed(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// What the decision is made from, gathered by [`probe`] on a real
/// machine and given directly by a test.
pub(crate) struct Facts<'a> {
    /// The instructions the shipped library needs that the processor
    /// lacks; empty where it has them all.
    pub missing: Vec<&'static str>,
    pub processor: Option<String>,
    /// `GREYCARD_ORT_DYLIB`, if set.
    pub env: Option<&'a OsStr>,
    /// The running executable.
    pub exe: Option<&'a Path>,
    /// The CPU-only runtime the registry has for this platform, if it
    /// has one.
    pub runtime: Option<&'static Runtime>,
    /// Whether greycard ships a runtime for this platform at all
    /// ([`SHIPS`]); where it does not, nothing beside the executable
    /// is looked for.
    pub shipped: bool,
}

/// The library to load, or why there is none to: a library named by
/// `GREYCARD_ORT_DYLIB` first, and taken on its word; then the library
/// greycard ships, where `candidates` says it is, if the processor can
/// run it; then a CPU-only runtime fetched into the store, which
/// `fetched` answers with (asked only when it is reached, as it hashes
/// the file); and else off, saying whether that runtime can be had.
/// Where greycard ships no runtime for the platform, the second step
/// is not taken. `exists` is asked of each path, so a test can lay out
/// a directory that is not there.
pub(crate) fn decide(
    facts: &Facts<'_>,
    exists: impl Fn(&Path) -> bool,
    fetched: impl FnOnce() -> Option<PathBuf>,
) -> Result<Found, Unavailable> {
    let exe_dir = facts.exe.and_then(Path::parent);
    if let Some(env) = facts.env.filter(|e| !e.is_empty()) {
        let named = PathBuf::from(env);
        // Relative to the executable when there is a file there, as
        // `ort` reads its own variable, else as given (relative to
        // where we run).
        let path = match exe_dir {
            Some(dir) if named.is_relative() && exists(&dir.join(&named)) => dir.join(&named),
            _ => named,
        };
        return if exists(&path) {
            Ok(Found {
                path,
                from: Source::Named,
            })
        } else {
            Err(Unavailable::NotAt { path })
        };
    }
    let looked = if facts.shipped {
        facts.exe.map(candidates).unwrap_or_default()
    } else {
        Vec::new()
    };
    if facts.missing.is_empty()
        && let Some(path) = looked.iter().find(|p| exists(p))
    {
        return Ok(Found {
            path: path.clone(),
            from: Source::Shipped,
        });
    }
    if let Some(path) = fetched() {
        return Ok(Found {
            path,
            from: Source::Fetched,
        });
    }
    if !facts.shipped {
        return Err(Unavailable::NotShipped {
            runtime: facts.runtime,
        });
    }
    if facts.missing.is_empty() {
        return Err(Unavailable::NotBeside { looked });
    }
    let (missing, name) = (facts.missing.clone(), facts.processor.clone());
    Err(match facts.runtime {
        Some(runtime) => Unavailable::Fetchable {
            missing,
            name,
            runtime,
        },
        None => Unavailable::Processor { missing, name },
    })
}

/// The executable as a file rather than a link to one: `current_exe`
/// answers with the path it was started by on macOS, and the
/// installer links `~/.local/bin/greycard` into the bundle, where the
/// library is. Followed to the file it names, or as given when it
/// cannot be. Not on Windows, where the installer links nothing and
/// `canonicalize` answers with a `\\?\` path that would then be in
/// every line and handed to LoadLibraryExW.
pub(crate) fn real_exe(exe: PathBuf) -> PathBuf {
    if cfg!(unix) {
        std::fs::canonicalize(&exe).unwrap_or(exe)
    } else {
        exe
    }
}

/// An error and every error under it, as one line: `ort`'s load error
/// says only that `dlopen` failed, and why (a glibc too old, Dawn not
/// beside it) is the error it wraps.
pub(crate) fn chain(e: &dyn std::error::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut next = Some(e);
    while let Some(e) = next {
        let text = e.to_string();
        let line = text.lines().next().unwrap_or("").trim().to_string();
        if !line.is_empty() && !parts.iter().any(|p| p.contains(&line)) {
            parts.push(line);
        }
        next = e.source();
    }
    parts.join(": ")
}

/// Where the shipped library sits for `exe`: beside it, which is
/// where a package puts it and `target/<profile>/` holds it for a
/// build; and for a test (`target/<profile>/deps`) or an example
/// (`target/<profile>/examples`), in the profile directory above.
pub(crate) fn candidates(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    let mut out = vec![dir.join(LIBRARY)];
    if dir
        .file_name()
        .is_some_and(|n| n == "deps" || n == "examples")
        && let Some(profile) = dir.parent()
    {
        out.push(profile.join(LIBRARY));
    }
    out
}

/// The decision for this machine and this process.
pub(crate) fn probe() -> Result<Found, Unavailable> {
    let env = std::env::var_os("GREYCARD_ORT_DYLIB");
    let exe = std::env::current_exe().ok().map(real_exe);
    let missing = missing_instructions();
    let processor = if missing.is_empty() {
        None
    } else {
        processor_name()
    };
    let runtime = crate::registry::cpu_runtime();
    decide(
        &Facts {
            missing,
            processor,
            env: env.as_deref(),
            exe: exe.as_deref(),
            runtime,
            shipped: SHIPS,
        },
        Path::is_file,
        || {
            let runtime = runtime?;
            let store = crate::Store::user().ok()?;
            store
                .have_runtime(runtime)
                .then(|| store.runtime_library(runtime))
        },
    )
}

/// The x86-64-v3 instructions pyke's build of ONNX Runtime assumes
/// (and Dawn's with it) that this processor lacks. An AArch64 build
/// assumes nothing a Mac or an ARM Linux machine lacks.
fn missing_instructions() -> Vec<&'static str> {
    #[cfg(target_arch = "x86_64")]
    {
        let mut missing = Vec::new();
        macro_rules! want {
            ($($feature:tt => $name:literal),* $(,)?) => {
                $(if !std::arch::is_x86_feature_detected!($feature) {
                    missing.push($name);
                })*
            };
        }
        want!(
            "avx" => "AVX",
            "avx2" => "AVX2",
            "bmi1" => "BMI1",
            "bmi2" => "BMI2",
            "f16c" => "F16C",
            "fma" => "FMA",
            "lzcnt" => "LZCNT",
            "movbe" => "MOVBE",
        );
        missing
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Vec::new()
    }
}

/// The processor's brand string, from the CPUID leaves every x86-64
/// processor since 2003 answers: the same text `/proc/cpuinfo` shows
/// as its model name, on any platform and without reading a file.
fn processor_name() -> Option<String> {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        #[allow(unused_unsafe)]
        let max = unsafe { __cpuid(0x8000_0000) }.eax;
        if max < 0x8000_0004 {
            return None;
        }
        let mut bytes = Vec::with_capacity(48);
        for leaf in 0x8000_0002u32..=0x8000_0004 {
            #[allow(unused_unsafe)]
            let r = unsafe { __cpuid(leaf) };
            for word in [r.eax, r.ebx, r.ecx, r.edx] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        let text = String::from_utf8_lossy(&bytes);
        let name = text
            .trim_matches(char::from(0))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!name.is_empty()).then_some(name)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(missing: &[&'static str], env: Option<&'a OsStr>, exe: &'a Path) -> Facts<'a> {
        Facts {
            missing: missing.to_vec(),
            processor: Some("Intel(R) Core(TM) i5-3450 CPU @ 3.10GHz".into()),
            env,
            exe: Some(exe),
            runtime: None,
            shipped: true,
        }
    }

    /// The decision's path, with no CPU-only runtime fetched.
    fn decided(facts: &Facts<'_>, exists: impl Fn(&Path) -> bool) -> Result<PathBuf, Unavailable> {
        decide(facts, exists, || None).map(|f| f.path)
    }

    const RUNTIME: &Runtime = &crate::registry::ONNXRUNTIME_CPU_LINUX_X64;

    /// A processor the shipped library cannot run on, where the
    /// registry has Microsoft's CPU-only runtime: off until it is
    /// fetched, and the line says so and how; once it is in the store,
    /// that one, and said to be the fetched one.
    #[test]
    fn a_processor_short_of_v3_fetches_the_cpu_runtime_and_then_loads_it() {
        let exe = Path::new("/opt/greycard/bin/greycard-ui");
        let beside = Path::new("/opt/greycard/bin").join(LIBRARY);
        let there = |p: &Path| p == beside;
        let short = Facts {
            runtime: Some(RUNTIME),
            ..facts(&["AVX2", "BMI2"], None, exe)
        };
        // Not fetched yet: fetchable, whatever is beside.
        let off = decide(&short, there, || None).unwrap_err();
        assert_eq!(off.fetchable(), Some(RUNTIME));
        let line = off.to_string();
        assert!(line.starts_with("the AI features are off until"), "{line}");
        assert!(
            line.contains("Microsoft's ONNX Runtime for the CPU"),
            "{line}"
        );
        assert!(line.contains(FETCH_RUNTIME), "{line}");
        assert!(
            line.contains("i5-3450") && line.contains("AVX2 or BMI2"),
            "{line}"
        );
        assert!(!line.contains('\n'));
        // The reason alone, for the sheet, without the opening.
        assert!(
            off.reason().starts_with("this processor (Intel"),
            "{}",
            off.reason()
        );
        // Without a runtime to fetch (a platform the registry has none
        // for): off as before, and nothing offered.
        let none = decide(&facts(&["AVX2"], None, exe), there, || None).unwrap_err();
        assert!(matches!(none, Unavailable::Processor { .. }));
        assert_eq!(none.fetchable(), None);
        assert!(!none.to_string().contains(FETCH_RUNTIME));

        // Fetched and its hash checked: that one, CPU only.
        let store = PathBuf::from("/home/u/.cache/greycard/models/rt").join(LIBRARY);
        let fetched = || Some(store.clone());
        assert_eq!(
            decide(&short, there, fetched),
            Ok(Found {
                path: store.clone(),
                from: Source::Fetched
            })
        );
    }

    /// The order: the library named by hand, then the shipped one
    /// where the processor can run it, then a fetched one, then off.
    #[test]
    fn the_preference_is_named_then_shipped_then_fetched_then_off() {
        let exe = Path::new("/opt/greycard/bin/greycard-ui");
        let beside = Path::new("/opt/greycard/bin").join(LIBRARY);
        let named = PathBuf::from("/usr/lib/microsoft/libonnxruntime.so");
        let store = PathBuf::from("/home/u/.cache/greycard/models/rt").join(LIBRARY);
        let everything = |p: &Path| p == beside || p == named;
        let only_beside = |p: &Path| p == beside;
        let nothing = |_: &Path| false;
        let with = |missing: &'static [&'static str], env: Option<&'static OsStr>| Facts {
            runtime: Some(RUNTIME),
            ..facts(missing, env, exe)
        };
        let env = Some(OsStr::new("/usr/lib/microsoft/libonnxruntime.so"));
        let from = |r: Result<Found, Unavailable>| r.map(|f| f.from);

        // Named beats all, on any processor, and the store is not asked.
        let asked = std::cell::Cell::new(false);
        let fetched = || {
            asked.set(true);
            Some(store.clone())
        };
        assert_eq!(
            from(decide(&with(&[], env), everything, fetched)),
            Ok(Source::Named)
        );
        assert!(
            !asked.get(),
            "the fetched one is not hashed when not wanted"
        );
        assert_eq!(
            from(decide(&with(&["AVX2"], env), everything, || Some(
                store.clone()
            ))),
            Ok(Source::Named)
        );
        // A processor that can run it: the shipped one before a
        // fetched one, and the store not asked.
        let asked = std::cell::Cell::new(false);
        let fetched = || {
            asked.set(true);
            Some(store.clone())
        };
        assert_eq!(
            from(decide(&with(&[], None), only_beside, fetched)),
            Ok(Source::Shipped)
        );
        assert!(!asked.get());
        // One that cannot: the fetched one, though the shipped is there.
        assert_eq!(
            from(decide(&with(&["BMI2"], None), only_beside, || Some(
                store.clone()
            ))),
            Ok(Source::Fetched)
        );
        // A processor that can, with nothing beside (a build without
        // the script): a fetched one still serves.
        assert_eq!(
            from(decide(&with(&[], None), nothing, || Some(store.clone()))),
            Ok(Source::Fetched)
        );
        // Nothing anywhere: off, fetchable only where the processor is
        // why; a build without its library is told how to make it.
        assert!(
            decide(&with(&["BMI2"], None), nothing, || None)
                .unwrap_err()
                .fetchable()
                .is_some()
        );
        assert!(matches!(
            decide(&with(&[], None), nothing, || None),
            Err(Unavailable::NotBeside { .. })
        ));
    }

    /// Where greycard ships no runtime for the platform (Windows on
    /// ARM): named, then fetched, then off, and nothing beside the
    /// executable is looked for or taken. Off is the fetchable kind,
    /// naming the fetch command and offering Microsoft's runtime, and
    /// it says why without a processor in it or a build script to run.
    #[test]
    fn where_nothing_is_shipped_the_order_is_named_then_fetched_then_off() {
        let arm64 = &crate::registry::ONNXRUNTIME_CPU_WINDOWS_ARM64;
        let exe = Path::new("/opt/greycard/bin/greycard-ui");
        let beside = Path::new("/opt/greycard/bin").join(LIBRARY);
        let named = PathBuf::from("/opt/microsoft/onnxruntime.dll");
        let store = PathBuf::from("/home/u/.cache/greycard/models/rt").join(LIBRARY);
        let everything = |p: &Path| p == beside || p == named;
        let only_beside = |p: &Path| p == beside;
        let unshipped = |env: Option<&'static OsStr>| Facts {
            missing: Vec::new(),
            processor: None,
            runtime: Some(arm64),
            shipped: false,
            ..facts(&[], env, exe)
        };
        let from = |r: Result<Found, Unavailable>| r.map(|f| f.from);

        // Named first, and the store not asked.
        let env = Some(OsStr::new("/opt/microsoft/onnxruntime.dll"));
        let asked = std::cell::Cell::new(false);
        let fetched = || {
            asked.set(true);
            Some(store.clone())
        };
        assert_eq!(
            decide(&unshipped(env), everything, fetched),
            Ok(Found {
                path: named.clone(),
                from: Source::Named
            })
        );
        assert!(!asked.get());
        // Then the fetched one, though a library sits beside: none is
        // shipped here, so one there is nobody's to trust.
        assert_eq!(
            from(decide(&unshipped(None), only_beside, || Some(
                store.clone()
            ))),
            Ok(Source::Fetched)
        );
        // Then off, the kind that offers the fetch, whatever is beside.
        let off = decide(&unshipped(None), only_beside, || None).unwrap_err();
        assert_eq!(
            off,
            Unavailable::NotShipped {
                runtime: Some(arm64)
            }
        );
        assert_eq!(off.fetchable(), Some(arm64));
        let line = off.to_string();
        assert_eq!(
            line,
            format!(
                "the AI features are off until Microsoft's ONNX Runtime for the CPU is \
                 downloaded ({FETCH_RUNTIME}): no ONNX Runtime of greycard's own is made for \
                 this platform"
            )
        );
        for wrong in ["processor", "AVX2", "ort-dylib", "beside"] {
            assert!(!line.contains(wrong), "{wrong:?} in {line}");
        }
        assert_eq!(
            off.reason(),
            "no ONNX Runtime of greycard's own is made for this platform"
        );
        // A platform with neither (Linux on ARM): off, nothing offered,
        // and the variable that can name one said.
        let bare = decide(
            &Facts {
                runtime: None,
                ..unshipped(None)
            },
            only_beside,
            || None,
        )
        .unwrap_err();
        assert_eq!(bare.fetchable(), None);
        let line = bare.to_string();
        assert!(
            line.starts_with("the AI features are off: no ONNX Runtime"),
            "{line}"
        );
        assert!(line.contains("GREYCARD_ORT_DYLIB"), "{line}");
        assert!(!line.contains(FETCH_RUNTIME), "{line}");
    }

    #[test]
    fn the_processor_and_the_library_decide_together() {
        let exe = Path::new("/opt/greycard/bin/greycard-ui");
        let beside = Path::new("/opt/greycard/bin").join(LIBRARY);
        let there = |p: &Path| p == beside;
        let nothing = |_: &Path| false;

        // A processor with every instruction, the library beside: load it.
        assert_eq!(decided(&facts(&[], None, exe), there), Ok(beside.clone()));
        // Without them, off, whatever is beside; and named.
        let short = decided(&facts(&["AVX2", "BMI2", "FMA"], None, exe), there).unwrap_err();
        assert!(matches!(short, Unavailable::Processor { .. }));
        let line = short.to_string();
        assert!(line.contains("i5-3450"), "{line}");
        assert!(line.contains("AVX2, BMI2 or FMA"), "{line}");
        assert!(!line.contains('\n'));
        // With them and nothing beside: off, and where it looked.
        assert_eq!(
            decided(&facts(&[], None, exe), nothing),
            Err(Unavailable::NotBeside {
                looked: vec![beside.clone()]
            })
        );
    }

    /// A test's binary looks in two places, and the line names both
    /// and how a developer makes the library.
    #[test]
    fn the_line_for_no_library_names_every_place_and_the_remedy() {
        let exe = Path::new("/t/debug/deps/models-0123");
        let line = decided(&facts(&[], None, exe), |_| false)
            .unwrap_err()
            .to_string();
        assert!(line.contains("/t/debug/deps and /t/debug"), "{line}");
        assert!(line.contains("scripts/ort-dylib.sh"), "{line}");
        assert!(!line.contains('\n'));
    }

    #[test]
    fn a_library_named_by_hand_is_taken_on_its_word() {
        let exe = Path::new("/opt/greycard/bin/greycard-ui");
        let mine = PathBuf::from("/usr/lib/microsoft/libonnxruntime.so");
        let there = |p: &Path| p == mine;
        // Even on a processor the shipped library cannot run on: the
        // one named is somebody's own choice, a CPU-only build, say.
        let env = mine.as_os_str();
        assert_eq!(
            decided(&facts(&["AVX2"], Some(env), exe), there),
            Ok(mine.clone())
        );
        // Named and not there: off, and said so, rather than falling
        // back to the shipped one the name was meant to replace.
        let gone = OsStr::new("/nowhere/libonnxruntime.so");
        assert_eq!(
            decided(&facts(&[], Some(gone), exe), there),
            Err(Unavailable::NotAt {
                path: PathBuf::from("/nowhere/libonnxruntime.so")
            })
        );
        // Empty is unset.
        let beside = Path::new("/opt/greycard/bin").join(LIBRARY);
        assert_eq!(
            decided(&facts(&[], Some(OsStr::new("")), exe), |p| p == beside),
            Ok(beside)
        );
        // Relative: beside the executable when it is there.
        let rel = OsStr::new("lib/custom.so");
        let under = PathBuf::from("/opt/greycard/bin/lib/custom.so");
        assert_eq!(
            decided(&facts(&[], Some(rel), exe), |p| p == under),
            Ok(under.clone())
        );
    }

    /// The installer links `~/.local/bin/greycard` into the bundle on
    /// a Mac, where `current_exe` answers with the link: the library
    /// is looked for beside the file it names.
    #[cfg(unix)]
    #[test]
    fn a_linked_executable_is_followed_to_where_the_library_is() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-dylib-linked-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let (bundle, bin) = (dir.join("greycard.app"), dir.join("bin"));
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bundle.join("greycard"), b"").unwrap();
        std::fs::write(bundle.join(LIBRARY), b"").unwrap();
        std::os::unix::fs::symlink(bundle.join("greycard"), bin.join("greycard")).unwrap();

        let exe = real_exe(bin.join("greycard"));
        let found = decided(&facts(&[], None, &exe), Path::is_file).unwrap();
        assert_eq!(found, std::fs::canonicalize(bundle.join(LIBRARY)).unwrap());
        // Unfollowed, the link's own directory has nothing.
        let raw = bin.join("greycard");
        assert!(decided(&facts(&[], None, &raw), Path::is_file).is_err());
        // A path that is not there is kept as given.
        assert_eq!(real_exe(dir.join("gone")), dir.join("gone"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ort`'s load error says `dlopen` failed; why is the error it
    /// wraps, and the line carries both.
    #[test]
    fn a_load_error_is_said_with_what_it_wraps() {
        #[derive(Debug)]
        struct Outer(Inner);
        #[derive(Debug)]
        struct Inner;
        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("failed to load from `/x/libonnxruntime.so`: dlopen failed")
            }
        }
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "libwebgpu_dawn.so: cannot open shared object file: No such file or directory",
                )
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        impl std::error::Error for Inner {}
        let line = chain(&Outer(Inner));
        assert!(
            line.contains("dlopen failed: libwebgpu_dawn.so: cannot open"),
            "{line}"
        );
    }

    #[test]
    fn the_library_is_found_beside_the_executable_and_above_a_test() {
        // A package's binary, and a build's: beside it.
        assert_eq!(
            candidates(Path::new("/t/release/greycard-ui")),
            [Path::new("/t/release").join(LIBRARY)]
        );
        // A test in target/<profile>/deps and an example in
        // target/<profile>/examples: beside it first, then the
        // profile directory, where the build's binaries are.
        assert_eq!(
            candidates(Path::new("/t/debug/deps/models-0123")),
            [
                Path::new("/t/debug/deps").join(LIBRARY),
                Path::new("/t/debug").join(LIBRARY)
            ]
        );
        assert_eq!(
            candidates(Path::new("/t/debug/examples/probe")),
            [
                Path::new("/t/debug/examples").join(LIBRARY),
                Path::new("/t/debug").join(LIBRARY)
            ]
        );
        let exe = Path::new("/t/debug/deps/models-0123");
        let above = Path::new("/t/debug").join(LIBRARY);
        assert_eq!(
            decided(&facts(&[], None, exe), |p| p == above),
            Ok(above.clone())
        );
        // Nowhere else: not a parent of an ordinary binary's directory.
        assert_eq!(
            candidates(Path::new("/usr/local/bin/greycard")),
            [Path::new("/usr/local/bin").join(LIBRARY)]
        );
    }

    /// The real directory layout a build leaves, on disk: the test's
    /// own executable is in `target/<profile>/deps`, and the library,
    /// when `scripts/ort-dylib.sh` has run, is in the profile
    /// directory above it, which `candidates` names.
    #[test]
    fn this_test_binary_looks_in_its_own_profile_directory() {
        let exe = std::env::current_exe().unwrap();
        let profile = exe.parent().and_then(Path::parent).unwrap();
        assert!(candidates(&exe).contains(&profile.join(LIBRARY)));
    }

    #[test]
    fn the_processor_check_names_only_what_is_missing() {
        // Whatever this machine is, the list is a subset of the
        // x86-64-v3 additions, and an AArch64 one asks for nothing.
        let v3 = [
            "AVX", "AVX2", "BMI1", "BMI2", "F16C", "FMA", "LZCNT", "MOVBE",
        ];
        let missing = missing_instructions();
        assert!(missing.iter().all(|m| v3.contains(m)), "{missing:?}");
        if cfg!(target_arch = "x86_64") {
            // Every x86-64 processor this runs on answers the brand
            // leaves; the name is printable and on one line.
            if let Some(name) = processor_name() {
                assert!(!name.contains('\n') && !name.contains('\0'), "{name:?}");
            }
        } else {
            assert!(missing.is_empty());
        }
    }

    #[test]
    fn the_lines_read_plainly() {
        assert_eq!(listed(&["AVX2"]), "AVX2");
        assert_eq!(listed(&["AVX2", "BMI2"]), "AVX2 or BMI2");
        let no_name = Unavailable::Processor {
            missing: vec!["AVX2"],
            name: None,
        };
        assert_eq!(
            no_name.to_string(),
            "the AI features are off: this processor has no AVX2, which greycard's ONNX Runtime \
             is built for"
        );
        let load = Unavailable::WouldNotLoad {
            path: PathBuf::from("/x/libonnxruntime.so"),
            why: "bad ELF".into(),
        };
        assert_eq!(
            load.to_string(),
            "the AI features are off: ONNX Runtime at /x/libonnxruntime.so would not load: bad ELF"
        );
    }
}
