//! Explicit PID-1 bootstrap for the container-private host execution profile.

#[path = "../container_bootstrap.rs"]
mod bootstrap;
#[path = "../container_profile_name.rs"]
mod profile_name;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser)]
#[command(
    about = "Prepare a private container delegation and start the non-root daemon",
    version
)]
struct Args {
    /// Select the image's existing `SYS_ADMIN` file-capability quota executable.
    #[arg(long)]
    project_quotas: bool,
    /// The `AppArmor` profile this container must run under, enforced.
    #[arg(long, default_value = profile_name::DEFAULT)]
    apparmor_profile: profile_name::ProfileName,
    /// Normal daemon arguments. The cgroup root is fixed by this entrypoint.
    #[arg(last = true, required = true)]
    daemon_arguments: Vec<OsString>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match bootstrap::run(
        args.project_quotas,
        &args.apparmor_profile,
        args.daemon_arguments,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("container execution bootstrap refused: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_defaults_and_a_stated_one_never_reaches_the_daemon() {
        let args = Args::try_parse_from(["exec", "--", "--socket", "s"]).unwrap();
        assert_eq!(args.apparmor_profile.to_string(), profile_name::DEFAULT);
        let args = Args::try_parse_from([
            "exec",
            "--project-quotas",
            "--apparmor-profile",
            "substrate-host-exec-v1-lane-a",
            "--",
            "--socket",
            "s",
        ])
        .unwrap();
        assert_eq!(
            args.apparmor_profile.to_string(),
            "substrate-host-exec-v1-lane-a"
        );
        assert_eq!(args.daemon_arguments, ["--socket", "s"]);
        assert!(
            Args::try_parse_from(["exec", "--apparmor-profile", "unconfined", "--", "x"]).is_err()
        );
    }

    /// Adversary: clap reads `--apparmor-profile=<name>` as the flag, the spelling the chart
    /// lane's word comparison does not count (`chart_adversary.rs`).
    #[test]
    fn the_profile_flag_is_read_in_its_one_word_spelling_too() {
        let args = Args::try_parse_from([
            "exec",
            "--apparmor-profile=substrate-host-exec-v1-lane-b",
            "--",
            "--socket",
            "s",
        ])
        .unwrap();
        assert_eq!(
            args.apparmor_profile.to_string(),
            "substrate-host-exec-v1-lane-b"
        );
    }

    /// Adversary: a second flag, an abbreviation, an empty value or the flag after `--` never
    /// selects a profile; after `--` it is the daemon's own argument.
    #[test]
    fn adversary_a_repeated_abbreviated_or_empty_profile_flag_is_refused() {
        for argv in [
            vec![
                "exec",
                "--apparmor-profile",
                "substrate-host-exec-v1-lane-a",
                "--apparmor-profile",
                "substrate-host-exec-v1-lane-b",
                "--",
                "x",
            ],
            vec![
                "exec",
                "--apparmor",
                "substrate-host-exec-v1-lane-a",
                "--",
                "x",
            ],
            vec!["exec", "--apparmor-profile=", "--", "x"],
            vec!["exec", "--apparmor-profile", "--", "x"],
        ] {
            assert!(Args::try_parse_from(&argv).is_err(), "{argv:?} admitted");
        }
        let args =
            Args::try_parse_from(["exec", "--", "--apparmor-profile", "unconfined"]).unwrap();
        assert_eq!(args.apparmor_profile.to_string(), profile_name::DEFAULT);
        assert_eq!(args.daemon_arguments, ["--apparmor-profile", "unconfined"]);
    }
}
