//! Security review, second look (design 35 D28): since the run's variables are operands of the
//! helper, the helper's diagnostic line is a place a value could newly appear. It is written to
//! the run's standard error, which the driver records. No refusal may repeat a value.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

use substrate_exec_gate::REFUSED;

/// A value no diagnostic line may hold.
const VALUE: &str = "s2-value-51c9";

fn tool(name: &str) -> PathBuf {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {name} in /usr/bin or /bin"))
}

/// One refusal: what it is, the helper's options, and the helper's own environment.
type Refusal<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a str)]);

/// The helper given `options` between its `--program` and the program, with `own` as its own
/// environment.
fn gate(options: &[&str], own: &[(&str, &str)]) -> std::process::Output {
    let sh = tool("sh");
    let text = sh.to_str().expect("UTF-8");
    let (root, rel) = match text.strip_prefix("/usr/") {
        Some(rel) => ("/usr", rel),
        None => ("/bin", text.strip_prefix("/bin/").expect("a base tool")),
    };
    Command::new(env!("CARGO_BIN_EXE_substrate-exec-gate"))
        .args(["--program", "follow", root, rel])
        .args(options)
        .arg("--")
        .arg(&sh)
        .args(["-c", "echo ran"])
        .env_clear()
        .envs(own.iter().copied())
        .output()
        .expect("the gate starts")
}

/// **No refusal repeats a value** (D28): a variable named twice, a name the helper does not
/// take, and a variable in the helper's own environment each end the start with status 125,
/// nothing executed, and a line that names the variable and not what it holds.
#[test]
fn security2_no_refusal_of_a_variable_repeats_its_value() {
    let refusals: [Refusal<'_>; 5] = [
        (
            "a name given twice",
            &["--env", "A", VALUE, "--env", "A", VALUE],
            &[],
        ),
        ("a name with =", &["--env", "A=B", VALUE], &[]),
        ("an empty name", &["--env", "", VALUE], &[]),
        (
            "a variable of the helper's own beside an operand",
            &["--env", "A", VALUE],
            &[("A", VALUE)],
        ),
        ("a variable of the helper's own", &[], &[("TOKEN", VALUE)]),
    ];
    let count = refusals.len();
    for (what, options, own) in refusals {
        let output = gate(options, own);
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(REFUSED), "{what}: {output:?}");
        assert!(output.stdout.is_empty(), "{what}: the program ran");
        assert!(
            diagnostic.starts_with("substrate-exec-gate: ")
                && diagnostic.trim_end().lines().count() == 1,
            "{what}: one diagnostic line, got {diagnostic:?}"
        );
        assert!(
            !diagnostic.contains(VALUE),
            "{what}: the diagnostic line repeats the value: {diagnostic}"
        );
    }
    println!("security2 exec gate refusals: executed, {count} refusals");
}
