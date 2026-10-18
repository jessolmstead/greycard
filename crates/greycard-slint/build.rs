fn main() {
    // A debug build, which is what the tests run, carries the element
    // names the testing backend finds controls by (a slider's value
    // field, the curve's In and Out); a release build goes without.
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("the Slint UI compiles");
}
