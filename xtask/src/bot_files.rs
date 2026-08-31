use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use clap::Args as ClapArgs;

use crate::report::Report;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    key: PathBuf,
}

pub fn check(args: &Args) -> Report {
    let mut failures = Vec::new();
    inspect(&args.config, false, &mut failures);
    inspect(&args.key, true, &mut failures);
    if failures.is_empty() {
        Report::passed("bot credential inputs have safe Unix metadata")
    } else {
        Report::failed(failures)
    }
}

fn inspect(path: &Path, private: bool, failures: &mut Vec<String>) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        failures.push("b10x-bot credential input is unavailable".to_owned());
        return;
    };
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_file() || metadata.uid() != effective_uid {
        failures
            .push("b10x-bot credential input must be a current-user-owned regular file".to_owned());
        return;
    }
    let unsafe_mask = if private { 0o077 } else { 0o022 };
    if metadata.permissions().mode() & unsafe_mask != 0 {
        failures.push(if private {
            "b10x-bot credential input must be owner-only".to_owned()
        } else {
            "b10x-bot credential input must be not group/world-writable".to_owned()
        });
    }
    if fs::File::open(path).is_err() {
        failures.push("b10x-bot credential input is not readable".to_owned());
    }
}
