use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

use eframe::egui;
use libghostty_vt::{
    focus,
    terminal::{Mode, ScrollViewport},
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use super::{
    engine::{Colors, Engine, Grid, GridSize},
    input::KeyInput,
};

const QUEUE_CAPACITY: usize = 64;
const READ_BYTES: usize = 4096;
const BATCH_EVENTS: usize = 16;
pub(super) const MAX_PASTE_BYTES: usize = 256 * 1024;

pub(super) enum Command {
    Key(KeyInput),
    Paste(String),
    Resize(GridSize),
    Colors(Colors),
    Scroll(isize),
    Focus(bool),
    Wake,
    Output(Vec<u8>),
    Eof,
    IoError(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Starting,
    Running,
    Exited(String),
    Failed(String),
}

#[derive(Clone)]
pub(super) struct Snapshot {
    pub grid: Option<Arc<Grid>>,
    pub status: Status,
    pub revision: u64,
}

struct Shared {
    snapshot: Mutex<Snapshot>,
    stopped: AtomicBool,
    visible: AtomicBool,
    repaint_pending: AtomicBool,
    context: egui::Context,
    viewport: egui::ViewportId,
}

impl Shared {
    fn publish(&self, grid: Option<Grid>, status: Status) {
        let mut snapshot = self.snapshot.lock().unwrap();
        if let Some(grid) = grid {
            snapshot.grid = Some(Arc::new(grid));
        }
        snapshot.status = status;
        snapshot.revision += 1;
        drop(snapshot);
        if self.visible.load(Ordering::Acquire)
            && !self.repaint_pending.swap(true, Ordering::AcqRel)
            && !self.stopped.load(Ordering::Acquire)
        {
            self.context.request_repaint_of(self.viewport);
        }
    }
}

pub(super) struct Session {
    sender: SyncSender<Command>,
    shared: Arc<Shared>,
    pub cwd: PathBuf,
}

impl Session {
    pub fn start(
        context: &egui::Context,
        cwd: &Path,
        size: GridSize,
        colors: Colors,
    ) -> Result<Self, String> {
        Self::start_command(context, cwd, size, colors, shell_command(cwd))
    }

    fn start_command(
        context: &egui::Context,
        cwd: &Path,
        size: GridSize,
        colors: Colors,
        command: CommandBuilder,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot {
                grid: None,
                status: Status::Starting,
                revision: 0,
            }),
            stopped: AtomicBool::new(false),
            visible: AtomicBool::new(true),
            repaint_pending: AtomicBool::new(false),
            context: context.clone(),
            viewport: context.viewport_id(),
        });
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let worker_shared = shared.clone();
        let worker_sender = sender.clone();
        thread::Builder::new()
            .name("terminal-engine".into())
            .spawn(move || {
                if let Err(error) = run(
                    &worker_shared,
                    worker_sender,
                    receiver,
                    command,
                    size,
                    colors,
                ) {
                    worker_shared.publish(None, Status::Failed(error));
                }
            })
            .map_err(|error| format!("Could not start terminal: {error}"))?;
        Ok(Self {
            sender,
            shared,
            cwd: cwd.to_owned(),
        })
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        if let Command::Paste(text) = &command
            && text.len() > MAX_PASTE_BYTES
        {
            return Err("Paste exceeds the terminal's 256 KiB input limit".into());
        }
        self.sender.try_send(command).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => {
                "Terminal is busy; input was not sent. Try again.".into()
            }
            mpsc::TrySendError::Disconnected(_) => {
                "The shell has stopped. Start a new terminal to continue.".into()
            }
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        // Clear before reading: a concurrent publish either lands in this
        // snapshot or leaves a repaint queued for the next frame.
        self.shared.repaint_pending.store(false, Ordering::Release);
        self.shared.snapshot.lock().unwrap().clone()
    }

    pub fn set_visible(&self, visible: bool) {
        self.shared.visible.store(visible, Ordering::Release);
        if !visible {
            self.shared.repaint_pending.store(false, Ordering::Release);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        // Cancellation cannot be lost when the bounded mailbox is full: the
        // worker checks the flag before processing every batch.
        let _ = self.sender.try_send(Command::Wake);
    }
}

fn shell_command(cwd: &Path) -> CommandBuilder {
    let mut command = CommandBuilder::new_default_prog();
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "tiptoptyp");
    command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
    command
}

struct Process {
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
}

impl Drop for Process {
    fn drop(&mut self) {
        // Runs on the worker, including every initialization/error path.
        // A foreground job may own the TTY even after the shell exits.
        // Terminate it so an inherited slave descriptor cannot retain I/O.
        #[cfg(unix)]
        if let Some(group) = self
            .master
            .process_group_leader()
            .filter(|group| *group > 0)
        {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn run(
    shared: &Shared,
    sender: SyncSender<Command>,
    receiver: Receiver<Command>,
    command: CommandBuilder,
    size: GridSize,
    colors: Colors,
) -> Result<(), String> {
    if shared.stopped.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut engine = Engine::new(size, colors).map_err(|error| error.to_string())?;
    let pair = native_pty_system()
        .openpty(pty_size(size))
        .map_err(|error| format!("Could not create PTY: {error}"))?;
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Could not start shell: {error}"))?;
    drop(pair.slave);
    let mut process = Process {
        child,
        master: pair.master,
    };
    let mut reader = process
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;
    let mut writer = process
        .master
        .take_writer()
        .map_err(|error| error.to_string())?;
    let (write_sender, write_receiver) = mpsc::sync_channel::<Vec<u8>>(QUEUE_CAPACITY);
    let errors = sender.clone();
    thread::Builder::new()
        .name("terminal-writer".into())
        .spawn(move || {
            while let Ok(bytes) = write_receiver.recv() {
                if let Err(error) = writer.write_all(&bytes) {
                    let _ = errors
                        .try_send(Command::IoError(format!("Terminal write failed: {error}")));
                    break;
                }
            }
        })
        .map_err(|error| error.to_string())?;
    thread::Builder::new()
        .name("terminal-reader".into())
        .spawn(move || {
            let mut buffer = [0; READ_BYTES];
            loop {
                let event = match reader.read(&mut buffer) {
                    Ok(0) => Command::Eof,
                    Ok(len) => Command::Output(buffer[..len].to_vec()),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    // Linux reports EIO when the slave side closes.
                    Err(error) if error.raw_os_error() == Some(5) => Command::Eof,
                    Err(error) => Command::IoError(format!("Terminal read failed: {error}")),
                };
                let finished = !matches!(event, Command::Output(_));
                if sender.send(event).is_err() || finished {
                    break;
                }
            }
        })
        .map_err(|error| error.to_string())?;
    let replies = write_sender.clone();
    let reply_failed = Arc::new(AtomicBool::new(false));
    let callback_failed = reply_failed.clone();
    engine
        .terminal
        .on_pty_write(move |_, bytes| {
            if replies.try_send(bytes.to_vec()).is_err() {
                callback_failed.store(true, Ordering::Release);
            }
        })
        .map_err(|error| error.to_string())?;
    shared.publish(
        Some(engine.snapshot().map_err(|error| error.to_string())?),
        Status::Running,
    );
    while !shared.stopped.load(Ordering::Acquire) {
        let Ok(first) = receiver.recv() else {
            break;
        };
        let mut batch = Some(first);
        for _ in 0..BATCH_EVENTS {
            if shared.stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            let Some(command) = batch.take().or_else(|| receiver.try_recv().ok()) else {
                break;
            };
            let mut bytes = Vec::new();
            match command {
                Command::Output(output) => engine.terminal.vt_write(&output),
                Command::Key(input) => {
                    bytes = engine.key(input).map_err(|error| error.to_string())?
                }
                Command::Paste(text) => {
                    bytes = engine.paste(&text).map_err(|error| error.to_string())?
                }
                Command::Resize(size) => {
                    engine.resize(size).map_err(|error| error.to_string())?;
                    process
                        .master
                        .resize(pty_size(size))
                        .map_err(|error| error.to_string())?;
                }
                Command::Colors(colors) => engine
                    .set_colors(colors)
                    .map_err(|error| error.to_string())?,
                Command::Scroll(delta) => engine
                    .terminal
                    .scroll_viewport(ScrollViewport::Delta(delta)),
                Command::Focus(focused) => {
                    if engine
                        .terminal
                        .mode(Mode::FOCUS_EVENT)
                        .map_err(|error| error.to_string())?
                    {
                        let mut buffer = [0; 8];
                        let event = if focused {
                            focus::Event::Gained
                        } else {
                            focus::Event::Lost
                        };
                        let len = event
                            .encode(&mut buffer)
                            .map_err(|error| error.to_string())?;
                        bytes.extend_from_slice(&buffer[..len]);
                    }
                }
                Command::Eof => {
                    // EOF normally means an exited shell. Do not wait on a
                    // still-running child that deliberately closed its TTY.
                    let status = process
                        .child
                        .try_wait()
                        .map_err(|error| error.to_string())?
                        .map_or_else(
                            || "Terminal closed".into(),
                            |status| format!("Shell exited ({status})"),
                        );
                    shared.publish(
                        Some(engine.snapshot().map_err(|error| error.to_string())?),
                        Status::Exited(status),
                    );
                    return Ok(());
                }
                Command::IoError(error) => return Err(error),
                Command::Wake => {}
            }
            if !bytes.is_empty() {
                write_sender.try_send(bytes).map_err(|_| "Shell input queue is full or disconnected; session stopped to avoid dropping input".to_owned())?;
            }
            if reply_failed.swap(false, Ordering::AcqRel) {
                return Err("Terminal reply queue is full or disconnected".into());
            }
        }
        shared.publish(
            Some(engine.snapshot().map_err(|error| error.to_string())?),
            Status::Running,
        );
    }
    Ok(())
}

fn pty_size(size: GridSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: size.cols.saturating_mul(size.cell_width),
        pixel_height: size.rows.saturating_mul(size.cell_height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn text(snapshot: &Snapshot) -> String {
        snapshot.grid.as_ref().map_or_else(String::new, |grid| {
            grid.rows
                .iter()
                .flat_map(|row| row.iter().map(|cell| cell.text.as_str()))
                .collect()
        })
    }

    #[cfg(unix)]
    fn start_script(root: &Path, script: &str) -> Session {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", script]);
        command.cwd(root);
        Session::start_command(
            &egui::Context::default(),
            root,
            GridSize {
                cols: 80,
                rows: 8,
                cell_width: 8,
                cell_height: 16,
            },
            Colors {
                foreground: egui::Color32::WHITE,
                background: egui::Color32::BLACK,
            },
            command,
        )
        .unwrap()
    }

    fn until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(
                Instant::now() < deadline,
                "terminal operation did not finish"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[cfg(unix)]
    fn real_pty_accepts_input_resizes_and_replies_to_terminal_queries() {
        let root = tempfile::tempdir().unwrap();
        let session = start_script(
            root.path(),
            "stty -echo; printf READY; IFS= read -r answer; printf '\\r\\nRECEIVED:%s\\r\\n' \"$answer\"; stty size; stty -icanon min 1; printf '\\033[6n'; dd bs=1 count=6 2>/dev/null | od -An -t x1",
        );
        until(|| text(&session.snapshot()).contains("READY"));
        session
            .send(Command::Resize(GridSize {
                cols: 90,
                rows: 12,
                cell_width: 8,
                cell_height: 16,
            }))
            .unwrap();
        session
            .send(Command::Paste("hello terminal\n".into()))
            .unwrap();
        until(|| {
            matches!(
                session.snapshot().status,
                Status::Exited(_) | Status::Failed(_)
            )
        });
        let snapshot = session.snapshot();
        assert!(
            matches!(snapshot.status, Status::Exited(_)),
            "{:?}",
            snapshot.status
        );
        let text = text(&snapshot);
        assert!(text.contains("RECEIVED:hello terminal"), "{text}");
        assert!(text.contains("12 90"), "{text}");
        assert!(
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains("1b 5b"),
            "query reply missing: {text}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn dropping_session_reaps_the_shell_and_foreground_job() {
        use nix::{sys::signal::kill, unistd::Pid};
        let root = tempfile::tempdir().unwrap();
        let session = start_script(
            root.path(),
            "echo $$ > shell.pid; sleep 30 & echo $! > job.pid; printf READY; wait",
        );
        until(|| text(&session.snapshot()).contains("READY"));
        let pid = |name| {
            Pid::from_raw(
                std::fs::read_to_string(root.path().join(name))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap(),
            )
        };
        let shell = pid("shell.pid");
        let job = pid("job.pid");
        drop(session);
        until(|| kill(shell, None).is_err() && kill(job, None).is_err());
    }

    #[test]
    fn full_queue_rejects_input_and_cancellation_still_wakes_worker() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let session = Session {
            cwd: PathBuf::new(),
            sender,
            shared: Arc::new(Shared {
                snapshot: Mutex::new(Snapshot {
                    grid: None,
                    status: Status::Starting,
                    revision: 0,
                }),
                stopped: AtomicBool::new(false),
                visible: AtomicBool::new(false),
                repaint_pending: AtomicBool::new(false),
                context: egui::Context::default(),
                viewport: egui::ViewportId::ROOT,
            }),
        };
        assert!(
            session
                .send(Command::Paste("x".repeat(MAX_PASTE_BYTES + 1)))
                .is_err()
        );
        session.send(Command::Wake).unwrap();
        assert!(
            session
                .send(Command::Paste("hello".into()))
                .unwrap_err()
                .contains("busy")
        );
        let shared = session.shared.clone();
        drop(session);
        assert!(shared.stopped.load(Ordering::Acquire));
        assert!(receiver.try_recv().is_ok());
    }

    #[test]
    fn hidden_output_does_not_schedule_a_repaint() {
        let shared = Shared {
            snapshot: Mutex::new(Snapshot {
                grid: None,
                status: Status::Starting,
                revision: 0,
            }),
            stopped: AtomicBool::new(false),
            visible: AtomicBool::new(false),
            repaint_pending: AtomicBool::new(false),
            context: egui::Context::default(),
            viewport: egui::ViewportId::ROOT,
        };
        for _ in 0..100 {
            shared.publish(None, Status::Running);
        }
        assert!(!shared.repaint_pending.load(Ordering::Acquire));
        shared.visible.store(true, Ordering::Release);
        shared.publish(None, Status::Running);
        assert!(shared.repaint_pending.load(Ordering::Acquire));
    }

    #[test]
    #[cfg(unix)]
    fn real_pty_runs_in_workspace_and_reports_exit() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "printf '\\033[32mPTY-OK\\033[0m\\r\\n'; pwd; exit 7"]);
        command.cwd(dir.path());
        let session = Session::start_command(
            &egui::Context::default(),
            dir.path(),
            GridSize {
                cols: 160,
                rows: 8,
                cell_width: 8,
                cell_height: 16,
            },
            Colors {
                foreground: egui::Color32::WHITE,
                background: egui::Color32::BLACK,
            },
            command,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snapshot = session.snapshot();
            assert!(
                !matches!(snapshot.status, Status::Failed(_)),
                "{:?}",
                snapshot.status
            );
            if matches!(snapshot.status, Status::Exited(_)) {
                let grid = snapshot.grid.unwrap();
                let text: String = grid
                    .rows
                    .iter()
                    .flat_map(|row| row.iter().map(|cell| cell.text.as_str()))
                    .collect();
                assert!(text.contains("PTY-OK"), "{text}");
                assert!(
                    text.contains(dir.path().file_name().unwrap().to_str().unwrap()),
                    "{text}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "PTY did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
