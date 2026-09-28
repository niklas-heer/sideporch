//! What the server uses: CPU, memory, disk and database, for admins.
//!
//! A background task samples CPU and memory every few seconds and keeps the
//! last ten minutes, so the system page can draw a short history.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use sysinfo::{Disks, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

const INTERVAL: Duration = Duration::from_secs(5);
/// Ten minutes of samples.
const KEPT: usize = 120;

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// Sideporch's CPU use, as a share of all cores, 0 to 100.
    pub process_cpu: f32,
    /// The whole machine's CPU use, 0 to 100.
    pub system_cpu: f32,
    pub process_memory: u64,
    pub used_memory: u64,
    pub total_memory: u64,
}

pub struct Monitor {
    samples: Mutex<VecDeque<Sample>>,
    started_at: i64,
}

impl Monitor {
    /// Starts sampling in the background.
    pub fn start() -> Arc<Self> {
        let monitor = Arc::new(Self {
            samples: Mutex::new(VecDeque::with_capacity(KEPT)),
            started_at: crate::now_ms(),
        });
        let sampler = Arc::clone(&monitor);
        tokio::spawn(async move {
            let mut system = System::new();
            let pid = sysinfo::get_current_pid().ok();
            let mut ticker = tokio::time::interval(INTERVAL);
            loop {
                ticker.tick().await;
                let (returned, sample) = tokio::task::spawn_blocking(move || {
                    let sample = measure(&mut system, pid);
                    (system, Some(sample))
                })
                .await
                .unwrap_or_else(|_| (System::new(), None));
                system = returned;
                if let (Some(sample), Ok(mut samples)) = (sample, sampler.samples.lock()) {
                    if samples.len() >= KEPT {
                        samples.pop_front();
                    }
                    samples.push_back(sample);
                }
            }
        });
        monitor
    }

    pub fn samples(&self) -> Vec<Sample> {
        self.samples
            .lock()
            .map(|samples| samples.iter().copied().collect())
            .unwrap_or_default()
    }

    pub const fn started_at(&self) -> i64 {
        self.started_at
    }
}

fn measure(system: &mut System, pid: Option<Pid>) -> Sample {
    system.refresh_memory();
    system.refresh_cpu_usage();
    let cores = system.cpus().len().max(1);
    let (process_cpu, process_memory) = pid
        .and_then(|pid| {
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                ProcessRefreshKind::nothing().with_cpu().with_memory(),
            );
            system.process(pid)
        })
        .map_or((0.0, 0), |process| {
            // Process CPU is per core, so it can pass 100 on a busy machine.
            let share = process.cpu_usage() / f32::from(u16::try_from(cores).unwrap_or(u16::MAX));
            (share.clamp(0.0, 100.0), process.memory())
        });
    Sample {
        process_cpu,
        system_cpu: system.global_cpu_usage().clamp(0.0, 100.0),
        process_memory,
        used_memory: system.used_memory(),
        total_memory: system.total_memory(),
    }
}

/// Facts that are cheap to read on request.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub os: String,
    pub cores: usize,
    pub load: (f64, f64, f64),
    pub machine_uptime_secs: u64,
    pub database_bytes: u64,
    pub files_bytes: u64,
    pub files_count: u64,
    pub disk_free: Option<u64>,
    pub disk_total: Option<u64>,
    pub data_dir: PathBuf,
}

/// Size of a file, or zero if it is missing.
fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// Reads the snapshot. Blocks; call off the async threads.
pub fn snapshot(data_dir: &Path, blobs: &crate::blobs::Blobs) -> Snapshot {
    let database_bytes = [
        data_dir.join("sideporch.db"),
        data_dir.join("sideporch.db-wal"),
        data_dir.join("sideporch.db-shm"),
    ]
    .iter()
    .map(|path| size(path))
    .fold(0_u64, u64::saturating_add);
    let (files_bytes, files_count) = blobs.usage();
    let canonical = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_owned());
    let disks = Disks::new_with_refreshed_list();
    // The disk whose mount point is the longest prefix of the data directory.
    let disk = disks
        .list()
        .iter()
        .filter(|disk| canonical.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len());
    let load = System::load_average();
    Snapshot {
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_owned()),
        cores: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        load: (load.one, load.five, load.fifteen),
        machine_uptime_secs: System::uptime(),
        database_bytes,
        files_bytes,
        files_count,
        disk_free: disk.map(sysinfo::Disk::available_space),
        disk_total: disk.map(sysinfo::Disk::total_space),
        data_dir: canonical,
    }
}

/// `1.5 GB`, `320 MB`, `12 kB`.
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut size = value;
    let mut unit = 0_usize;
    let mut tenths = 0_u64;
    while size >= 1_000 && unit < UNITS.len().saturating_sub(1) {
        tenths = (size % 1_000) / 100;
        size /= 1_000;
        unit = unit.saturating_add(1);
    }
    let name = UNITS.get(unit).copied().unwrap_or("B");
    if unit == 0 || size >= 100 {
        format!("{size} {name}")
    } else {
        format!("{size}.{tenths} {name}")
    }
}

/// `3 d 4 h`, `5 h 12 min`, `42 min`.
pub fn duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {minutes} min")
    } else {
        format!("{minutes} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes_and_durations() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1_500), "1.5 kB");
        assert_eq!(bytes(320_000_000), "320 MB");
        assert_eq!(bytes(2_340_000_000), "2.3 GB");
        assert_eq!(duration(42 * 60), "42 min");
        assert_eq!(duration(5 * 3_600 + 720), "5 h 12 min");
        assert_eq!(duration(3 * 86_400 + 4 * 3_600), "3 d 4 h");
    }

    #[test]
    fn measures_this_process() {
        let mut system = System::new();
        let sample = measure(&mut system, sysinfo::get_current_pid().ok());
        assert!(sample.process_memory > 0);
        assert!(sample.total_memory >= sample.used_memory);
    }
}
