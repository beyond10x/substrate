// Writes a constant the library includes, so a test passing shows the build script ran.
fn main() {
    let out = std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR");
    let generated = std::path::Path::new(&out).join("generated.rs");
    std::fs::write(
        generated,
        "pub const FROM_BUILD_SCRIPT: &str = \"generated\";\n",
    )
    .expect("the build script writes OUT_DIR");
    println!("cargo::rerun-if-changed=build.rs");
}
