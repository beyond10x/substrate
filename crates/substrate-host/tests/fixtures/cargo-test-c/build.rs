// Compiles `csrc/answer.c` into a static library the way the `cc` crate does by default for a
// macOS host build: the compiler `CC` names, else `cc` on `PATH`, and the archiver `AR` names,
// else `ar`. The fixture carries no `cc` crate so that the confined build stays offline and
// vendors nothing; the commands it runs are the ones that crate runs.
use std::path::Path;
use std::process::Command;

fn run(mut command: Command) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?}: {}; stdout {:?}; stderr {:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn main() {
    let out = std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR");
    let out = Path::new(&out);
    let object = out.join("answer.o");
    let mut compile = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()));
    compile.args(["-c", "csrc/answer.c", "-o"]).arg(&object);
    run(compile);
    let mut archive = Command::new(std::env::var_os("AR").unwrap_or_else(|| "ar".into()));
    archive.arg("crs").arg(out.join("libanswer.a")).arg(&object);
    run(archive);
    println!("cargo::rustc-link-search=native={}", out.display());
    println!("cargo::rustc-link-lib=static=answer");
    println!("cargo::rerun-if-changed=csrc/answer.c");
    println!("cargo::rerun-if-changed=build.rs");
}
