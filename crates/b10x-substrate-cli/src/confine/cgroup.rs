//! The private child cgroup one `confine run` invocation opens its host driver on.
//!
//! The host driver reconciles every `substrate-ex_*` group directly beneath its cgroup root when it
//! opens: it kills each one as an orphan, which is right for the one daemon that owns a root. Two
//! `confine run` invocations, or one beside a daemon, would kill each other's runs on a shared
//! root. So each invocation makes its own group beneath the root the operator names, moves itself
//! into a leaf of it (the host's probe requires the process to sit beneath its cgroup root, and the
//! root to hold none directly), and hands that group to the driver: the driver's reconcile only
//! ever reads this invocation's own groups. On drop the process moves back to the group it came
//! from, whatever is left beneath the private group is killed, and the group is removed.
//!
//! `SIGKILL` skips the drop, so a killed invocation leaves its group. The group's name records its
//! owner as `confine-<pid>-<start time>`, and the next invocation on the root removes every such
//! group whose owner is gone and that holds no process, before it makes its own.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The controllers a host driver's cgroup root must give its children.
const CONTROLLERS: [&str; 3] = ["cpu", "memory", "pids"];

/// The name of every private group starts with this, then `<pid>-<start time>` of its owner.
const PREFIX: &str = "confine-";

/// Where this process sits while its private group is the driver's root.
const LEAF: &str = "cli";

/// One invocation's private group, removed when dropped.
#[derive(Debug)]
pub struct PrivateCgroup {
    group: PathBuf,
    origin: PathBuf,
}

impl PrivateCgroup {
    /// Makes a private group beneath `root` and moves this process into it.
    ///
    /// # Errors
    ///
    /// Why the group cannot be made: `root` is not a delegated cgroup v2 root this process runs
    /// beneath without being in it directly, or a controller, directory or move was refused.
    pub fn create(root: &Path) -> Result<Self, String> {
        let root = std::fs::canonicalize(root).map_err(|error| format!("{error}"))?;
        if !root.join("cgroup.controllers").is_file() {
            return Err("not a cgroup v2 directory".to_owned());
        }
        let origin = current_cgroup().ok_or("this process's cgroup is unreadable")?;
        if !origin.starts_with(&root) || origin == root {
            return Err(format!(
                "this process runs in {}, which is not beneath it; run beneath the root, in a \
                 group of its own",
                origin.display()
            ));
        }
        enable(&root)?;
        let start = start_time("self").ok_or("this process's start time is unreadable")?;
        reap(&root, &mut std::io::stderr());
        let group = root.join(format!("{PREFIX}{}-{start}", std::process::id()));
        std::fs::create_dir(&group)
            .map_err(|error| format!("creating {}: {error}", group.display()))?;
        let private = Self { group, origin };
        enable(&private.group)?;
        let leaf = private.group.join(LEAF);
        std::fs::create_dir(&leaf)
            .map_err(|error| format!("creating {}: {error}", leaf.display()))?;
        std::fs::write(leaf.join("cgroup.procs"), std::process::id().to_string())
            .map_err(|error| format!("moving into {}: {error}", leaf.display()))?;
        Ok(private)
    }

    /// The group the driver opens on.
    pub fn path(&self) -> &Path {
        &self.group
    }
}

impl Drop for PrivateCgroup {
    fn drop(&mut self) {
        let left = std::fs::write(
            self.origin.join("cgroup.procs"),
            std::process::id().to_string(),
        )
        .is_ok();
        if !left && in_group(&self.group) {
            // This process cannot leave, so `cgroup.kill` on the group would end it too: leave
            // the group in place rather than kill the invocation that owns it.
            return;
        }
        let _ = std::fs::write(self.group.join("cgroup.kill"), "1");
        for _ in 0..40 {
            if remove_tree(&self.group) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

/// Enables [`CONTROLLERS`] for `group`'s children, and `io` where `group` has it.
fn enable(group: &Path) -> Result<(), String> {
    let available = std::fs::read_to_string(group.join("cgroup.controllers"))
        .map_err(|error| format!("{}: {error}", group.display()))?;
    let enabled = std::fs::read_to_string(group.join("cgroup.subtree_control")).unwrap_or_default();
    let missing: Vec<&str> = CONTROLLERS
        .into_iter()
        .filter(|want| !enabled.split_whitespace().any(|have| have == *want))
        .collect();
    if let Some(absent) = missing
        .iter()
        .find(|want| !available.split_whitespace().any(|have| have == **want))
    {
        return Err(format!(
            "{} does not carry the {absent} controller",
            group.display()
        ));
    }
    if !missing.is_empty() {
        let line = missing
            .iter()
            .map(|controller| format!("+{controller}"))
            .collect::<Vec<_>>()
            .join(" ");
        std::fs::write(group.join("cgroup.subtree_control"), line).map_err(|error| {
            format!(
                "enabling {} for the children of {}: {error}",
                missing.join(" "),
                group.display()
            )
        })?;
    }
    if available.split_whitespace().any(|have| have == "io")
        && !enabled.split_whitespace().any(|have| have == "io")
    {
        let _ = std::fs::write(group.join("cgroup.subtree_control"), "+io");
    }
    Ok(())
}

/// This process's cgroup v2 directory.
fn current_cgroup() -> Option<PathBuf> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let relative = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    Some(Path::new("/sys/fs/cgroup").join(relative.trim().trim_start_matches('/')))
}

fn in_group(group: &Path) -> bool {
    current_cgroup().is_some_and(|current| current.starts_with(group))
}

/// The start time of the process `pid` (or `self`), in clock ticks since boot: field 22 of
/// `/proc/<pid>/stat`. With the pid it names one process for the life of the boot, where the pid
/// alone is reused.
fn start_time(pid: &str) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name, field 2, is parenthesised and may hold spaces and parentheses itself.
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(19)?.parse().ok()
}

/// The owner a private group's name records: `confine-<pid>-<start time>`.
fn owner(name: &str) -> Option<(&str, u64)> {
    let (pid, start) = name.strip_prefix(PREFIX)?.split_once('-')?;
    if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((pid, start.parse().ok()?))
}

/// Whether `group` or any group beneath it holds a process, or cannot be read.
fn holds_process(group: &Path) -> bool {
    match std::fs::read_to_string(group.join("cgroup.procs")) {
        Ok(procs) if procs.trim().is_empty() => {}
        _ => return true,
    }
    std::fs::read_dir(group).map_or(true, |entries| {
        entries.flatten().any(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir()) && holds_process(&entry.path())
        })
    })
}

/// Removes the private groups beneath `root` that an invocation killed by `SIGKILL` left behind:
/// each `confine-<pid>-<start time>` whose owner is gone — no process `pid`, or one started at
/// another time, which is the pid reused — and that holds no process, deepest first.
///
/// A group whose owner runs is never touched, whatever it holds, and a group holding any process
/// is left until it holds none: this removes, it never kills. `rmdir` on a cgroup that a process
/// joined in between fails with `EBUSY`, so nothing here can end a process.
///
/// What it leaves it names on `report`, one line each: a dead owner's group that still holds a
/// process (which no timeout or whole-tree kill bounds any longer), a group it could not remove,
/// and a root it could not read. None of them changes the invocation's own outcome.
fn reap(root: &Path, report: &mut impl std::io::Write) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            let _ = writeln!(
                report,
                "b10x-substrate: cannot read {} to remove groups killed runs left: {error}",
                root.display()
            );
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some((pid, start)) = name.to_str().and_then(owner) else {
            continue;
        };
        if start_time(pid) == Some(start) || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let group = entry.path();
        if holds_process(&group) {
            let _ = writeln!(
                report,
                "b10x-substrate: left {}: its owner {pid} is gone and a process still runs in it, \
                 or it cannot be read",
                group.display()
            );
        } else if !remove_tree(&group) {
            let _ = writeln!(
                report,
                "b10x-substrate: could not remove {}, left by a run that is gone",
                group.display()
            );
        }
    }
}

/// Removes `group` and every group beneath it, deepest first; `true` once `group` is gone.
fn remove_tree(group: &Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(group) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                remove_tree(&entry.path());
            }
        }
    }
    std::fs::remove_dir(group).is_ok() || !group.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_name_records_its_owner_and_only_that_process_matches_it() {
        let pid = std::process::id().to_string();
        let start = start_time("self").expect("this process's start time");
        assert_eq!(start_time(&pid), Some(start));
        let name = format!("{PREFIX}{pid}-{start}");
        assert_eq!(owner(&name), Some((pid.as_str(), start)));
        // The same pid started at another time is the pid reused, not the owner.
        assert_ne!(start_time(&pid), Some(start + 1));
        for other in [
            "confine-",
            "confine-12",
            "confine--5",
            "confine-1x-5",
            "substrate-ex_1-5",
        ] {
            assert_eq!(owner(other), None, "{other}");
        }
    }
}
