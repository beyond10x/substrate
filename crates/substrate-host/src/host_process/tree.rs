//! A record's cgroup subtree (S3).
//!
//! A member runs as the daemon's uid, so it can create cgroups beneath the record's own and move
//! itself into one. Such a member is still a member: `cgroup.kill` and `populated` cover the whole
//! subtree, so every other read of membership here does too — the `SIGTERM`, the removal and the
//! listening-socket observation. Each walk is bounded; a subtree past the bound is a named failure,
//! never a partial answer read as a whole one.

use std::collections::BTreeSet;
use std::os::fd::AsFd as _;
use std::path::{Path, PathBuf};

use super::{cgroup_failed, pidfd};
use crate::DriverError;

/// The most cgroups, the record's own included, one walk visits.
pub(super) const MAX_TREE_CGROUPS: usize = 16_384;
/// The most process descriptors held open at once while signalling.
const SIGNAL_BATCH: usize = 256;

/// The record's cgroup and every cgroup beneath it, parents before their children. Symlinks are
/// never followed.
///
/// # Errors
///
/// `process.cgroup-failed` when the top cannot be read or the subtree exceeds
/// [`MAX_TREE_CGROUPS`].
pub(super) fn cgroups(top: &Path) -> Result<Vec<PathBuf>, DriverError> {
    let mut found = vec![top.to_path_buf()];
    let mut next = 0;
    while next < found.len() {
        let directory = found[next].clone();
        next += 1;
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            // A nested cgroup removed during the walk has nothing left to visit.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && next > 1 => continue,
            Err(error) => return Err(cgroup_failed(format!("cgroup walk: {error}"))),
        };
        for entry in entries {
            let Ok(entry) = entry else { continue };
            if !matches!(entry.file_type(), Ok(kind) if kind.is_dir()) {
                continue;
            }
            if found.len() >= MAX_TREE_CGROUPS {
                return Err(cgroup_failed(format!(
                    "The host-process cgroup holds more than {MAX_TREE_CGROUPS} nested cgroups."
                )));
            }
            found.push(entry.path());
        }
    }
    Ok(found)
}

/// The pids one cgroup lists now; none for a cgroup removed meanwhile.
fn procs(cgroup: &Path) -> Result<Vec<u32>, DriverError> {
    match std::fs::read_to_string(cgroup.join("cgroup.procs")) {
        Ok(text) => Ok(text
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
            .collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(cgroup_failed(format!("cgroup processes: {error}"))),
    }
}

/// Every pid the subtree lists now.
pub(super) fn members(top: &Path) -> Result<BTreeSet<u32>, DriverError> {
    let mut members = BTreeSet::new();
    for cgroup in cgroups(top)? {
        members.extend(procs(&cgroup)?);
    }
    Ok(members)
}

/// Sends `signal` to every member of the subtree and returns the pids it reached.
///
/// Each pid is pinned by a pidfd first and signalled only if its cgroup still lists it after the
/// pin: a live process keeps its number, so a listed pid behind a live pidfd is that member, and a
/// pidfd on a process that has since exited reaches nobody. A pid recycled between the read and
/// the signal is therefore never signalled. A member that moves between nested cgroups during the
/// walk can be missed here; `cgroup.kill` after the grace still reaches it.
pub(super) fn signal_members(top: &Path, signal: i32) -> Result<BTreeSet<u32>, DriverError> {
    let mut reached = BTreeSet::new();
    for cgroup in cgroups(top)? {
        let listed = procs(&cgroup)?;
        for batch in listed.chunks(SIGNAL_BATCH) {
            let mut pinned = Vec::with_capacity(batch.len());
            for pid in batch {
                match pidfd::open(*pid) {
                    Ok(Some(fd)) => pinned.push((*pid, fd)),
                    Ok(None) => {}
                    Err(error) => return Err(cgroup_failed(format!("pidfd_open: {error}"))),
                }
            }
            if pinned.is_empty() {
                continue;
            }
            let still = procs(&cgroup)?.into_iter().collect::<BTreeSet<_>>();
            for (pid, fd) in pinned {
                if !still.contains(&pid) {
                    continue;
                }
                match pidfd::send(fd.as_fd(), signal) {
                    Ok(true) => {
                        reached.insert(pid);
                    }
                    Ok(false) => {}
                    Err(error) => {
                        return Err(cgroup_failed(format!("signal {signal} failed: {error}")));
                    }
                }
            }
        }
    }
    Ok(reached)
}

/// Removes the record's cgroup and every cgroup beneath it, children first. Call only once the
/// subtree is unpopulated: `rmdir` refuses a cgroup with members, so a populated one fails here
/// rather than being emptied.
///
/// # Errors
///
/// `process.cgroup-failed` when any cgroup in the subtree cannot be removed.
pub(super) fn remove(top: &Path) -> Result<(), DriverError> {
    for cgroup in cgroups(top)?.iter().rev() {
        match std::fs::remove_dir(cgroup) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(cgroup_failed(format!(
                    "cgroup cleanup of {}: {error}",
                    cgroup.display()
                )));
            }
        }
    }
    Ok(())
}
