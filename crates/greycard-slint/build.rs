fn main() {
    // A debug build, which is what the tests run, carries the element
    // names the testing backend finds controls by (a slider's value
    // field, the curve's In and Out); a release build goes without.
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    // The Slint compiler recurses over each expression, and the
    // window's key handler is deep enough to overflow the 1 MB a
    // build script's main thread has on Windows (Linux gives it 8).
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let config = slint_build::CompilerConfiguration::new()
                .with_style("fluent-dark".into())
                .with_debug_info(debug);
            slint_build::compile_with_config("ui/app.slint", config).expect("the Slint UI compiles")
        })
        .expect("a thread for the Slint compiler")
        .join()
        .expect("the Slint compiler's thread");
}
