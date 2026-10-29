//! The one-line release records a workstation lane prints: `<Trailer>: <sha> <count> <instant>`.
//!
//! `release.yml` reads each from the annotated tag and refuses one that does not name the tagged
//! commit, the count the tagged source declares, or an RFC 3339 UTC instant.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, ensure};

/// `git -C <root> <args>`, its trimmed standard output.
pub fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("git")?;
    ensure!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// `<trailer>: <sha> <count> <RFC 3339 UTC>`.
pub fn record_line(trailer: &str, sha: &str, count: usize, at: SystemTime) -> String {
    format!("{trailer}: {sha} {count} {}", rfc3339_utc(at))
}

/// `YYYY-MM-DDTHH:MM:SSZ`, the form `release.yml` accepts.
pub fn rfc3339_utc(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let clock = seconds % 86_400;
    // Howard Hinnant's days-from-civil inverse, valid for every date after the epoch.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        clock / 3_600,
        clock % 3_600 / 60,
        clock % 60
    )
}
