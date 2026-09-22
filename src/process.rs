//! Shared termination and reaping for one-shot child processes.
//! Streaming LSP/watch adapters retain their protocol-specific supervision.
use std::{
    io,
    process::{Child, ExitStatus},
    thread,
    time::{Duration, Instant},
};

/// Join a pipe reader only after it has actually completed.
///
/// `Child::wait` closes the direct child's pipe handles, but an executable
/// wrapper can leave the same handles open in a descendant. Rust cannot cancel
/// a thread blocked in `Read`, so timing out deliberately detaches that reader
/// instead of hanging the owning worker or the application during `Drop`.
pub(crate) fn finish_reader_with_timeout<T>(
    reader: thread::JoinHandle<T>,
    timeout: Duration,
) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while !reader.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    reader.is_finished().then(|| reader.join().ok()).flatten()
}

struct ReapedChild(Child);
impl Drop for ReapedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Every exit path, including a monitor failure or unwind, reaps the child.
/// Monitor checks cancellation/output limits without erasing their error kind.
pub(crate) fn wait(
    child: Child,
    timeout: Duration,
    mut monitor: impl FnMut() -> io::Result<()>,
) -> io::Result<ExitStatus> {
    let mut child = ReapedChild(child);
    let start = Instant::now();
    loop {
        monitor()?;
        if let Some(status) = child.0.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "child process timed out",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// A finite external tool with deterministic cancellation and process reaping.
/// Unix tools get their own process group so custom wrappers cannot leave a
/// descendant running after a document/settings change.
pub(crate) struct OwnedChild(Child);
impl OwnedChild {
    pub(crate) fn spawn(command: &mut std::process::Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        command.spawn().map(Self)
    }
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }
    pub(crate) fn child_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(self.0.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    #[test]
    fn status_timeout_and_cancellation_are_distinct() {
        let child = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
        assert_eq!(
            wait(child, Duration::from_secs(5), || Ok(()))
                .unwrap()
                .code(),
            Some(7)
        );
        for cancel in [false, true] {
            let child = Command::new("sh")
                .args(["-c", "exec sleep 30"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            let error = wait(child, Duration::ZERO, || {
                if cancel {
                    Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(
                error.kind(),
                if cancel {
                    io::ErrorKind::Interrupted
                } else {
                    io::ErrorKind::TimedOut
                }
            );
        }
    }
}
