use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use substrate_wire::{StorageLimit, StorageQuotaFacts, StorageUsage};

use crate::DriverError;

pub(crate) const ALLOCATION_UNIT_BYTES: u64 = 1_024;
const FS_XFLAG_PROJINHERIT: u32 = 0x0000_0200;
const PRJQUOTA: u32 = 2;
const Q_GETQUOTA: u32 = (0x0080_0007_u32 << 8) | PRJQUOTA;
const Q_SETQUOTA: u32 = (0x0080_0008_u32 << 8) | PRJQUOTA;
const QIF_BLIMITS: u32 = 1;
const QIF_ILIMITS: u32 = 1 << 2;

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct Fsxattr {
    xflags: u32,
    extsize: u32,
    nextents: u32,
    projid: u32,
    cowextsize: u32,
    pad: [u8; 8],
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct IfDqblk {
    bhardlimit: u64,
    bsoftlimit: u64,
    curspace: u64,
    ihardlimit: u64,
    isoftlimit: u64,
    curinodes: u64,
    btime: u64,
    itime: u64,
    valid: u32,
}

struct AllocationState {
    used: HashSet<u32>,
    paths: HashMap<PathBuf, u32>,
}

pub(crate) struct ProjectQuotas {
    filesystem: File,
    range: (u32, u32),
    state: Mutex<AllocationState>,
}

impl ProjectQuotas {
    pub(crate) fn open(
        root: &Path,
        range: Option<(u32, u32)>,
    ) -> Result<Option<Arc<Self>>, DriverError> {
        let Some(range) = range else {
            return Ok(None);
        };
        let filesystem = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(root)
            .map_err(|_| quota_unserved())?;
        let quotas = Arc::new(Self {
            filesystem,
            range,
            state: Mutex::new(AllocationState {
                used: HashSet::new(),
                paths: HashMap::new(),
            }),
        });
        quotas.recover(root)?;
        Ok(Some(quotas))
    }

    pub(crate) const fn facts() -> StorageQuotaFacts {
        StorageQuotaFacts {
            allocation_unit_bytes: ALLOCATION_UNIT_BYTES,
            max_bytes: substrate_wire::MAX_STORAGE_QUOTA_BYTES,
            max_inodes: substrate_wire::MAX_STORAGE_QUOTA_INODES,
        }
    }

    pub(crate) fn probe(root: &Path, range: Option<(u32, u32)>) -> bool {
        let Some(_) = range else {
            return false;
        };
        let probe = root.join(format!(".substrate-quota-probe-{}", ulid::Ulid::generate()));
        if std::fs::create_dir(&probe).is_err() {
            return false;
        }
        let Ok(Some(manager)) = Self::open(root, range) else {
            let _ = std::fs::remove_dir(&probe);
            return false;
        };
        let result = (|| {
            manager
                .apply(
                    &probe,
                    StorageLimit {
                        max_bytes: substrate_wire::MIN_STORAGE_QUOTA_BYTES,
                        max_inodes: substrate_wire::MIN_STORAGE_QUOTA_INODES,
                    },
                )
                .ok()?;
            let project = project_id(&probe).ok()?;
            let child = probe.join("inherited");
            std::fs::create_dir(&child).ok()?;
            (project_id(&child).ok()? == project).then_some(())?;
            let mut file = File::create(child.join("ceiling")).ok()?;
            let mut inode_refused = false;
            for index in 0..32 {
                if let Err(error) = File::create(child.join(format!("inode-{index}"))) {
                    inode_refused = error.raw_os_error() == Some(libc::EDQUOT);
                    break;
                }
            }
            inode_refused.then_some(())?;
            let block = vec![0_u8; 64 * 1_024];
            let mut refused = false;
            for _ in 0..32 {
                if let Err(error) = file.write_all(&block) {
                    refused = error.raw_os_error() == Some(libc::EDQUOT);
                    break;
                }
            }
            refused.then_some(())
        })()
        .is_some();
        let _ = std::fs::remove_dir_all(&probe);
        let _ = manager.release(&probe);
        result
    }

    pub(crate) fn apply(
        &self,
        path: &Path,
        limit: StorageLimit,
    ) -> Result<StorageUsage, DriverError> {
        if !limit.within_contract_bounds() || !limit.max_bytes.is_multiple_of(ALLOCATION_UNIT_BYTES)
        {
            return Err(DriverError::exhausted(
                "storage.quota-limit",
                "Storage quota is outside the proved bounds or allocation unit.",
                "storage",
            ));
        }
        let mut state = self.state.lock();
        if state.paths.contains_key(path) {
            return Err(DriverError::refused(
                "storage.quota-conflict",
                "A quota is already attached to this resource.",
                "storage",
            ));
        }
        let project = (self.range.0..=self.range.1)
            .find(|candidate| !state.used.contains(candidate))
            .ok_or_else(|| {
                DriverError::exhausted(
                    "storage.project-id-capacity",
                    "The delegated storage quota identity range is exhausted.",
                    "storage",
                )
            })?;
        set_quota(&self.filesystem, project, limit)?;
        if let Err(error) = set_project(path, project) {
            let _ = clear_quota(&self.filesystem, project);
            return Err(error);
        }
        state.used.insert(project);
        state.paths.insert(path.to_path_buf(), project);
        drop(state);
        self.usage(path, limit)
    }

    pub(crate) fn usage(
        &self,
        path: &Path,
        limit: StorageLimit,
    ) -> Result<StorageUsage, DriverError> {
        let project = self
            .state
            .lock()
            .paths
            .get(path)
            .copied()
            .or_else(|| project_id(path).ok())
            .filter(|project| (self.range.0..=self.range.1).contains(project))
            .ok_or_else(quota_unserved)?;
        let quota = get_quota(&self.filesystem, project)?;
        Ok(StorageUsage {
            limit,
            used_bytes: quota.curspace,
            used_inodes: quota.curinodes,
            observed_at: Utc::now(),
        })
    }

    pub(crate) fn release(&self, path: &Path) -> Result<(), DriverError> {
        let mut state = self.state.lock();
        let Some(project) = state.paths.get(path).copied() else {
            return Ok(());
        };
        let quota = get_quota(&self.filesystem, project)?;
        if quota.curspace != 0 || quota.curinodes != 0 || path.exists() {
            return Err(DriverError::failed(
                "storage.quota-cleanup-incomplete",
                "Quota identity still has storage or a physical directory.",
            ));
        }
        clear_quota(&self.filesystem, project)?;
        state.paths.remove(path);
        state.used.remove(&project);
        Ok(())
    }

    fn recover(&self, root: &Path) -> Result<(), DriverError> {
        let mut state = self.state.lock();
        recover_directory(root, self.range, &mut state)?;
        let scratch = root.join(".substrate-scratch");
        if scratch.is_dir() {
            recover_directory(&scratch, self.range, &mut state)?;
        }
        Ok(())
    }
}

fn recover_directory(
    root: &Path,
    range: (u32, u32),
    state: &mut AllocationState,
) -> Result<(), DriverError> {
    for entry in std::fs::read_dir(root).map_err(|_| quota_unserved())? {
        let entry = entry.map_err(|_| quota_unserved())?;
        let path = entry.path();
        if !entry.file_type().map_err(|_| quota_unserved())?.is_dir() {
            continue;
        }
        if let Ok(project) = project_id(&path)
            && (range.0..=range.1).contains(&project)
        {
            if !state.used.insert(project) {
                return Err(DriverError::failed(
                    "storage.project-id-collision",
                    "Two retained resources carry one delegated quota identity.",
                ));
            }
            state.paths.insert(path, project);
        }
    }
    Ok(())
}

fn set_project(path: &Path, project: u32) -> Result<(), DriverError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| quota_unserved())?;
    let mut attr = fsxattr(&file)?;
    attr.projid = project;
    attr.xflags |= FS_XFLAG_PROJINHERIT;
    let request = nix::request_code_write!(b'X', 32, std::mem::size_of::<Fsxattr>());
    // SAFETY: request is FS_IOC_FSSETXATTR and points to a live correctly sized C representation.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request, &raw const attr) };
    if result == 0 {
        Ok(())
    } else {
        Err(quota_unserved())
    }
}

fn project_id(path: &Path) -> Result<u32, DriverError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| quota_unserved())?;
    Ok(fsxattr(&file)?.projid)
}

fn fsxattr(file: &File) -> Result<Fsxattr, DriverError> {
    let mut attr = Fsxattr::default();
    let request = nix::request_code_read!(b'X', 31, std::mem::size_of::<Fsxattr>());
    // SAFETY: request is FS_IOC_FSGETXATTR and points to writable correctly sized storage.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request, &raw mut attr) };
    if result == 0 {
        Ok(attr)
    } else {
        Err(quota_unserved())
    }
}

fn set_quota(filesystem: &File, project: u32, limit: StorageLimit) -> Result<(), DriverError> {
    let mut quota = IfDqblk {
        bhardlimit: limit.max_bytes / ALLOCATION_UNIT_BYTES,
        ihardlimit: limit.max_inodes,
        valid: QIF_BLIMITS | QIF_ILIMITS,
        ..IfDqblk::default()
    };
    quota_call(filesystem, Q_SETQUOTA, project, &raw mut quota)
}

fn clear_quota(filesystem: &File, project: u32) -> Result<(), DriverError> {
    let mut quota = IfDqblk {
        valid: QIF_BLIMITS | QIF_ILIMITS,
        ..IfDqblk::default()
    };
    quota_call(filesystem, Q_SETQUOTA, project, &raw mut quota)
}

fn get_quota(filesystem: &File, project: u32) -> Result<IfDqblk, DriverError> {
    let mut quota = IfDqblk::default();
    quota_call(filesystem, Q_GETQUOTA, project, &raw mut quota)?;
    Ok(quota)
}

fn quota_call(
    filesystem: &File,
    command: u32,
    project: u32,
    quota: *mut IfDqblk,
) -> Result<(), DriverError> {
    // SAFETY: quotactl_fd receives an owned filesystem fd and a pointer to the kernel ABI struct.
    let result = unsafe {
        libc::syscall(
            libc::SYS_quotactl_fd,
            filesystem.as_raw_fd(),
            command,
            project,
            quota,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(quota_unserved())
    }
}

fn quota_unserved() -> DriverError {
    DriverError::unserved(
        "storage.project-quota-unserved",
        "The configured filesystem did not prove delegated project quotas.",
        "storage",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_publish_the_exact_contract_bounds() {
        assert_eq!(ProjectQuotas::facts().allocation_unit_bytes, 1_024);
        assert_eq!(
            ProjectQuotas::facts().max_bytes,
            substrate_wire::MAX_STORAGE_QUOTA_BYTES
        );
        assert_eq!(
            ProjectQuotas::facts().max_inodes,
            substrate_wire::MAX_STORAGE_QUOTA_INODES
        );
    }

    #[test]
    fn no_delegated_range_proves_no_quota() {
        let root = tempfile::tempdir().unwrap();
        assert!(!ProjectQuotas::probe(root.path(), None));
        assert!(ProjectQuotas::open(root.path(), None).unwrap().is_none());
    }
}
