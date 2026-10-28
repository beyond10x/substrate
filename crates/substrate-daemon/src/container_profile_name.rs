//! The host execution `AppArmor` profile's name, a deployment parameter (design 33).
//!
//! A name is the default or the default followed by `-` and a suffix of 1 to 40 characters from
//! `[a-z0-9-]` that starts and ends with a letter or digit. The grammar admits no character the
//! policy parser, YAML or a shell treats specially, so a name substituted into the policy cannot
//! add a rule, and it can never select a profile outside this family.

use std::fmt;
use std::str::FromStr;

/// The name every deployment uses unless it states another.
pub const DEFAULT: &str = "substrate-host-exec-v1";

/// A validated profile name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileName(String);

impl ProfileName {
    /// The label the kernel reports for a process this profile confines in enforce mode.
    pub fn enforced_label(&self) -> String {
        format!("{} (enforce)", self.0)
    }
}

impl Default for ProfileName {
    fn default() -> Self {
        Self(DEFAULT.to_owned())
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ProfileName {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let admitted = name == DEFAULT
            || name
                .strip_prefix(DEFAULT)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(suffix);
        if admitted {
            Ok(Self(name.to_owned()))
        } else {
            Err(format!(
                "{name:?} is neither {DEFAULT} nor {DEFAULT}-<suffix>, a suffix of 1 to \
                 {MAX_SUFFIX} characters from [a-z0-9-] that starts and ends with a letter or digit"
            ))
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
        let longest = format!("{DEFAULT}-{}", "a".repeat(40));
        for name in [
            DEFAULT,
            "substrate-host-exec-v1-lane-01k8xyz",
            "substrate-host-exec-v1-0",
            "substrate-host-exec-v1-a--b",
            longest.as_str(),
        ] {
            assert_eq!(
                name.parse::<ProfileName>().map(|name| name.to_string()),
                Ok(name.to_owned())
            );
        }
        assert_eq!(longest.len(), 63);
        assert_eq!(ProfileName::default().to_string(), DEFAULT);
    }

    #[test]
    fn a_name_outside_the_family_or_carrying_policy_syntax_is_refused() {
        let too_long = format!("{DEFAULT}-{}", "a".repeat(41));
        for name in [
            "",
            "unconfined",
            "docker-default",
            "substrate-host-exec-v2",
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

    #[test]
    fn the_enforced_label_is_the_name_in_enforce_mode() {
        assert_eq!(
            ProfileName::default().enforced_label(),
            "substrate-host-exec-v1 (enforce)"
        );
        let lane: ProfileName = "substrate-host-exec-v1-lane-a".parse().unwrap();
        assert_eq!(
            lane.enforced_label(),
            "substrate-host-exec-v1-lane-a (enforce)"
        );
    }
}
