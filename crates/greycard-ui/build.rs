fn main() {
    // greycard-ai's ONNX Runtime brings Dawn as a shared library beside
    // the binary; the loader must look there. Windows looks beside the
    // executable on its own, and MSVC's linker knows no -Wl.
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => {}
        Ok("macos") => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path"),
        _ => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN"),
    }
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("the Slint UI compiles");
}
