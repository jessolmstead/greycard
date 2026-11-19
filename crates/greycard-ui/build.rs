use std::fmt::Write as _;
use std::path::Path;

// The catalogs' reader and the plural rules' parser, shared with the
// window's i18n module, which reads tables this writes.
#[allow(dead_code)]
#[path = "src/i18n/plural.rs"]
mod plural;
#[allow(dead_code)]
#[path = "src/i18n/po.rs"]
mod po;

fn main() {
    // No runpath: nothing the binary loads at exec sits beside it.
    // ONNX Runtime is opened by its full path at the first model, and
    // it finds Dawn through its own runpath (crates/ort-dylib).
    // The Slint UI is compiled in greycard-slint's build script; what
    // is left here reads nothing but itself, the catalogs, and the
    // icon, so it runs again only when one of them changes rather than
    // at every edit.
    println!("cargo:rerun-if-changed=build.rs");
    catalogs();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resource();
        windows_stack();
    }
}

/// The main thread's stack on Windows, 8 MB as Linux's and macOS's are.
/// Windows gives it 1 MB unless the binary says otherwise, and clap's
/// derived parser for our flags is one function whose frame, built
/// without optimizing, is past that: the editor died of a stack
/// overflow before reading a flag, on Windows alone (CI, 2026-11-18;
/// `ulimit -s 1024` shows it on Linux).
fn windows_stack() {
    let arg = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => "/STACK:8388608",
        _ => "-Wl,--stack,8388608",
    };
    println!("cargo:rustc-link-arg-bins={arg}");
}

/// Every `translations/<lang>/LC_MESSAGES/greycard.po` as a static
/// table in `OUT_DIR/catalogs.rs`: each language's tag, plural rule,
/// and translated messages (not fuzzy, no form empty), sorted by their
/// English. A catalog that will not read, or whose plural rule will
/// not, fails the build rather than the window.
fn catalogs() {
    let root = Path::new("../../translations");
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed=src/i18n/po.rs");
    println!("cargo:rerun-if-changed=src/i18n/plural.rs");
    let mut languages: Vec<_> = std::fs::read_dir(root)
        .expect("the translations folder reads")
        .map(|e| e.expect("a translations entry reads"))
        .filter(|e| e.path().join("LC_MESSAGES/greycard.po").is_file())
        .map(|e| {
            e.file_name()
                .into_string()
                .expect("a language's folder is named in UTF-8")
        })
        .collect();
    languages.sort();
    let mut out = String::from("static TABLES: &[Table] = &[\n");
    for tag in languages {
        let path = root.join(&tag).join("LC_MESSAGES/greycard.po");
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read_to_string(&path).expect("a catalog reads");
        let catalog = po::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let forms = catalog
            .field("Plural-Forms")
            .unwrap_or("nplurals=2; plural=(n != 1);");
        if let Err(e) = plural::forms(forms) {
            panic!("{}: Plural-Forms: {e}", path.display());
        }
        let mut entries: Vec<_> = catalog
            .entries
            .iter()
            .filter(|e| e.translated())
            .map(|e| (e.key(), &e.strs))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| a.0 == b.0);
        writeln!(
            out,
            "    Table {{ tag: {tag:?}, plural_forms: {forms:?}, entries: &["
        )
        .unwrap();
        for (key, strs) in entries {
            writeln!(out, "        ({key:?}, &{strs:?}),").unwrap();
        }
        out.push_str("    ] },\n");
    }
    out.push_str("];\n");
    let dest =
        Path::new(&std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("catalogs.rs");
    std::fs::write(dest, out).expect("the catalogs' tables are written");
}

// The icon and the version fields go into the executable's own resource
// table, which is where Explorer, the taskbar, the Start menu shortcut
// and the properties sheet read them; there is no .desktop entry to
// carry them on Windows. `winresource` is a build dependency only on a
// Windows host, since it drives the SDK's rc.exe, so this compiles the
// resource when the host and the target are both Windows and leaves it
// out of a cross build rather than failing one.
#[cfg(windows)]
fn windows_resource() {
    let icon = "../../assets/icon/greycard.ico";
    println!("cargo:rerun-if-changed={icon}");
    winresource::WindowsResource::new()
        .set_icon(icon)
        .set("ProductName", "greycard")
        .set("FileDescription", "greycard")
        .set("CompanyName", "Jess Olmstead")
        // The binary links Slint, under GPL-3.0-only, so the whole is
        // version 3; winresource writes the .rc as UTF-8 under
        // `#pragma code_page(65001)`, which keeps the ©.
        .set("LegalCopyright", "Copyright © Jess Olmstead. GPL-3.0.")
        .compile()
        .expect("rc.exe compiles the resource");
}

#[cfg(not(windows))]
fn windows_resource() {}
