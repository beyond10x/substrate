//! Explicit immutable node-profile installation, independent of the application entrypoint.

#[path = "../container_profile_name.rs"]
mod profile_name;
#[path = "../container_profiles.rs"]
mod profiles;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
struct Args {
    /// Verify only; performs no profile installation or filesystem mutation.
    #[arg(long, conflicts_with = "hold")]
    check: bool,
    /// Remain available for deployment readiness checks until SIGINT/SIGTERM.
    #[arg(long)]
    hold: bool,
    /// The `AppArmor` profile name the policy is rendered, installed and checked under.
    #[arg(long, default_value = profile_name::DEFAULT)]
    apparmor_profile: profile_name::ProfileName,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let name = &args.apparmor_profile;
    if args.check {
        return profiles::check(name);
    }
    // Register before installation so PID 1 honors normal deployment termination throughout.
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    profiles::install(name)?;
    println!("versioned container execution profiles installed and enforced");
    if args.hold {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = terminate.recv() => break,
                result = tokio::signal::ctrl_c() => { result?; break; },
                _ = interval.tick() => profiles::check(name)?,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_defaults_and_a_name_outside_the_family_is_refused() {
        let args = Args::try_parse_from(["profiles", "--hold"]).unwrap();
        assert_eq!(args.apparmor_profile.to_string(), profile_name::DEFAULT);
        let args = Args::try_parse_from([
            "profiles",
            "--check",
            "--apparmor-profile",
            "substrate-host-exec-v1-lane-a",
        ])
        .unwrap();
        assert_eq!(
            args.apparmor_profile.to_string(),
            "substrate-host-exec-v1-lane-a"
        );
        assert!(Args::try_parse_from(["profiles", "--apparmor-profile", "unconfined"]).is_err());
    }

    /// Adversary: clap reads `--apparmor-profile=<name>` as the flag, the spelling the chart
    /// lane's word comparison does not count (`chart_adversary.rs`).
    #[test]
    fn the_profile_flag_is_read_in_its_one_word_spelling_too() {
        let args = Args::try_parse_from([
            "profiles",
            "--hold",
            "--apparmor-profile=substrate-host-exec-v1-lane-b",
        ])
        .unwrap();
        assert_eq!(
            args.apparmor_profile.to_string(),
            "substrate-host-exec-v1-lane-b"
        );
    }

    /// Adversary: a second flag, an abbreviation or an empty value never selects a profile.
    #[test]
    fn adversary_a_repeated_abbreviated_or_empty_profile_flag_is_refused() {
        for argv in [
            vec![
                "profiles",
                "--apparmor-profile",
                "substrate-host-exec-v1-lane-a",
                "--apparmor-profile",
                "substrate-host-exec-v1-lane-b",
            ],
            vec!["profiles", "--apparmor", "substrate-host-exec-v1-lane-a"],
            vec!["profiles", "--apparmor-profile="],
            vec!["profiles", "--check", "--apparmor-profile"],
        ] {
            assert!(Args::try_parse_from(&argv).is_err(), "{argv:?} admitted");
        }
    }
}
