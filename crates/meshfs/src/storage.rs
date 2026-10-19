//! Storage Driver and Kernel Acceleration for MeshFS
//! Detects underlying backends (ZFS, Btrfs, LVM Thin, POSIX) and executes zero-copy
//! Copy-on-Write clones via the Linux kernel `FICLONE` ioctl where supported.

use std::fs::File;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

// Linux FICLONE ioctl code: _IOW(0x94, 9, int) = 0x40049409
#[cfg(target_os = "linux")]
const FICLONE: libc::c_ulong = 0x40049409;

/// Storage backend disk metrics.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct StorageMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub used_percent: f64,
}

/// Information about the storage backend carrying MeshFS data.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackendInfo {
    pub backend_type: String,
    pub path: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub used_percent: f64,
    pub features: BackendFeatures,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BackendFeatures {
    pub reflink_support: bool,
    pub cow_support: bool,
    pub zpool: Option<String>,
    pub zfs_dataset: Option<String>,
    pub zfs_compression: Option<String>,
    pub lvm_thin_pool: bool,
}

/// Active storage driver instance.
#[derive(Debug, Clone)]
pub struct StorageDriver {
    pub path: PathBuf,
    pub info: BackendInfo,
}

impl StorageDriver {
    /// Detects storage characteristics of a given directory.
    pub fn detect(dir_path: &Path) -> Self {
        let info = Self::detect_backend(dir_path);
        Self {
            path: dir_path.to_path_buf(),
            info,
        }
    }

    /// Queries metrics dynamically on the filesystem.
    pub fn get_metrics(&self, dir_path: &Path) -> Option<StorageMetrics> {
        let (total, used, free, pct) = Self::statvfs(dir_path);
        Some(StorageMetrics {
            total_bytes: total,
            used_bytes: used,
            free_bytes: free,
            used_percent: pct,
        })
    }

    /// Detects storage characteristics and returns BackendInfo.
    pub fn detect_backend(dir_path: &Path) -> BackendInfo {
        let path_str = dir_path.to_string_lossy().to_string();
        let mut backend_type = "posix-dir".to_string();
        let mut features = BackendFeatures::default();

        // 1. Check ZFS via zfs list
        if let Ok(output) = Command::new("zfs")
            .args(["list", "-H", "-o", "name,mountpoint,compression", &path_str])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let parts: Vec<&str> = stdout.trim().split_whitespace().collect();
                if !parts.is_empty() {
                    backend_type = "zfs".to_string();
                    let dataset = parts[0].to_string();
                    let zpool = dataset.split('/').next().unwrap_or("").to_string();
                    let compression = parts.get(2).map(|s| s.to_string()).unwrap_or_else(|| "on".to_string());

                    features.cow_support = true;
                    features.zfs_dataset = Some(dataset);
                    features.zpool = Some(zpool);
                    features.zfs_compression = Some(compression);
                }
            }
        }

        // 2. Check Btrfs via btrfs subvolume show
        if backend_type == "posix-dir" {
            if let Ok(output) = Command::new("btrfs")
                .args(["subvolume", "show", &path_str])
                .output()
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.contains("Subvolume ID") || stdout.contains("Name:") {
                        backend_type = "btrfs".to_string();
                        features.reflink_support = true;
                        features.cow_support = true;
                    }
                }
            }
        }

        // 3. Check LVM Thin pool
        if backend_type == "posix-dir" {
            if let Ok(output) = Command::new("lvs")
                .args(["--noheadings", "-o", "lv_name,vg_name,lv_attr"])
                .output()
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        let fields: Vec<&str> = line.split_whitespace().collect();
                        if let Some(attr) = fields.last() {
                            if attr.starts_with('t') {
                                features.lvm_thin_pool = true;
                                break;
                            }
                        }
                    }
                }
            }
        }

        // 4. Query disk statistics via statvfs
        let (total, used, free, pct) = Self::statvfs(dir_path);

        BackendInfo {
            backend_type,
            path: path_str,
            total_bytes: total,
            used_bytes: used,
            free_bytes: free,
            used_percent: pct,
            features,
        }
    }

    /// Queries filesystem metrics via libc::statvfs.
    pub fn statvfs(path: &Path) -> (u64, u64, u64, f64) {
        use std::ffi::CString;
        use std::mem::MaybeUninit;

        let c_path = match CString::new(path.to_string_lossy().as_bytes()) {
            Ok(c) => c,
            Err(_) => return (0, 0, 0, 0.0),
        };

        let mut stat = MaybeUninit::<libc::statvfs>::uninit();
        let res = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };

        if res == 0 {
            let stat = unsafe { stat.assume_init() };
            let frsize = stat.f_frsize as u64;
            let total = stat.f_blocks as u64 * frsize;
            let free = stat.f_bavail as u64 * frsize;
            let used = total.saturating_sub(free);
            let pct = if total > 0 {
                ((used as f64 / total as f64) * 1000.0).round() / 10.0
            } else {
                0.0
            };
            (total, used, free, pct)
        } else {
            (0, 0, 0, 0.0)
        }
    }

    /// Performs a zero-copy CoW file reflink using the Linux FICLONE ioctl.
    /// Falls back to regular copy if FICLONE is not supported on the target filesystem.
    #[cfg(target_os = "linux")]
    pub fn clone_file(src: &Path, dest: &Path) -> io::Result<()> {
        let src_file = File::open(src)?;
        let dest_file = File::create(dest)?;

        let ret = unsafe {
            libc::ioctl(dest_file.as_raw_fd(), FICLONE, src_file.as_raw_fd())
        };

        if ret == 0 {
            // Kernel FICLONE succeeded: instant zero-copy reflink!
            Ok(())
        } else {
            // Reflink not supported or cross-device: fallback to standard stream copy
            drop(dest_file);
            drop(src_file);
            std::fs::copy(src, dest)?;
            Ok(())
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn clone_file(src: &Path, dest: &Path) -> io::Result<()> {
        std::fs::copy(src, dest)?;
        Ok(())
    }
}
