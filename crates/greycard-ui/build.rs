fn main() {
    // No runpath: nothing the binary loads at exec sits beside it.
    // ONNX Runtime is opened by its full path at the first model, and
    // it finds Dawn through its own runpath (crates/ort-dylib).
    // The Slint UI is compiled in greycard-slint's build script; what
    // is left here reads nothing but itself and the icon, so it runs
    // again only when one of them changes rather than at every edit.
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resource();
    }
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
        .set("LegalCopyright", "GPL-3.0-or-later")
        .compile()
        .expect("rc.exe compiles the resource");
}

#[cfg(not(windows))]
fn windows_resource() {}
