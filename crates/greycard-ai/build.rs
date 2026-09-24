// The prebuilt ONNX Runtime with WebGPU brings Dawn as a shared
// library, copied beside the binaries; the loader must look there. A
// test binary sits one directory below the library, in
// target/<profile>/deps, so both are named; an example sits in
// target/<profile>/examples. Windows looks beside the executable on
// its own, and MSVC's linker knows no -Wl.
fn main() {
    let origin = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => return,
        Ok("macos") => "@executable_path",
        _ => "$ORIGIN",
    };
    println!("cargo:rustc-link-arg-tests=-Wl,-rpath,{origin}/..");
    println!("cargo:rustc-link-arg-tests=-Wl,-rpath,{origin}");
    // An example sits in target/<profile>/examples, one below it too.
    println!("cargo:rustc-link-arg-examples=-Wl,-rpath,{origin}/..");
}
