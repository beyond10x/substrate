//! Deployment-owned node prerequisite; never called by a daemon or workspace request.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, ensure};

use crate::profile_name::{ProfileName, Version};

const ROOT: &str = "/node-profiles";
const SECCOMP: &str = "host-exec-v1-amd64.json";
/// The `v1` policy source and its template, as shipped since `0.10.0`: its base name is the token
/// (design 33 § 3).
const POLICY_V1: &[u8] = include_bytes!("../../../deploy/apparmor/substrate-host-execution");
/// The `v2` policy source and its template: `v1` with `noatime` and `nodiratime` admitted on a
/// remount (D180).
const POLICY_V2: &[u8] = include_bytes!("../../../deploy/apparmor/substrate-host-execution-v2");
const SYSCALLS: &[u8] = include_bytes!("../../../deploy/seccomp/host-exec-v1-amd64.json");

/// The committed policy text a version renders from.
const fn template(version: Version) -> &'static [u8] {
    match version {
        Version::V1 => POLICY_V1,
        Version::V2 => POLICY_V2,
    }
}

/// The policy for `name`: its version's compiled-in source with every occurrence of that
/// version's base name, its declaration and its two self-peers, replaced. For a base name it is
/// the source itself.
fn render(name: &ProfileName) -> Vec<u8> {
    let policy = template(name.version());
    let token = name.version().base();
    let name = name.to_string();
    let mut rendered = Vec::with_capacity(policy.len() + 3 * name.len());
    let mut rest = policy;
    while let Some(at) = rest
        .windows(token.len())
        .position(|window| window == token.as_bytes())
    {
        rendered.extend_from_slice(&rest[..at]);
        rendered.extend_from_slice(name.as_bytes());
        rest = &rest[at + token.len()..];
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
#[path = "container_profile_remount.rs"]
mod remount_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_name::{DEFAULT, V1, V2};

    const LANE: &str = "substrate-host-exec-v1-lane-01k8xyz";
    const LANE_V2: &str = "substrate-host-exec-v2-lane-01k8xyz";

    fn lane() -> ProfileName {
        LANE.parse().unwrap()
    }

    fn v1() -> ProfileName {
        V1.parse().unwrap()
    }

    fn text(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }

    #[test]
    fn the_template_names_the_profile_only_in_its_declaration_and_its_two_self_peers() {
        for version in Version::ALL {
            let token = version.base();
            let source = text(template(version));
            assert_eq!(source.matches(token).count(), 3, "{source}");
            let naming: Vec<&str> = source
                .lines()
                .filter(|line| line.contains(token))
                .map(str::trim)
                .collect();
            assert_eq!(
                naming,
                [
                    format!("profile {token} flags=(attach_disconnected,mediate_deleted) {{"),
                    format!("signal (send,receive) peer={token},"),
                    format!("ptrace (trace,tracedby,read,readby) peer={token},"),
                ]
            );
            // No version names another as its peer or carries another's declaration.
            assert_eq!(
                source.matches("substrate-host-exec-v").count(),
                3,
                "{token}"
            );
        }
    }

    #[test]
    fn the_default_renders_the_committed_source_and_its_file_byte_for_byte() {
        let default = ProfileName::default();
        assert_eq!(default.version(), Version::V2);
        assert_eq!(render(&default), POLICY_V2);
        assert_eq!(source_file(&default), "substrate-host-exec-v2.apparmor");
        assert_eq!(render(&v1()), POLICY_V1);
        assert_eq!(source_file(&v1()), "substrate-host-exec-v1.apparmor");
    }

    #[test]
    fn a_named_profile_renders_the_same_rules_under_its_own_name_and_file() {
        for (lane, base, policy) in [(LANE, V1, POLICY_V1), (LANE_V2, V2, POLICY_V2)] {
            let profile: ProfileName = lane.parse().unwrap();
            let rendered = render(&profile);
            let expected = text(policy)
                .replace(
                    &format!("profile {base} flags="),
                    &format!("profile {lane} flags="),
                )
                .replace(&format!("peer={base},"), &format!("peer={lane},"));
            assert_eq!(text(&rendered), expected);
            assert_ne!(rendered, policy);
            assert_eq!(text(&rendered).matches("profile ").count(), 1);
            assert_eq!(text(&rendered).matches(lane).count(), 3);
            assert_eq!(text(&rendered).replace(lane, base).as_bytes(), policy);
            assert_eq!(source_file(&profile), format!("{lane}.apparmor"));
        }
    }

    #[test]
    fn a_source_rendered_for_another_name_is_refused() {
        let default = render(&ProfileName::default());
        let v1 = render(&v1());
        let named = render(&lane());
        let named_v2 = render(&LANE_V2.parse().unwrap());
        let other = render(&"substrate-host-exec-v1-lane-other".parse().unwrap());
        for bytes in [&default, &v1, &named, &named_v2] {
            assert!(same_bytes(bytes, bytes).is_ok());
        }
        for (actual, expected) in [
            (&default, &named),
            (&named, &default),
            (&other, &named),
            (&v1, &default),
            (&default, &v1),
            (&named, &named_v2),
            (&named_v2, &named),
        ] {
            assert!(same_bytes(actual, expected).is_err());
        }
    }

    /// Adversary: each version's base rendering is the reviewed policy every container-runtime
    /// review result records by hash (design 33 § 1, § 8), not merely whatever the file holds
    /// today. `v1` is the policy shipped since `0.10.0`.
    #[test]
    fn adversary_the_default_rendering_is_the_reviewed_policy_by_hash() {
        use sha2::Digest as _;
        assert_eq!(
            hex::encode(sha2::Sha256::digest(render(&v1()))),
            "e3ff9b10eb633585354cedbcbe1dafb4ec01c4296ea1085677221adca3b6843a"
        );
        assert_eq!(
            hex::encode(sha2::Sha256::digest(render(&ProfileName::default()))),
            "6ac530bcbff411ef0a765d2f81db777ea073e0bb57374b92cac2b068d373e968"
        );
    }

    /// Adversary, a property with a fixed seed: every name the grammar admits renders its
    /// version's policy with exactly three self-references and nothing else changed, and no
    /// non-base rendering keeps a declaration or a peer naming the base profile.
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
        for version in Version::ALL {
            let base = version.base();
            let policy = template(version);
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
                let name = format!("{base}-{}", String::from_utf8(suffix).unwrap());
                let Ok(profile) = name.parse::<ProfileName>() else {
                    continue;
                };
                admitted += 1;
                assert_eq!(profile.version(), version, "{name}");
                let rendered = String::from_utf8(render(&profile)).unwrap();
                assert_eq!(rendered.matches(name.as_str()).count(), 3, "{name}");
                assert_eq!(
                    rendered.len(),
                    policy.len() + 3 * (name.len() - base.len()),
                    "{name}"
                );
                assert_eq!(
                    rendered.replace(name.as_str(), base).as_bytes(),
                    policy,
                    "{name}"
                );
                for stale in [format!("profile {base} "), format!("peer={base},")] {
                    assert!(!rendered.contains(&stale), "{name} keeps {stale:?}");
                }
                assert_eq!(rendered.matches("\nprofile ").count(), 1, "{name}");
                assert_eq!(source_file(&profile), format!("{name}.apparmor"));
            }
            assert!(
                admitted > 200,
                "the generator admitted only {admitted} {base} names"
            );
        }
    }

    /// Adversary: a registry line for a child, a stack, another policy namespace, a longer
    /// family name or the other version is neither the profile loaded nor the profile enforced.
    #[test]
    fn adversary_a_registry_line_for_another_profile_never_counts() {
        let default = ProfileName::default();
        let v1 = v1();
        let lane = lane();
        for profile in [&default, &v1, &lane] {
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
        assert_eq!(default.to_string(), DEFAULT);
        let registry = "substrate-host-exec-v1 (enforce)\ncri-containerd.apparmor.d (enforce)\n";
        assert!(loaded(registry, &v1()) && enforced(registry, &v1()));
        assert!(!loaded(registry, &lane()) && !enforced(registry, &lane()));
        // A node holding only v1, as every node did up to 0.14.0, does not hold the default.
        assert!(!loaded(registry, &default) && !enforced(registry, &default));

        let registry = "substrate-host-exec-v2 (enforce)\n";
        assert!(loaded(registry, &default) && enforced(registry, &default));
        assert!(!loaded(registry, &v1()) && !enforced(registry, &v1()));

        let registry = format!("{LANE} (enforce)\n");
        assert!(loaded(&registry, &lane()) && enforced(&registry, &lane()));
        assert!(!loaded(&registry, &default) && !enforced(&registry, &default));

        let registry = format!("{LANE} (complain)\n");
        assert!(loaded(&registry, &lane()) && !enforced(&registry, &lane()));
    }
}
