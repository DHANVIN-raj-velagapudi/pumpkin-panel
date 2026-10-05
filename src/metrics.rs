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
