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

    /// Terminate the owning process group and reap the direct child on every
    /// exit, including helpers still running after the direct child exited.
    pub(crate) fn wait(
        mut self,
        timeout: Duration,
        mut monitor: impl FnMut() -> io::Result<()>,
    ) -> io::Result<ExitStatus> {
        let started = Instant::now();
        loop {
            monitor()?;
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            if started.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child process timed out",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
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
    fn owned_wait_closes_descendant_pipes_after_timeout_or_cancellation() {
        use std::{io::BufRead as _, sync::mpsc};
        for cancel in [false, true] {
            let mut command = Command::new("sh");
            command
                .args(["-c", "sleep 30 & printf 'ready\\n'; wait"])
                .stdout(Stdio::piped());
            let mut child = OwnedChild::spawn(&mut command).unwrap();
            let stdout = child.child_mut().stdout.take().unwrap();
            let (sender, receiver) = mpsc::channel();
            let reader = thread::spawn(move || {
                let mut stdout = io::BufReader::new(stdout);
                let mut line = String::new();
                stdout.read_line(&mut line).unwrap();
                sender.send(line == "ready\n").unwrap();
                line.clear();
                // A surviving descendant would retain this pipe for 30 seconds.
                sender.send(stdout.read_line(&mut line).unwrap() == 0).ok();
            });
            assert_eq!(receiver.recv_timeout(Duration::from_secs(5)), Ok(true));
            let error = child
                .wait(Duration::ZERO, || {
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
            assert_eq!(receiver.recv_timeout(Duration::from_secs(5)), Ok(true));
            reader.join().unwrap();
        }
    }
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
