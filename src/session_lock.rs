use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};

/// Enforces the mutual-exclusion rules between the long-running "live"
/// sessions (Live Region Translate / Live Clipboard Translate) and one-shot
/// `capture` — each is its own OS process (see `daemon::spawn_subcommand`),
/// so this needs real cross-process coordination, not just an in-memory
/// flag. Two independently named locks (`region`, `clipboard`), each a
/// small file in the per-user config directory, using the same
/// flock-via-`fd-lock` approach as `daemon::acquire_single_instance_lock` —
/// the OS releases it automatically on any exit, including a crash, so
/// there's no stale-lock cleanup to get wrong.
///
/// The actual rule (only one combination is allowed to run concurrently:
/// Live Clipboard Translate + one-shot `capture`) falls out of which lock
/// each command checks before proceeding:
/// - `capture` checks `region` only — a running Live Region Translate holds
///   an active screen-capture session (`xcap`'s DXGI Desktop Duplication on
///   Windows, a PipeWire ScreenCast stream on Linux) that a one-shot
///   screenshot can contend with; Live Clipboard Translate never touches
///   the screen at all, so it isn't checked here.
/// - `watch-clipboard` checks `region`, then acquires `clipboard` for its
///   own session.
/// - `watch-region` checks `clipboard`, then acquires `region` for its own
///   session.
///
/// A second `region` or `clipboard` acquisition attempt while one is
/// already running (e.g. clicking the same tray menu item twice) falls out
/// of the same mechanism for free — the second `try_acquire` simply fails
/// like any other conflict, not a special case.
pub struct SessionLock {
    lock: fd_lock::RwLock<File>,
    path: PathBuf,
}

impl SessionLock {
    /// Opens (creating if needed) `<name>.lock` in the per-user config
    /// directory. Doesn't acquire anything yet — call `try_acquire` or
    /// `holder_pid`.
    pub fn open(name: &str) -> Result<Self> {
        let dir = crate::config::app_config_dir()
            .context("could not determine a config directory for this platform")?;
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create config directory {}", dir.display()))?;
        let path = dir.join(format!("{name}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open lock file {}", path.display()))?;
        Ok(Self {
            lock: fd_lock::RwLock::new(file),
            path,
        })
    }

    /// Tries to acquire this lock without blocking, writing this process's
    /// PID into the file on success (read back by another process's
    /// `holder_pid` if it later needs to offer "stop it for me"). Returns
    /// `None` if it's already held elsewhere — never blocks waiting for it.
    pub fn try_acquire(&mut self) -> Result<Option<fd_lock::RwLockWriteGuard<'_, File>>> {
        let mut guard = match self.lock.try_write() {
            Ok(guard) => guard,
            Err(_) => return Ok(None),
        };
        guard.set_len(0)?;
        guard.seek(SeekFrom::Start(0))?;
        write!(guard, "{}", std::process::id())?;
        guard.flush()?;
        Ok(Some(guard))
    }

    /// Best-effort: the PID last written by whoever holds (or most recently
    /// held) this lock. A plain, uncoordinated read of the file's raw
    /// bytes — not synchronized with the lock itself — so this is
    /// deliberately never used for correctness, only to offer a "stop it
    /// for me" button in a conflict prompt; if it's stale or unparseable,
    /// callers just don't offer that button.
    pub fn holder_pid(&self) -> Option<u32> {
        fs::read_to_string(&self.path).ok()?.trim().parse().ok()
    }

    /// Is this lock currently held by someone else? A plain `&self` peek
    /// (via a shared/read-mode lock attempt, which any existing exclusive
    /// holder blocks) with no lifetime tied to the result, deliberately
    /// kept separate from `try_acquire` — a function that both peeks *and*
    /// conditionally returns an `&mut self`-borrowed guard runs into a real
    /// borrow-checker limitation (the named lifetime on the returned guard
    /// forces the compiler to treat the whole input reference as borrowed
    /// for that lifetime, blocking any other access to it earlier in the
    /// same function, confirmed by trying exactly that and hitting E0502/
    /// E0499). Splitting the "is it free" check out like this avoids that
    /// entirely. Public (unlike a `fn` that would otherwise stay private)
    /// since `region_ipc::send` also needs it, to refuse queuing a command
    /// for a Live Region Translate session that isn't actually running.
    pub fn is_active(&self) -> bool {
        self.lock.try_read().is_err()
    }
}

/// Checks whether `lock` is free; if not, shows a conflict prompt naming
/// what's currently running (`blocking_name`) and, if the user agrees to
/// stop it, terminates that process and retries for a few seconds. Returns
/// the acquired guard — bind it to `_` to release it immediately after a
/// momentary check (`capture`'s use case: it doesn't hold the `region` lock
/// itself, it just needs to know Live Region Translate isn't active right
/// now), or to a named variable to hold it for a whole session (Live
/// Region/Clipboard Translate's use case). `None` means the user cancelled
/// — callers should treat that like any other user cancellation (log and
/// return `Ok(())`), not an error.
pub fn resolve_conflict<'a>(
    lock: &'a mut SessionLock,
    starting_what: &str,
    blocking_name: &str,
) -> Result<Option<fd_lock::RwLockWriteGuard<'a, File>>> {
    if !lock.is_active() {
        return Ok(lock.try_acquire()?);
    }

    let pid = lock.holder_pid();
    let choice = crate::popup::show_conflict(
        &format!("{blocking_name} is active"),
        &format!(
            "{blocking_name} is currently running, which can't run at the same time as {starting_what}."
        ),
        pid.is_some(),
    )?;

    match choice {
        crate::popup::ConflictChoice::Cancel => Ok(None),
        crate::popup::ConflictChoice::StopAndContinue => {
            if let Some(pid) = pid {
                terminate_process(pid)?;
            }
            // Give the OS a moment to actually release the flock once the
            // other process exits — `is_active` (no tied lifetime) in the
            // loop, one single `try_acquire` (the actual `'a`-tied borrow)
            // at the end, for the same reason `try_acquire` itself can't be
            // the thing called repeatedly in a loop here (E0499: each
            // iteration's call would conflict with the previous one, since
            // both are forced to share the same named lifetime `'a`).
            for _ in 0..20 {
                if !lock.is_active() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            match lock.try_acquire()? {
                Some(guard) => Ok(Some(guard)),
                None => anyhow::bail!("{blocking_name} didn't stop in time"),
            }
        }
    }
}

/// Terminates another `ocr-translate` process by PID — the "stop it for
/// me" half of a conflict prompt (see `popup::show_conflict`). Best-effort:
/// the PID came from a racy, uncoordinated read (`SessionLock::holder_pid`),
/// so a failure here (process already exited, PID reused by something
/// else entirely) is reported but not treated as fatal by callers — they
/// just retry acquiring the lock and surface whatever's still wrong then.
#[cfg(target_os = "windows")]
pub fn terminate_process(pid: u32) -> Result<()> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, false, pid)
            .map_err(|e| anyhow::anyhow!("failed to open process {pid}: {e}"))?;
        let result = TerminateProcess(handle, 1);
        let _ = CloseHandle(handle);
        result.map_err(|e| anyhow::anyhow!("failed to terminate process {pid}: {e}"))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn terminate_process(pid: u32) -> Result<()> {
    let status = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .context("failed to run `kill`")?;
    if !status.success() {
        anyhow::bail!("`kill -TERM {pid}` exited with {status}");
    }
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn terminate_process(_pid: u32) -> Result<()> {
    anyhow::bail!("stopping another session isn't implemented on this OS yet")
}
