// No runpath: nothing the binary loads at exec sits beside it. ONNX
// Runtime is opened by its full path at the first model, and it finds
// Dawn through its own runpath (crates/ort-dylib).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resource();
        windows_stack();
    }
}

// The main thread's stack on Windows, 8 MB as on Linux and macOS: see
// crates/greycard-ui/build.rs, whose flags overflowed Windows' 1 MB.
fn windows_stack() {
    let arg = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => "/STACK:8388608",
        _ => "-Wl,--stack,8388608",
    };
    println!("cargo:rustc-link-arg-bins={arg}");
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
        .set("CompanyName", "Jess Olmstead")
        // As the editor's: see crates/greycard-ui/build.rs.
        .set("LegalCopyright", "Copyright © Jess Olmstead. GPL-3.0.")
        .compile()
        .expect("rc.exe compiles the resource");
}

#[cfg(not(windows))]
fn windows_resource() {}
