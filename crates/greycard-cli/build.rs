fn main() {
    // greycard-ai's ONNX Runtime brings Dawn as a shared library beside
    // the binary; the loader must look there. Windows looks beside the
    // executable on its own, and MSVC's linker knows no -Wl.
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => windows_resource(),
        Ok("macos") => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path"),
        _ => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN"),
    }
}

// The same resource the editor carries, so the two binaries sitting
// beside each other in bin\ do not show one icon and one blank page.
// See crates/greycard-ui/build.rs for why it is gated on the host.
#[cfg(windows)]
fn windows_resource() {
    let icon = "../../assets/icon/greycard.ico";
    println!("cargo:rerun-if-changed={icon}");
    winresource::WindowsResource::new()
        .set_icon(icon)
        .set("ProductName", "greycard")
        .set("FileDescription", "greycard command line")
        .set("LegalCopyright", "GPL-3.0-or-later")
        .compile()
        .expect("rc.exe compiles the resource");
}

#[cfg(not(windows))]
fn windows_resource() {}
