fn main() {
    // greycard-ai's ONNX Runtime brings Dawn as a shared library beside
    // the binary; the loader must look there. Windows looks beside the
    // executable on its own, and MSVC's linker knows no -Wl.
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => windows_resource(),
        Ok("macos") => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path"),
        _ => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN"),
    }
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("the Slint UI compiles");
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
