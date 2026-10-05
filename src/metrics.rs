// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Process resource sampling, shared by the live stats endpoint and the
//! background history recorder that feeds the charts.

use serde::Serialize;

/// One point on the CPU/memory charts.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Sample {
    pub at: i64,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

/// Samples CPU and memory for one process id.
///
/// `sysinfo` reports CPU as a sum across cores, so a process saturating 13 of
/// 32 threads reads as 1300%. Dividing by the logical core count turns that
/// back into a share of the whole machine, which is what the UI shows.
///
/// This blocks for a short interval, so callers should use `spawn_blocking`.
pub fn sample_process(pid: u32) -> (u64, f32) {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let pid = Pid::from_u32(pid);
    let mut system = System::new_all();
    let cores = system.cpus().len().max(1) as f32;

    // CPU percentage is derived from the delta between two samples.
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

    system.process(pid).map_or((0, 0.0), |process| {
        (process.memory(), (process.cpu_usage() / cores).min(100.0))
    })
}

pub fn host_memory() -> u64 {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    system.total_memory()
}

/// Whether `pid` is alive *and* is running the expected program.
///
/// The identity check matters: process ids get reused, and adopting whatever
/// happens to hold an old pid would be worse than not adopting at all.
pub fn process_matches(pid: u32, binary: &std::path::Path) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

    let Some(process) = system.process(pid) else {
        return false;
    };

    // Compare the executable path when the OS reports one; fall back to the
    // file name, which is all some platforms expose for other users' processes.
    match process.exe() {
        Some(exe) => {
            exe == binary
                || exe.file_name().is_some() && exe.file_name() == binary.file_name()
        }
        None => process
            .name()
            .to_str()
            .zip(binary.file_name().and_then(|n| n.to_str()))
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)),
    }
}

/// Reduces a path to a form that compares equal across the spellings Windows
/// reports for the same file: case, slash direction and the `\\?\` prefix.
fn normalise(path: &std::path::Path) -> String {
    let text = path
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_owned();

    // Windows paths ignore case; elsewhere `/srv/A` and `/srv/a` are different.
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

#[cfg(test)]
mod normalise_tests {
    use super::normalise;
    use std::path::Path;

    #[test]
    fn spellings_of_one_path_compare_equal() {
        let verbatim = normalise(Path::new(r"\\?\E:\pumpkin mc\server\"));
        let forward = normalise(Path::new("E:/pumpkin mc/server"));
        assert_eq!(verbatim, forward);
    }

    #[test]
    fn different_directories_stay_different() {
        assert_ne!(
            normalise(Path::new("E:/a/server")),
            normalise(Path::new("E:/b/server"))
        );
    }
}

/// Finds a process already running `binary`, for servers the panel did not
/// start itself (launched from a terminal, a shortcut, a service).
///
/// Deliberately stricter than [`process_matches`]: it requires the full
/// executable path to match, and the working directory too whenever the OS
/// will tell us. Two servers can share a binary, and adopting the wrong one
/// would show another server's players and let a Stop button kill it.
///
/// Returns the pid and the process start time (seconds since the epoch).
pub fn find_running(binary: &std::path::Path, working_dir: &std::path::Path) -> Option<(u32, i64)> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::OnlyIfNotSet),
    );

    let wanted_exe = normalise(binary);
    let wanted_dir = normalise(working_dir);

    system
        .processes()
        .values()
        .filter(|process| {
            process
                .exe()
                .is_some_and(|exe| normalise(exe) == wanted_exe)
        })
        .find(|process| {
            process
                .cwd()
                .is_none_or(|cwd| normalise(cwd) == wanted_dir)
        })
        .map(|process| (process.pid().as_u32(), process.start_time() as i64))
}
