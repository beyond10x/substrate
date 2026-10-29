//! Deployment-owned node prerequisite; never called by a daemon or workspace request.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, ensure};

use crate::profile_name::{DEFAULT, ProfileName};

const ROOT: &str = "/node-profiles";
const SECCOMP: &str = "host-exec-v1-amd64.json";
/// The one policy source and its template: the default name is the token (design 33 § 3).
const POLICY: &[u8] = include_bytes!("../../../deploy/apparmor/substrate-host-execution");
const SYSCALLS: &[u8] = include_bytes!("../../../deploy/seccomp/host-exec-v1-amd64.json");

/// The policy for `name`: the compiled-in source with every occurrence of the default name, its
/// declaration and its two self-peers, replaced. For the default it is the source itself.
fn render(name: &ProfileName) -> Vec<u8> {
    let name = name.to_string();
    let mut rendered = Vec::with_capacity(POLICY.len() + 3 * name.len());
    let mut rest = POLICY;
    while let Some(at) = rest
        .windows(DEFAULT.len())
        .position(|window| window == DEFAULT.as_bytes())
    {
        rendered.extend_from_slice(&rest[..at]);
        rendered.extend_from_slice(name.as_bytes());
        rest = &rest[at + DEFAULT.len()..];
    }
    rendered.extend_from_slice(rest);
    rendered
}

/// The retained source file the parser loads, beneath the node-profile directory.
fn source_file(name: &ProfileName) -> String {
    format!("{name}.apparmor")
}

/// Whether the kernel's profile registry holds `name` in any mode.
fn loaded(registry: &str, name: &ProfileName) -> bool {
    let prefix = format!("{name} (");
    registry.lines().any(|line| line.starts_with(&prefix))
}

/// Whether the kernel's profile registry holds `name` in enforce mode.
fn enforced(registry: &str, name: &ProfileName) -> bool {
    let label = name.enforced_label();
    registry.lines().any(|line| line == label)
}

pub fn check(name: &ProfileName) -> Result<()> {
    validate_root()?;
    verify_file(&Path::new(ROOT).join(source_file(name)), &render(name))?;
    verify_file(&Path::new(ROOT).join(SECCOMP), SYSCALLS)?;
    ensure!(
        enforced(&profile_lines()?, name),
        "the versioned AppArmor profile is not enforced"
    );
    Ok(())
}

pub fn install(name: &ProfileName) -> Result<()> {
    validate_root()?;
    let root = Path::new(ROOT);
    let policy = root.join(source_file(name));
    let bytes = render(name);
    if loaded(&profile_lines()?, name) {
        verify_file(&policy, &bytes)
            .context("refuse a loaded profile without its exact protected source")?;
    }
    install_file(&policy, &bytes)?;
    install_file(&root.join(SECCOMP), SYSCALLS)?;
    let status = Command::new("/sbin/apparmor_parser")
        .env_clear()
        .args(["--replace", "--skip-read-cache"])
        .arg(&policy)
        .status()
        .context("load the compiled-in AppArmor policy")?;
    ensure!(status.success(), "AppArmor profile installation failed");
    check(name)
}

fn profile_lines() -> Result<String> {
    fs::read_to_string("/sys/kernel/security/apparmor/profiles")
        .context("read the node AppArmor profile registry")
}

fn validate_root() -> Result<()> {
    let metadata = fs::symlink_metadata(ROOT)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
        "the dedicated node profile directory must be protected and root-owned"
    );
    Ok(())
}

fn verify_file(path: &Path, expected: &[u8]) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o6022 == 0,
        "node profile file must be protected, regular and root-owned"
    );
    ensure!(
        metadata.len() == expected.len() as u64,
        "node profile length differs"
    );
    same_bytes(&fs::read(path)?, expected)
}

fn same_bytes(actual: &[u8], expected: &[u8]) -> Result<()> {
    ensure!(actual == expected, "versioned node profile bytes differ");
    Ok(())
}

fn install_file(path: &Path, bytes: &[u8]) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => return verify_file(path, bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path.parent().context("profile parent")?;
    let temporary = parent.join(format!(".pending-{}", ulid::Ulid::generate()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        // A same-directory hard link publishes atomically and never overwrites an existing name.
        match fs::hard_link(&temporary, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        fs::File::open(parent)?.sync_all()?;
        verify_file(path, bytes)
    })();
    let cleanup = fs::remove_file(temporary);
    result?;
    cleanup?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANE: &str = "substrate-host-exec-v1-lane-01k8xyz";

    fn lane() -> ProfileName {
        LANE.parse().unwrap()
    }

    fn text(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }

    #[test]
    fn the_template_names_the_profile_only_in_its_declaration_and_its_two_self_peers() {
        let source = text(POLICY);
        assert_eq!(source.matches(DEFAULT).count(), 3, "{source}");
        let naming: Vec<&str> = source
            .lines()
            .filter(|line| line.contains(DEFAULT))
            .map(str::trim)
            .collect();
        assert_eq!(
            naming,
            [
                "profile substrate-host-exec-v1 flags=(attach_disconnected,mediate_deleted) {",
                "signal (send,receive) peer=substrate-host-exec-v1,",
                "ptrace (trace,tracedby,read,readby) peer=substrate-host-exec-v1,",
            ]
        );
    }

    #[test]
    fn the_default_renders_the_committed_source_and_its_file_byte_for_byte() {
        let default = ProfileName::default();
        assert_eq!(render(&default), POLICY);
        assert_eq!(source_file(&default), "substrate-host-exec-v1.apparmor");
    }

    #[test]
    fn a_named_profile_renders_the_same_rules_under_its_own_name_and_file() {
        let rendered = render(&lane());
        let expected = text(POLICY)
            .replace(
                "profile substrate-host-exec-v1 flags=",
                &format!("profile {LANE} flags="),
            )
            .replace("peer=substrate-host-exec-v1,", &format!("peer={LANE},"));
        assert_eq!(text(&rendered), expected);
        assert_ne!(rendered, POLICY);
        assert_eq!(text(&rendered).matches("profile ").count(), 1);
        assert_eq!(text(&rendered).matches(LANE).count(), 3);
        assert_eq!(text(&rendered).replace(LANE, DEFAULT).as_bytes(), POLICY);
        assert_eq!(source_file(&lane()), format!("{LANE}.apparmor"));
    }

    #[test]
    fn a_source_rendered_for_another_name_is_refused() {
        let default = render(&ProfileName::default());
        let named = render(&lane());
        let other = render(&"substrate-host-exec-v1-lane-other".parse().unwrap());
        assert!(same_bytes(&named, &named).is_ok());
        assert!(same_bytes(&default, &default).is_ok());
        for (actual, expected) in [(&default, &named), (&named, &default), (&other, &named)] {
            assert!(same_bytes(actual, expected).is_err());
        }
    }

    /// Adversary: the default rendering is the reviewed policy every container-runtime review
    /// result records by hash (design 33 § 1, § 8), not merely whatever the file holds today.
    #[test]
    fn adversary_the_default_rendering_is_the_reviewed_policy_by_hash() {
        use sha2::Digest as _;
        assert_eq!(
            hex::encode(sha2::Sha256::digest(render(&ProfileName::default()))),
            "e3ff9b10eb633585354cedbcbe1dafb4ec01c4296ea1085677221adca3b6843a"
        );
    }

    /// Adversary, a property with a fixed seed: every name the grammar admits renders the policy
    /// with exactly three self-references and nothing else changed, and no non-default rendering
    /// keeps a declaration or a peer naming the default profile.
    #[test]
    fn adversary_every_admitted_name_changes_exactly_the_three_sites() {
        const ALPHABET: &[u8] = b"az09-----";
        const NOISE: &[u8] = b"A_.,/ {}\n";
        let mut state: u64 = 0x5eed_0033;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            state >> 33
        };
        let mut admitted = 0;
        for _ in 0..4000 {
            let length = 1 + usize::try_from(next() % 44).unwrap();
            let mut suffix = Vec::with_capacity(length);
            for _ in 0..length {
                let pick = usize::try_from(next()).unwrap();
                suffix.push(if pick % 50 == 0 {
                    NOISE[pick % NOISE.len()]
                } else {
                    ALPHABET[pick % ALPHABET.len()]
                });
            }
            let name = format!("{DEFAULT}-{}", String::from_utf8(suffix).unwrap());
            let Ok(profile) = name.parse::<ProfileName>() else {
                continue;
            };
            admitted += 1;
            let rendered = String::from_utf8(render(&profile)).unwrap();
            assert_eq!(rendered.matches(name.as_str()).count(), 3, "{name}");
            assert_eq!(
                rendered.len(),
                POLICY.len() + 3 * (name.len() - DEFAULT.len()),
                "{name}"
            );
            assert_eq!(
                rendered.replace(name.as_str(), DEFAULT).as_bytes(),
                POLICY,
                "{name}"
            );
            for stale in [
                "profile substrate-host-exec-v1 ",
                "peer=substrate-host-exec-v1,",
            ] {
                assert!(!rendered.contains(stale), "{name} keeps {stale:?}");
            }
            assert_eq!(rendered.matches("\nprofile ").count(), 1, "{name}");
            assert_eq!(source_file(&profile), format!("{name}.apparmor"));
        }
        assert!(
            admitted > 200,
            "the generator admitted only {admitted} names"
        );
    }

    /// Adversary: a registry line for a child, a stack, another policy namespace or a longer
    /// family name is neither the profile loaded nor the profile enforced.
    #[test]
    fn adversary_a_registry_line_for_another_profile_never_counts() {
        let default = ProfileName::default();
        let lane = lane();
        for profile in [&default, &lane] {
            let label = profile.enforced_label();
            let registry = [
                format!("docker-default//{label}"),
                format!("docker-default//&{label}"),
                format!(":other:{label}"),
                format!(":other://{label}"),
                format!("{profile}-x (enforce)"),
                format!("{profile}x (enforce)"),
                format!(" {label}"),
                format!("{profile} (kill)"),
            ]
            .join("\n");
            assert!(
                !enforced(&registry, profile),
                "{profile} counted enforced in {registry}"
            );
            let others: Vec<&str> = registry
                .lines()
                .filter(|line| !line.ends_with(" (kill)"))
                .collect();
            assert!(
                !loaded(&others.join("\n"), profile),
                "{profile} counted loaded in {others:?}"
            );
            assert!(loaded(&registry, profile), "{profile} (kill) is loaded");
        }
    }

    #[test]
    fn loaded_and_enforced_match_the_exact_name_only() {
        let default = ProfileName::default();
        let registry = "substrate-host-exec-v1 (enforce)\ncri-containerd.apparmor.d (enforce)\n";
        assert!(loaded(registry, &default) && enforced(registry, &default));
        assert!(!loaded(registry, &lane()) && !enforced(registry, &lane()));

        let registry = format!("{LANE} (enforce)\n");
        assert!(loaded(&registry, &lane()) && enforced(&registry, &lane()));
        assert!(!loaded(&registry, &default) && !enforced(&registry, &default));

        let registry = format!("{LANE} (complain)\n");
        assert!(loaded(&registry, &lane()) && !enforced(&registry, &lane()));
    }
}
