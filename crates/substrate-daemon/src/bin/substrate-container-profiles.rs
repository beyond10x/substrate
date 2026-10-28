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
}
