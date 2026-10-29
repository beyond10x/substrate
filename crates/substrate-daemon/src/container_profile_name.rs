//! The host execution `AppArmor` profile's name, a deployment parameter (design 33).
//!
//! A name is a policy version's base name, or that base followed by `-` and a suffix of 1 to 40
//! characters from `[a-z0-9-]` that starts and ends with a letter or digit. The grammar admits no
//! character the policy parser, YAML or a shell treats specially, so a name substituted into the
//! policy cannot add a rule, and it can never select a profile outside these families.
//!
//! Each version is its own policy text (design 33 § 10). `v1` is the policy shipped since
//! `0.10.0`, kept byte for byte; `v2` is `v1` with `noatime` and `nodiratime` admitted on a
//! remount (D180). A suffix tells two loaded copies of one version apart; it never names another.

use std::fmt;
use std::str::FromStr;

/// The base name of the `v1` policy.
pub const V1: &str = "substrate-host-exec-v1";
/// The base name of the `v2` policy.
pub const V2: &str = "substrate-host-exec-v2";
/// The name every deployment uses unless it states another.
pub const DEFAULT: &str = V2;

/// A policy version: which committed policy text a name renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V1,
    V2,
}

impl Version {
    /// Every version, oldest first.
    pub const ALL: [Self; 2] = [Self::V1, Self::V2];

    /// The version's base name, the token its policy text names itself by.
    pub const fn base(self) -> &'static str {
        match self {
            Self::V1 => V1,
            Self::V2 => V2,
        }
    }
}

/// A validated profile name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileName {
    version: Version,
    name: String,
}

impl ProfileName {
    /// The label the kernel reports for a process this profile confines in enforce mode.
    pub fn enforced_label(&self) -> String {
        format!("{} (enforce)", self.name)
    }

    /// The policy version this name renders.
    #[allow(
        dead_code,
        reason = "the installer selects a template by it; the bootstrap compares only labels"
    )]
    pub fn version(&self) -> Version {
        self.version
    }
}

impl Default for ProfileName {
    fn default() -> Self {
        Self {
            version: Version::V2,
            name: DEFAULT.to_owned(),
        }
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name)
    }
}

impl FromStr for ProfileName {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let version = Version::ALL.into_iter().find(|version| {
            let base = version.base();
            name == base
                || name
                    .strip_prefix(base)
                    .and_then(|rest| rest.strip_prefix('-'))
                    .is_some_and(suffix)
        });
        match version {
            Some(version) => Ok(Self {
                version,
                name: name.to_owned(),
            }),
            None => Err(format!(
                "{name:?} is none of {V1}, {V2}, {V1}-<suffix> or {V2}-<suffix>, a suffix of 1 to \
                 {MAX_SUFFIX} characters from [a-z0-9-] that starts and ends with a letter or digit"
            )),
        }
    }
}

const MAX_SUFFIX: usize = 40;

fn suffix(value: &str) -> bool {
    let bytes = value.as_bytes();
    let edge = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    (1..=MAX_SUFFIX).contains(&bytes.len())
        && bytes.first().is_some_and(edge)
        && bytes.last().is_some_and(edge)
        && bytes.iter().all(|byte| edge(byte) || *byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_and_its_suffixed_family_are_names() {
        for (version, base) in [(Version::V1, V1), (Version::V2, V2)] {
            assert_eq!(version.base(), base);
            let longest = format!("{base}-{}", "a".repeat(40));
            let names = [
                base.to_owned(),
                format!("{base}-lane-01k8xyz"),
                format!("{base}-0"),
                format!("{base}-a--b"),
                longest.clone(),
            ];
            for name in &names {
                let parsed = name.parse::<ProfileName>();
                assert_eq!(
                    parsed.as_ref().map(ToString::to_string),
                    Ok(name.clone()),
                    "{name}"
                );
                assert_eq!(parsed.unwrap().version(), version, "{name}");
            }
            assert_eq!(longest.len(), 63);
        }
        assert_eq!(DEFAULT, V2);
        assert_eq!(ProfileName::default().to_string(), DEFAULT);
        assert_eq!(ProfileName::default().version(), Version::V2);
        assert_eq!(Version::ALL, [Version::V1, Version::V2]);
    }

    #[test]
    fn a_name_outside_the_family_or_carrying_policy_syntax_is_refused() {
        let too_long = format!("{DEFAULT}-{}", "a".repeat(41));
        let too_long_v1 = format!("{V1}-{}", "a".repeat(41));
        for name in [
            "",
            "unconfined",
            "docker-default",
            "substrate-host-exec-v0",
            "substrate-host-exec-v3",
            "substrate-host-exec-v10",
            "substrate-host-exec-v12",
            "substrate-host-exec-v",
            "substrate-host-exec-v2x",
            "substrate-host-exec-v2-",
            "substrate-host-exec-v2-A",
            too_long_v1.as_str(),
            "substrate-host-exec-v1x",
            "substrate-host-exec-v1-",
            "substrate-host-exec-v1--a",
            "substrate-host-exec-v1-a-",
            "substrate-host-exec-v1-Lane",
            "substrate-host-exec-v1-a b",
            "substrate-host-exec-v1-a\n",
            "substrate-host-exec-v1-a{",
            "substrate-host-exec-v1-a,",
            "substrate-host-exec-v1-a/b",
            " substrate-host-exec-v1",
            too_long.as_str(),
        ] {
            assert!(
                name.parse::<ProfileName>().is_err(),
                "{name:?} was admitted"
            );
        }
    }

    /// Adversary: the grammar is bytes, so no Unicode lookalike, case fold, control byte, path
    /// separator or length edge reaches the policy text or `localhostProfile`.
    #[test]
    fn adversary_lookalikes_controls_paths_and_length_edges_are_refused() {
        let suffix_39 = format!("{DEFAULT}-{}", "0".repeat(39));
        let suffix_40 = format!("{DEFAULT}-a{}z", "-".repeat(38));
        for name in [suffix_39.as_str(), suffix_40.as_str()] {
            assert_eq!(
                name.parse::<ProfileName>().map(|n| n.to_string()),
                Ok(name.to_owned())
            );
        }
        assert_eq!(suffix_40.len(), 63);
        let suffix_41 = format!("{DEFAULT}-a{}z", "-".repeat(39));
        let suffix_1000 = format!("{DEFAULT}-{}", "a".repeat(1000));
        for name in [
            suffix_41.as_str(),
            suffix_1000.as_str(),
            "substrate-host-exec-v1\n",
            "substrate-host-exec-v1\0",
            "substrate-host-exec-v1-a\0",
            "substrate-host-exec-v1-a\r",
            "substrate-host-exec-v1-a\t",
            "substrate-host-exec-v1-a\u{a0}",
            "substrate-host-exec-v1-a\u{200b}",
            "substrate-host-exec-v1 ",
            "substrate-host-exec-v1-\u{ff41}",
            "substrate-host-exec-v1-\u{e9}",
            "substrate-host-exec-v1-\u{131}",
            "substrate-host-exec-v1-K",
            "substrate-host-exec-v1-\u{212a}",
            "SUBSTRATE-HOST-EXEC-V1",
            "substrate-host-exec-v1\u{2010}a",
            "substrate-host-exec-v1-..",
            "substrate-host-exec-v1-a..b",
            "substrate-host-exec-v1-a.b",
            "substrate-host-exec-v1//a",
            "substrate-host-exec-v1-a//b",
            "substrate-host-exec-v1-a//&unconfined",
            "substrate-host-exec-v1-a:b",
            ":ns:substrate-host-exec-v1",
            "substrate-host-exec-v1-a*",
            "substrate-host-exec-v1-a?",
            "substrate-host-exec-v1-@{x}",
            "substrate-host-exec-v1-a#b",
            "substrate-host-exec-v1-a\"",
            "substrate-host-exec-v1-a (enforce)",
            "substrate-host-exec-v1=a",
            "substrate-host-exec-v1_a",
        ] {
            // Each lookalike is refused in both versions' spelling.
            for spelled in [name.to_owned(), name.replace(V1, V2).replace("-V1", "-V2")] {
                assert!(
                    spelled.parse::<ProfileName>().is_err(),
                    "{spelled:?} was admitted"
                );
            }
        }
    }

    #[test]
    fn the_enforced_label_is_the_name_in_enforce_mode() {
        assert_eq!(
            ProfileName::default().enforced_label(),
            "substrate-host-exec-v2 (enforce)"
        );
        assert_eq!(
            V1.parse::<ProfileName>().unwrap().enforced_label(),
            "substrate-host-exec-v1 (enforce)"
        );
        let lane: ProfileName = "substrate-host-exec-v1-lane-a".parse().unwrap();
        assert_eq!(
            lane.enforced_label(),
            "substrate-host-exec-v1-lane-a (enforce)"
        );
    }
}
