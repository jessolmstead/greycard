// Makes the ONNX Runtime shared library greycard loads the first time
// a model is asked for, and puts it beside the binaries in
// `target/<profile>/`, where `scripts/package.sh` takes it from.
//
//     scripts/ort-dylib.sh [profile ...]      (debug and release if none)
//
// Why a library of our own. `ort` links pyke's prebuilt ONNX Runtime
// statically by default, and pyke builds it for x86-64-v3 (AVX, AVX2,
// BMI1, BMI2, F16C, FMA, LZCNT, MOVBE): its static initializers run
// before `main`, so on an older x86 processor the editor died with
// an illegal instruction before any of our code ran. Loaded at first
// use instead (`ort`'s `load-dynamic`), nothing of it is in the
// process until a model is wanted, and greycard-ai asks the
// processor first. pyke publishes no shared library, only the static
// archive and Dawn beside it, and Microsoft's own releases carry no
// WebGPU; so we make the shared library from pyke's archive here.
//
// What it does, on each of the three hosts we build on:
//
// 1. Fetches the archive `ort-sys` 2.0.0-rc.13 would have fetched for
//    the `webgpu` feature (the rows below are copied from its
//    `build/download/dist.tsv`; a new `ort` means new rows), checks
//    its SHA-256, and unpacks it into
//    `target/ort-dylib/<hash>/src`. The archive is a tar inside a raw
//    LZMA2 stream, no .xz container, which `lzma-rust2` reads as
//    `ort-sys` does.
// 2. Links one shared library out of it that exports `OrtGetApiBase`
//    alone, the one symbol `ort` looks up, and needs Dawn beside it:
//
//    Linux    `c++ -shared` (or `$CXX`), the archive pulled in from
//             `-u OrtGetApiBase` and a version script that hides
//             everything else, Dawn on `-lwebgpu_dawn` with a runpath
//             of `$ORIGIN`. pyke's archive is compiled by Clang but
//             against libstdc++ (its symbols are the `__cxx11` ABI,
//             none are libc++'s `__1`), so it links `-lstdc++`, as
//             `ort-sys` itself did; it also wants glibc 2.38 or newer
//             (`__isoc23_strtol`), the floor the static build already
//             had, and libm, libgcc_s and libc beside.
//    macOS    `c++ -dynamiclib` (Apple's clang), the same root on
//             `-u _OrtGetApiBase`, `-exported_symbol` for the export,
//             install name `@rpath/libonnxruntime.dylib` and a runpath
//             of `@loader_path` for Dawn's own `@rpath` name; libc++,
//             libobjc, Foundation, CoreFoundation and CoreML (the
//             archive carries the CoreML provider and some
//             Objective-C++); built for macOS 13.4, Dawn's floor and
//             Info.plist's, or `$MACOSX_DEPLOYMENT_TARGET`.
//    Windows  MSVC's `link.exe /DLL`, found the way `cc` finds it,
//             with a `.def` exporting `OrtGetApiBase` and
//             `/INCLUDE:OrtGetApiBase` as the root, Dawn's import
//             library, and the system libraries the archive's objects
//             call into (kernel32, advapi32, setupapi, dxgi, dbghelp,
//             pathcch); the C++ runtime comes from the objects' own
//             `/DEFAULTLIB` directives (`/MD`). The toolset has to be
//             at least the one the archive was built with, 14.51.36231
//             for this one: see the header of release.yml.
//
//    Not `--whole-archive` / `/WHOLEARCHIVE`: pyke's archive holds some
//    members twice (ONNX's protobuf sources among them), so taking
//    every member fails on duplicate symbols. Rooted at
//    `OrtGetApiBase`, the linker takes exactly the members the
//    executables took when `ort` linked the archive into them.
// 3. Copies Dawn (and on Windows the two DXC libraries Dawn loads by
//    name) next to the library, and links (Windows: copies) all of
//    them into `target/<profile>/`, its `deps/` and its `examples/`,
//    where the binaries, the tests and the examples sit; `ort`'s
//    `copy-dylibs` used to put Dawn in the same three.
//
// A second run with nothing changed fetches and links nothing. The
// stamp holds a hash of the link's program, arguments and exports, so a
// changed recipe, compiler or macOS floor links again; the fetch has
// a ten-minute limit.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

/// One prebuilt ONNX Runtime from pyke: where it is, its hash, and the
/// files in it that matter.
struct Dist {
    url: &'static str,
    sha256: &'static str,
    /// The static archive the library is linked from.
    archive: &'static str,
    /// The shared libraries that come with it and must sit beside ours.
    beside: &'static [&'static str],
    /// The library made, under the name greycard-ai looks for.
    library: &'static str,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const DIST: Option<Dist> = Some(Dist {
    url: "https://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/x86_64-unknown-linux-gnu+webgpu.tar.lzma2",
    sha256: "68406bc32de516ee8baeaa4c5f2de2bb0031269f19ce8b76d64988c0498b93be",
    archive: "libonnxruntime.a",
    beside: &["libwebgpu_dawn.so"],
    library: "libonnxruntime.so",
});

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const DIST: Option<Dist> = Some(Dist {
    url: "https://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/aarch64-apple-darwin+coreml,webgpu.tar.lzma2",
    sha256: "c887d7ec10b06832d95e0fa035a28425794b26e1a5c34804bda7c6b50f7197a3",
    archive: "libonnxruntime.a",
    beside: &["libwebgpu_dawn.dylib"],
    library: "libonnxruntime.dylib",
});

#[cfg(all(windows, target_arch = "x86_64", target_env = "msvc"))]
const DIST: Option<Dist> = Some(Dist {
    url: "https://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/x86_64-pc-windows-msvc+webgpu.tar.lzma2",
    sha256: "7e21d6e157b80e910ef6bc336846ed7958f13b9dbdd0b0369d6225a59aebd825",
    archive: "onnxruntime.lib",
    beside: &["webgpu_dawn.dll", "dxcompiler.dll", "dxil.dll"],
    library: "onnxruntime.dll",
});

#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(windows, target_arch = "x86_64", target_env = "msvc"),
)))]
const DIST: Option<Dist> = None;

/// Bumped when something the stamp cannot see changes (the copying of
/// the libraries beside it, say); a change to the link's own program,
/// arguments or exports is in the stamp already (`Recipe::fingerprint`).
const RECIPE: &str = "2";

/// How long the fetch may take, the whole of it: the Windows archive
/// is 55 MB, and a stalled connection should fail the build rather
/// than hold a CI runner until its own limit.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// The dictionary `ort-sys` decodes pyke's archives with.
const DICTIONARY: u32 = 1 << 26;

/// The macOS release the library is built for unless
/// `MACOSX_DEPLOYMENT_TARGET` says otherwise: Dawn's floor, which
/// Info.plist carries too.
const MACOS_FLOOR: &str = "13.4";

fn main() -> Result<()> {
    let args = Args::from(std::env::args_os().skip(1))?;
    let Some(dist) = DIST else {
        bail!(
            "no prebuilt ONNX Runtime with WebGPU for this platform; the AI features need a \
             library given by GREYCARD_ORT_DYLIB"
        );
    };
    std::fs::create_dir_all(&args.target_dir)?;
    let target_dir = std::fs::canonicalize(&args.target_dir)?;
    let work = target_dir.join("ort-dylib").join(&dist.sha256[..16]);
    let made = make(&dist, &work)?;
    let size = std::fs::metadata(made.join(dist.library))?.len();
    println!(
        "{} is {:.1} MB, in {}",
        dist.library,
        size as f64 / 1e6,
        made.display()
    );
    let files: Vec<&str> = std::iter::once(dist.library)
        .chain(dist.beside.iter().copied())
        .collect();
    for profile in &args.profiles {
        for dir in profile_dirs(&target_dir, profile) {
            std::fs::create_dir_all(&dir)?;
            for f in &files {
                put(&made.join(f), &dir.join(f))
                    .with_context(|| format!("putting {f} in {}", dir.display()))?;
            }
        }
        println!("in {}", target_dir.join(profile).display());
    }
    Ok(())
}

/// The command line: an optional `--target-dir` and the profiles.
#[derive(Debug, PartialEq, Eq)]
struct Args {
    target_dir: PathBuf,
    profiles: Vec<String>,
}

impl Args {
    fn from(args: impl Iterator<Item = OsString>) -> Result<Args> {
        let mut target_dir = None;
        let mut profiles = Vec::new();
        let mut args = args.peekable();
        while let Some(a) = args.next() {
            if a == "--target-dir" {
                target_dir = Some(PathBuf::from(
                    args.next().context("--target-dir wants a directory")?,
                ));
            } else {
                let p = a.into_string().ok().context("a profile is a plain name")?;
                ensure!(
                    !p.starts_with('-') && !p.contains(['/', '\\']) && p != "..",
                    "{p} is not a profile"
                );
                profiles.push(p);
            }
        }
        let target_dir = target_dir
            .or_else(|| std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from))
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        if profiles.is_empty() {
            profiles = vec!["debug".into(), "release".into()];
        }
        Ok(Args {
            target_dir,
            profiles,
        })
    }
}

/// Where a profile's binaries, tests and examples sit.
fn profile_dirs(target_dir: &Path, profile: &str) -> [PathBuf; 3] {
    let base = target_dir.join(profile);
    [base.join("deps"), base.join("examples"), base]
}

/// The library and what sits beside it, made in `work/lib` unless a
/// previous run already made them by this recipe from this archive.
fn make(dist: &Dist, work: &Path) -> Result<PathBuf> {
    let out = work.join("lib");
    let src = work.join("src");
    let recipe = recipe(dist, &src, &out, work)?;
    let stamp = format!("{} recipe {RECIPE} {}\n", dist.sha256, recipe.fingerprint());
    // The unpacked archive is the archive's alone: a new recipe links
    // it again without fetching it again.
    let unpacked = format!("{}\n", dist.sha256);
    let made = |dir: &Path, stamp_name: &str, files: &[&str]| {
        std::fs::read_to_string(dir.join(stamp_name)).is_ok_and(|s| s == stamp)
            && files.iter().all(|f| dir.join(f).is_file())
    };
    let mut files = vec![dist.library];
    files.extend(dist.beside);
    if made(&out, ".made", &files) {
        return Ok(out);
    }

    let mut wanted = vec![dist.archive];
    wanted.extend(dist.beside);
    let have = |dir: &Path| {
        std::fs::read_to_string(dir.join(".unpacked")).is_ok_and(|s| s == unpacked)
            && wanted.iter().all(|f| dir.join(f).is_file())
    };
    if !have(&src) {
        let archive = work.join("archive.tar.lzma2");
        if !archive.is_file() || sha256_of(&archive)? != dist.sha256 {
            fetch(dist, &archive)?;
        }
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(&src)?;
        println!("unpacking into {}", src.display());
        let file = std::fs::File::open(&archive)?;
        let mut reader =
            lzma_rust2::Lzma2Reader::new(std::io::BufReader::new(file), DICTIONARY, None);
        untar(&mut reader, &src)?;
        for f in &wanted {
            ensure!(src.join(f).is_file(), "the archive has no {f}");
        }
        std::fs::write(src.join(".unpacked"), &unpacked)?;
        // The archive itself is not needed again once unpacked.
        let _ = std::fs::remove_file(&archive);
    }

    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;
    recipe.run()?;
    for f in dist.beside {
        std::fs::copy(src.join(f), out.join(f))?;
    }
    std::fs::write(out.join(".made"), &stamp)?;
    Ok(out)
}

/// The archive, fetched to `to` and checked against its hash.
fn fetch(dist: &Dist, to: &Path) -> Result<()> {
    println!("fetching {}", dist.url);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(dist.url)
        .call()
        .with_context(|| format!("fetching {}", dist.url))?;
    let mut reader = response.body_mut().as_reader();
    let part = to.with_extension("part");
    std::fs::create_dir_all(part.parent().expect("a file in a directory"))?;
    let mut out = std::fs::File::create(&part)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
    }
    out.flush()?;
    drop(out);
    let got = hex(&hasher.finalize());
    if got != dist.sha256 {
        let _ = std::fs::remove_file(&part);
        bail!(
            "{} does not match its published hash: expected {}, got {got}",
            dist.url,
            dist.sha256
        );
    }
    std::fs::rename(&part, to)?;
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The regular files of a ustar stream, written under `out`. A name
/// that would land outside `out` stops the whole thing: the archive
/// is checked against its hash first, but a reader of it should not
/// trust it to be well formed either.
fn untar(reader: &mut impl Read, out: &Path) -> Result<()> {
    let mut header = [0u8; 512];
    loop {
        if !read_block(reader, &mut header)? || header.iter().all(|&b| b == 0) {
            return Ok(());
        }
        let size = octal(&header[124..136]).context("a tar entry's size")?;
        let sum = octal(&header[148..156]).context("a tar entry's checksum")?;
        let computed: u64 = header
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    b as u64
                }
            })
            .sum();
        ensure!(sum == computed, "a tar entry's checksum does not match");
        let name = text(&header[0..100]);
        let prefix = text(&header[345..500]);
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let padded = size.div_ceil(512) * 512;
        let mut entry = reader.by_ref().take(padded);
        if matches!(header[156], b'0' | 0) {
            let to = out.join(inside(&path)?);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::create(&to)?;
            let copied = std::io::copy(&mut (&mut entry).take(size), &mut file)?;
            ensure!(copied == size, "{path} is cut short");
        }
        // The rest of the entry: its padding, or a whole entry of a
        // kind not written (a directory, a link, an extended header).
        std::io::copy(&mut entry, &mut std::io::sink())?;
    }
}

/// One 512-byte block, or false at a clean end of the stream.
fn read_block(reader: &mut impl Read, block: &mut [u8; 512]) -> Result<bool> {
    let mut got = 0;
    while got < block.len() {
        match reader.read(&mut block[got..])? {
            0 if got == 0 => return Ok(false),
            0 => bail!("the archive ends inside a tar header"),
            n => got += n,
        }
    }
    Ok(true)
}

fn octal(field: &[u8]) -> Option<u64> {
    let s = text(field);
    u64::from_str_radix(s.trim(), 8).ok()
}

fn text(field: &[u8]) -> &str {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    std::str::from_utf8(&field[..end]).unwrap_or("")
}

/// `path` as a relative path with no way out of the directory it is
/// joined to.
fn inside(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for c in Path::new(path).components() {
        match c {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            _ => bail!("the archive names {path}, outside where it unpacks"),
        }
    }
    ensure!(
        !out.as_os_str().is_empty(),
        "the archive names an empty path"
    );
    Ok(out)
}

/// The link of the library from the unpacked archive in `src` into
/// `out`, before it is run: what to run and with what, so the stamp
/// can say which recipe made what is there.
struct Recipe {
    /// `$CXX` or `c++`; or MSVC's `link.exe`, found when it is run.
    program: Option<OsString>,
    args: Vec<OsString>,
    /// The exports file the link reads, and its text.
    exports: (PathBuf, &'static str),
    library: PathBuf,
}

fn recipe(dist: &Dist, src: &Path, out: &Path, work: &Path) -> Result<Recipe> {
    let library = out.join(dist.library);
    let archive = src.join(dist.archive);
    Ok(if cfg!(target_os = "linux") {
        let script = work.join("exports.map");
        Recipe {
            program: Some(cxx()),
            args: linux_args(&archive, src, &library, &script),
            exports: (script, LINUX_EXPORTS),
            library,
        }
    } else if cfg!(target_os = "macos") {
        let floor = std::env::var("MACOSX_DEPLOYMENT_TARGET")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| MACOS_FLOOR.to_string());
        Recipe {
            program: Some(cxx()),
            args: macos_args(&archive, src, &library, &floor),
            // Read by nothing; the export is on the command line.
            exports: (work.join("exports.txt"), "_OrtGetApiBase\n"),
            library,
        }
    } else if cfg!(windows) {
        let def = work.join("onnxruntime.def");
        Recipe {
            program: None,
            args: windows_args(&archive, src, &library, &def, work),
            exports: (def, WINDOWS_EXPORTS),
            library,
        }
    } else {
        bail!("no recipe for linking ONNX Runtime on this platform");
    })
}

impl Recipe {
    /// What the stamp keeps of the recipe: a hash of the program, the
    /// arguments (the floor and every flag among them) and the
    /// exports, so an edit to any of them links again.
    fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        let program = self.program.clone().unwrap_or_else(|| "link.exe".into());
        for part in std::iter::once(&program).chain(&self.args) {
            hasher.update(part.as_encoded_bytes());
            hasher.update([0]);
        }
        hasher.update(self.exports.1.as_bytes());
        hex(&hasher.finalize())[..16].to_string()
    }

    fn run(&self) -> Result<()> {
        std::fs::write(&self.exports.0, self.exports.1)?;
        let mut command = match &self.program {
            Some(program) => Command::new(program),
            None => find_msvc_tools::find("x86_64-pc-windows-msvc", "link.exe").context(
                "no MSVC link.exe found; install the Visual Studio C++ build tools (the \
                 release.yml header names the toolset)",
            )?,
        };
        command.args(&self.args);
        println!("linking {}", self.library.display());
        let status = command
            .status()
            .with_context(|| format!("running {:?}", command.get_program()))?;
        ensure!(status.success(), "the link failed ({status})");
        ensure!(
            self.library.is_file(),
            "the link made no {}",
            self.library.display()
        );
        Ok(())
    }
}

/// `$CXX`, or the platform's own `c++`.
fn cxx() -> OsString {
    std::env::var_os("CXX")
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "c++".into())
}

const LINUX_EXPORTS: &str = "{\n  global: OrtGetApiBase;\n  local: *;\n};\n";

const WINDOWS_EXPORTS: &str = "LIBRARY onnxruntime\nEXPORTS\n    OrtGetApiBase\n";

fn os(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

fn linux_args(archive: &Path, src: &Path, library: &Path, exports: &Path) -> Vec<OsString> {
    let mut a = os(&["-shared", "-o"]);
    a.push(library.into());
    a.extend(os(&[
        "-Wl,-soname,libonnxruntime.so",
        "-Wl,-u,OrtGetApiBase",
    ]));
    a.push(archive.into());
    let mut search = OsString::from("-L");
    search.push(src);
    a.push(search);
    let mut version = OsString::from("-Wl,--version-script=");
    version.push(exports);
    a.push(version);
    a.extend(os(&[
        "-lwebgpu_dawn",
        "-Wl,-rpath,$ORIGIN",
        "-Wl,-z,defs",
        "-Wl,--gc-sections",
        "-Wl,--as-needed",
        "-s",
        "-lstdc++",
        "-lm",
        "-ldl",
        "-lpthread",
    ]));
    a
}

fn macos_args(archive: &Path, src: &Path, library: &Path, floor: &str) -> Vec<OsString> {
    let mut a = os(&["-dynamiclib", "-o"]);
    a.push(library.into());
    a.extend(os(&[
        "-install_name",
        "@rpath/libonnxruntime.dylib",
        "-Wl,-u,_OrtGetApiBase",
        "-Wl,-exported_symbol,_OrtGetApiBase",
    ]));
    a.push(format!("-mmacosx-version-min={floor}").into());
    a.push(archive.into());
    let mut search = OsString::from("-L");
    search.push(src);
    a.push(search);
    a.extend(os(&[
        "-lwebgpu_dawn",
        "-Wl,-rpath,@loader_path",
        "-Wl,-dead_strip",
        "-Wl,-S",
        "-Wl,-x",
        "-framework",
        "Foundation",
        "-framework",
        "CoreFoundation",
        "-framework",
        "CoreML",
        "-lobjc",
        "-lc++",
    ]));
    a
}

fn windows_args(
    archive: &Path,
    src: &Path,
    library: &Path,
    def: &Path,
    work: &Path,
) -> Vec<OsString> {
    let flag = |name: &str, path: &Path| {
        let mut f = OsString::from(name);
        f.push(path);
        f
    };
    let mut a = os(&["/NOLOGO", "/DLL", "/MACHINE:X64"]);
    a.push(flag("/OUT:", library));
    a.push(flag("/IMPLIB:", &work.join("onnxruntime.imp.lib")));
    a.push(flag("/DEF:", def));
    a.extend(os(&[
        "/INCLUDE:OrtGetApiBase",
        "/OPT:REF",
        "/OPT:ICF",
        "/INCREMENTAL:NO",
        "/DEBUG:NONE",
        "/Brepro",
    ]));
    a.push(archive.into());
    a.push(src.join("webgpu_dawn.lib").into());
    a.extend(os(&[
        "kernel32.lib",
        "advapi32.lib",
        "setupapi.lib",
        "dxgi.lib",
        "dbghelp.lib",
        "pathcch.lib",
    ]));
    a
}

/// `from` at `to`: a link where the platform makes one without
/// asking, a copy on Windows, where a symbolic link wants Developer
/// Mode. Whatever was at `to` goes, a link left dangling included.
fn put(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.symlink_metadata().is_ok() {
        std::fs::remove_file(to)?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to)
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(from, to).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ort-dylib-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A ustar header for a regular file (`kind` b'0') or anything else.
    fn header(name: &str, size: usize, kind: u8) -> [u8; 512] {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        let s = format!("{size:011o}");
        h[124..135].copy_from_slice(s.as_bytes());
        h[156] = kind;
        h[257..262].copy_from_slice(b"ustar");
        h[148..156].copy_from_slice(b"        ");
        let sum: u64 = h.iter().map(|&b| b as u64).sum();
        let s = format!("{sum:06o}\0 ");
        h[148..156].copy_from_slice(s.as_bytes());
        h
    }

    fn entry(tar: &mut Vec<u8>, name: &str, data: &[u8], kind: u8) {
        tar.extend(header(name, data.len(), kind));
        tar.extend(data);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }

    #[test]
    fn untar_writes_the_files_and_skips_the_rest() {
        let mut tar = Vec::new();
        entry(&mut tar, "lib", b"", b'5');
        entry(&mut tar, "libonnxruntime.a", &[7u8; 700], b'0');
        entry(&mut tar, "pax", b"12 path=x\n", b'x');
        entry(&mut tar, "sub/libwebgpu_dawn.so", b"dawn", b'0');
        tar.extend([0u8; 1024]);
        let dir = scratch("untar");
        untar(&mut tar.as_slice(), &dir).unwrap();
        assert_eq!(
            std::fs::read(dir.join("libonnxruntime.a")).unwrap(),
            vec![7u8; 700]
        );
        assert_eq!(
            std::fs::read(dir.join("sub/libwebgpu_dawn.so")).unwrap(),
            b"dawn"
        );
        assert!(!dir.join("lib").exists());
        assert!(!dir.join("pax").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untar_refuses_a_path_outside_and_a_bad_checksum() {
        let dir = scratch("untar-bad");
        let mut tar = Vec::new();
        entry(&mut tar, "../escape", b"x", b'0');
        assert!(untar(&mut tar.as_slice(), &dir).is_err());
        let mut tar = Vec::new();
        entry(&mut tar, "/etc/escape", b"x", b'0');
        assert!(untar(&mut tar.as_slice(), &dir).is_err());
        let mut tar = Vec::new();
        entry(&mut tar, "fine", b"x", b'0');
        tar[0] = b'g';
        assert!(untar(&mut tar.as_slice(), &dir).is_err());
        assert!(!dir.join("fine").exists() && !dir.join("gine").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cut_archive_is_an_error() {
        let dir = scratch("untar-cut");
        let mut tar = Vec::new();
        entry(&mut tar, "libonnxruntime.a", &[1u8; 2000], b'0');
        tar.truncate(1200);
        assert!(untar(&mut tar.as_slice(), &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_arguments_default_to_both_profiles() {
        let a = Args::from(["--target-dir", "/t"].into_iter().map(OsString::from)).unwrap();
        assert_eq!(a.target_dir, PathBuf::from("/t"));
        assert_eq!(a.profiles, ["debug", "release"]);
        let a = Args::from(["release"].into_iter().map(OsString::from)).unwrap();
        assert_eq!(a.profiles, ["release"]);
        assert!(Args::from(["../x"].into_iter().map(OsString::from)).is_err());
        assert!(Args::from(["--target-dir"].into_iter().map(OsString::from)).is_err());
    }

    #[test]
    fn a_profile_puts_the_library_where_binaries_tests_and_examples_sit() {
        let dirs = profile_dirs(Path::new("/t"), "debug");
        assert!(dirs.contains(&PathBuf::from("/t/debug")));
        assert!(dirs.contains(&PathBuf::from("/t/debug/deps")));
        assert!(dirs.contains(&PathBuf::from("/t/debug/examples")));
    }

    /// The one export, on every platform, under the name `ort` asks
    /// for; the archive pulled in from that root and never whole.
    #[test]
    fn every_recipe_exports_only_the_api_base() {
        let (a, s, l, x) = (
            Path::new("/s/a"),
            Path::new("/s"),
            Path::new("/o/l"),
            Path::new("/w/x"),
        );
        let linux: Vec<String> = linux_args(a, s, l, x)
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert!(linux.contains(&"-Wl,-u,OrtGetApiBase".into()));
        assert!(linux.contains(&"-Wl,--version-script=/w/x".into()));
        assert!(linux.contains(&"-Wl,-rpath,$ORIGIN".into()));
        assert!(linux.contains(&"-lstdc++".into()));
        assert!(
            LINUX_EXPORTS.contains("global: OrtGetApiBase;") && LINUX_EXPORTS.contains("local: *;")
        );

        let mac: Vec<String> = macos_args(a, s, l, "13.4")
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert!(mac.contains(&"-Wl,-exported_symbol,_OrtGetApiBase".into()));
        assert!(mac.contains(&"-Wl,-rpath,@loader_path".into()));
        assert!(mac.contains(&"@rpath/libonnxruntime.dylib".into()));
        assert!(mac.contains(&"-mmacosx-version-min=13.4".into()));

        let win: Vec<String> = windows_args(a, s, l, x, Path::new("/w"))
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert!(win.contains(&"/DLL".into()));
        assert!(win.contains(&"/DEF:/w/x".into()));
        assert!(win.contains(&"/INCLUDE:OrtGetApiBase".into()));
        assert!(WINDOWS_EXPORTS.lines().any(|l| l.trim() == "OrtGetApiBase"));

        for args in [&linux, &mac, &win] {
            assert!(
                !args.iter().any(|a| a.contains("whole-archive")
                    || a.contains("WHOLEARCHIVE")
                    || a.contains("force_load")),
                "{args:?}"
            );
        }
    }

    /// A changed flag, compiler or floor makes a different stamp, so
    /// the library is linked again; the same recipe, the same stamp.
    #[test]
    fn the_stamp_follows_the_recipe() {
        let r = |program: &str, args: &[&str]| Recipe {
            program: Some(program.into()),
            args: args.iter().map(OsString::from).collect(),
            exports: (PathBuf::from("/w/x"), LINUX_EXPORTS),
            library: PathBuf::from("/o/l"),
        };
        let a = r("c++", &["-shared", "-s"]);
        assert_eq!(a.fingerprint(), r("c++", &["-shared", "-s"]).fingerprint());
        assert_ne!(
            a.fingerprint(),
            r("clang++", &["-shared", "-s"]).fingerprint()
        );
        assert_ne!(a.fingerprint(), r("c++", &["-shared"]).fingerprint());
        assert_ne!(
            r("c++", &["-mmacosx-version-min=13.4"]).fingerprint(),
            r("c++", &["-mmacosx-version-min=14.0"]).fingerprint()
        );
    }

    #[test]
    fn put_replaces_whatever_was_there() {
        let dir = scratch("put");
        let from = dir.join("from");
        std::fs::write(&from, b"new").unwrap();
        let to = dir.join("to");
        std::fs::write(&to, b"old").unwrap();
        put(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
        // A link left dangling (a cleared cache) is replaced too.
        #[cfg(unix)]
        {
            let dangling = dir.join("dangling");
            std::os::unix::fs::symlink(dir.join("gone"), &dangling).unwrap();
            put(&from, &dangling).unwrap();
            assert_eq!(std::fs::read(&dangling).unwrap(), b"new");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
