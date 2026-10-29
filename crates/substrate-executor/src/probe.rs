//! The probe child (design 34 § 6.2): the program the capability probe's Pod runs under the
//! executor's `run`, which writes the argv it received as one JSON line. The driver renders the
//! argv with `$(HOME)`, `$$`, a space and a non-ASCII character in it, and holds the line it reads
//! back to the one [`report`] makes from the argv it sent, byte for byte: a shell or a variable
//! expansion anywhere between the Pod spec and `execve` changes it, and `exec.argv-only` stays
//! absent.

use serde::{Deserialize, Serialize};

/// The executor role the probe Pod's argv names after the executor's own path.
pub const PROBE_CHILD: &str = "probe-child";

/// The argv the child received, in order, `argv[0]` first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgvReport {
    pub argv: Vec<String>,
}

/// The exact bytes `probe-child` writes for `argv`: one JSON object and a newline.
///
/// # Panics
///
/// Never: a list of strings always serializes.
pub fn report<S: AsRef<str>>(argv: &[S]) -> String {
    let report = ArgvReport {
        argv: argv.iter().map(|part| part.as_ref().to_owned()).collect(),
    };
    let mut line = serde_json::to_string(&report).expect("an argv report serializes");
    line.push('\n');
    line
}

/// The arguments the probe Pod's child runs with after the executor path and [`PROBE_CHILD`]: a
/// Kubernetes variable reference, a shell parameter, a space and a non-ASCII character, each of
/// which a shell or the kubelet's expansion would change.
pub const PROBE_ARGUMENTS: [&str; 4] = ["$(HOME)", "$$", "a b", "\u{e9}"];

#[cfg(test)]
mod tests {
    use super::{ArgvReport, PROBE_ARGUMENTS, report};

    #[test]
    fn the_report_is_one_json_line_that_round_trips_every_argument() {
        let argv = ["/x", "probe-child", "$(HOME)", "$$", "a b", "\u{e9}"];
        let line = report(&argv);
        let body = line.strip_suffix('\n').expect("one newline at the end");
        assert!(!body.contains('\n'));
        let parsed: ArgvReport = serde_json::from_str(body).expect("the line parses");
        assert_eq!(parsed.argv, argv);
        assert_eq!(PROBE_ARGUMENTS, argv[2..]);
    }
}
