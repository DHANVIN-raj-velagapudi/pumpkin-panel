//! Restricting a server to a number of CPU cores.
//!
//! Unlike memory, core count can actually be *enforced* without a container:
//! both Windows and Linux let you pin a process to a subset of logical
//! processors. Pinning to the first N cores is what "allow N cores" means here.

/// Number of logical processors on this machine.
pub fn available_cores() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// Restricts `pid` to the first `cores` logical processors.
///
/// A `cores` value of 0, or one at or above the machine's core count, means
/// "use everything" and leaves the process untouched.
pub fn limit_to_cores(pid: u32, cores: usize) -> Result<(), String> {
    let total = available_cores();
    if cores == 0 || cores >= total {
        return Ok(());
    }
    apply(pid, cores)
}

#[cfg(windows)]
fn apply(pid: u32, cores: usize) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, SetProcessAffinityMask, PROCESS_SET_INFORMATION,
    };

    // A bitmask with the low `cores` bits set selects those processors.
    let mask: usize = if cores >= usize::BITS as usize {
        usize::MAX
    } else {
        (1usize << cores) - 1
    };

    // SAFETY: the handle is checked before use and closed on every path.
    unsafe {
        let handle = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err("could not open the server process to set its core limit".into());
        }

        let ok = SetProcessAffinityMask(handle, mask);
        CloseHandle(handle);

        if ok == 0 {
            return Err("the operating system refused the core limit".into());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn apply(pid: u32, cores: usize) -> Result<(), String> {
    // SAFETY: the cpu set is zeroed before use and sized by the libc type.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        for cpu in 0..cores {
            libc::CPU_SET(cpu, &mut set);
        }

        let result = libc::sched_setaffinity(
            pid as libc::pid_t,
            std::mem::size_of::<libc::cpu_set_t>(),
            &set,
        );
        if result != 0 {
            return Err("the operating system refused the core limit".into());
        }
    }
    Ok(())
}

#[cfg(not(any(windows, unix)))]
fn apply(_pid: u32, _cores: usize) -> Result<(), String> {
    Err("core limits are not supported on this platform".into())
}

/// Asks a process the panel does not own to shut down.
///
/// Used for servers adopted after a panel restart, where there is no stdin to
/// type `stop` into. On Unix this is a polite SIGTERM that Pumpkin can act on;
/// Windows has no equivalent for a console process, so the request is abrupt
/// and the caller should say so.
pub fn request_stop(pid: u32) -> Result<(), String> {
    stop_impl(pid)
}

#[cfg(windows)]
fn stop_impl(pid: u32) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    // SAFETY: the handle is checked before use and closed on every path.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err("could not open the server process".into());
        }
        let ok = TerminateProcess(handle, 0);
        CloseHandle(handle);
        if ok == 0 {
            return Err("the operating system refused to stop the process".into());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn stop_impl(pid: u32) -> Result<(), String> {
    // SAFETY: kill with SIGTERM on a pid is a plain syscall with no shared state.
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if result != 0 {
        return Err("the operating system refused to stop the process".into());
    }
    Ok(())
}

#[cfg(not(any(windows, unix)))]
fn stop_impl(_pid: u32) -> Result<(), String> {
    Err("stopping an adopted process is not supported on this platform".into())
}
