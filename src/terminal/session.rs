use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    engine::{Colors, Engine, Grid, GridSize, SearchDump},
    input::{KeyInput, MouseInput},
};

const QUEUE_CAPACITY: usize = 64;
static NEXT_SESSION_GENERATION: AtomicU64 = AtomicU64::new(1);
const READ_BYTES: usize = 4096;
const BATCH_EVENTS: usize = 16;
pub(super) const MAX_PASTE_BYTES: usize = 256 * 1024;

pub(super) enum Command {
    Key(KeyInput),
    Mouse(MouseInput),
    Paste(String),
    Resize(GridSize),
    Colors(Colors),
    Scroll(isize),
    Focus(bool),
    Wake,
    Output(Vec<u8>),
    Eof,
    IoError(String),
    /// Dump scrollback text for find. The worker replies once; a stale
    /// receiver just drops it.
    SearchDump {
        reply: SyncSender<SearchReply>,
    },
}

#[derive(Debug, Clone)]
pub(super) struct SearchReply {
    pub dump: SearchDump,
    /// The content revision at the instant the worker made this dump.
    pub revision: u64,
}

/// Cap for find dumps: recent output wins, older lines fall off.
pub(super) const SEARCH_DUMP_CAP_BYTES: usize = 512 * 1024;

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
    /// Alert bells rung by the program since the session started.
    pub bells: u64,
}

struct Shared {
    snapshot: Mutex<Snapshot>,
    stopped: AtomicBool,
    visible: AtomicBool,
    repaint_pending: AtomicBool,
    bells: Arc<AtomicU64>,
    context: egui::Context,
    viewport: egui::ViewportId,
    generation: u64,
    content_revision: AtomicU64,
}

impl Shared {
    fn publish(&self, grid: Option<Grid>, status: Status) {
        let mut snapshot = self.snapshot.lock().unwrap();
        if let Some(grid) = grid {
            snapshot.grid = Some(Arc::new(grid));
        }
        let status_changed =
            std::mem::discriminant(&snapshot.status) != std::mem::discriminant(&status);
        snapshot.status = status;
        snapshot.revision += 1;
        snapshot.bells = self.bells.load(Ordering::Acquire);
        drop(snapshot);
        if (status_changed || self.visible.load(Ordering::Acquire))
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
        Self::start_command(context, cwd, size, colors, shell_command(cwd, colors))
    }

    pub(super) fn start_command(
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
                bells: 0,
            }),
            stopped: AtomicBool::new(false),
            bells: Arc::new(AtomicU64::new(0)),
            visible: AtomicBool::new(true),
            repaint_pending: AtomicBool::new(false),
            context: context.clone(),
            viewport: context.viewport_id(),
            generation: NEXT_SESSION_GENERATION.fetch_add(1, Ordering::Relaxed),
            content_revision: AtomicU64::new(0),
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

    pub(super) fn generation(&self) -> u64 {
        self.shared.generation
    }

    /// Viewport scrolling, colors, focus and search requests do not change the
    /// text. Keeping this separate avoids a search-request/repaint feedback loop.
    pub(super) fn content_revision(&self) -> u64 {
        self.shared.content_revision.load(Ordering::Acquire)
    }

    pub(super) fn request_search_dump(
        &self,
        reply: SyncSender<SearchReply>,
    ) -> Result<(), mpsc::TrySendError<()>> {
        self.sender
            .try_send(Command::SearchDump { reply })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => mpsc::TrySendError::Full(()),
                mpsc::TrySendError::Disconnected(_) => mpsc::TrySendError::Disconnected(()),
            })
    }

    #[cfg(test)]
    pub(super) fn search_test_session() -> (Self, Receiver<Command>) {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        (
            Self {
                sender,
                cwd: PathBuf::new(),
                shared: Arc::new(Shared {
                    snapshot: Mutex::new(Snapshot {
                        grid: None,
                        status: Status::Running,
                        revision: 0,
                        bells: 0,
                    }),
                    stopped: AtomicBool::new(false),
                    bells: Arc::new(AtomicU64::new(0)),
                    visible: AtomicBool::new(false),
                    repaint_pending: AtomicBool::new(false),
                    context: egui::Context::default(),
                    viewport: egui::ViewportId::ROOT,
                    generation: NEXT_SESSION_GENERATION.fetch_add(1, Ordering::Relaxed),
                    content_revision: AtomicU64::new(0),
                }),
            },
            receiver,
        )
    }

    pub(super) fn can_search(&self) -> bool {
        !self.shared.stopped.load(Ordering::Acquire)
            && matches!(
                self.shared.snapshot.lock().unwrap().status,
                Status::Starting | Status::Running | Status::Exited(_)
            )
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

fn shell_command(cwd: &Path, colors: Colors) -> CommandBuilder {
    let mut command = CommandBuilder::new_default_prog();
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    // Child TUI programs detect the light/dark theme from this, not from
    // escape queries alone. It is fixed at spawn; live palette switches for
    // already-running programs arrive through the VT defaults instead.
    command.env("COLORFGBG", colors.colorfgbg());
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
    let process = Process {
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
    let bells = shared.bells.clone();
    engine
        .terminal
        .on_bell(move |_| {
            bells.fetch_add(1, Ordering::Release);
        })
        .map_err(|error| error.to_string())?;
    shared.publish(
        Some(engine.snapshot().map_err(|error| error.to_string())?),
        Status::Running,
    );
    // EOF ends PTY I/O, but the engine remains available for scrollback
    // navigation, search and reflow. Its idle wait is a blocking mailbox read.
    let mut process = Some(process);
    let mut write_sender = Some(write_sender);
    let mut status = Status::Running;
    while !shared.stopped.load(Ordering::Acquire) {
        let Ok(first) = receiver.recv() else {
            break;
        };
        let mut batch = Some(first);
        // Rendering the grid is the batch's expensive step, and nobody sees
        // it while the pane is hidden. Program output must still be parsed
        // (queries, state) and status must still publish, but a batch of pure
        // program output skips the render walk. Anything else (resize,
        // colors, input, wakeups) renders normally so a reshown idle shell
        // never displays a stale grid.
        let mut refresh = false;
        let mut publish = false;
        for _ in 0..BATCH_EVENTS {
            if shared.stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            let Some(command) = batch.take().or_else(|| receiver.try_recv().ok()) else {
                break;
            };
            if !matches!(command, Command::SearchDump { .. }) {
                publish = true;
                refresh = refresh
                    || shared.visible.load(Ordering::Acquire)
                    || !matches!(command, Command::Output(_));
            }
            if matches!(command, Command::Resize(_))
                || (process.is_some() && matches!(command, Command::Output(_)))
            {
                shared.content_revision.fetch_add(1, Ordering::Release);
            }
            let mut bytes = Vec::new();
            match command {
                Command::Output(output) if process.is_some() => engine.terminal.vt_write(&output),
                Command::Key(input) if process.is_some() => {
                    bytes = engine.key(input).map_err(|error| error.to_string())?
                }
                Command::Mouse(input) if process.is_some() => {
                    bytes = engine.mouse(input).map_err(|error| error.to_string())?
                }
                Command::Paste(text) if process.is_some() => {
                    bytes = engine.paste(&text).map_err(|error| error.to_string())?
                }
                Command::Output(_) | Command::Key(_) | Command::Mouse(_) | Command::Paste(_) => {}
                Command::Resize(size) => {
                    engine.resize(size).map_err(|error| error.to_string())?;
                    if let Some(process) = &mut process {
                        process
                            .master
                            .resize(pty_size(size))
                            .map_err(|error| error.to_string())?;
                    }
                }
                Command::Colors(colors) => {
                    let flipped = engine
                        .set_colors(colors)
                        .map_err(|error| error.to_string())?;
                    // A light/dark flip reaches programs that enabled mode
                    // 2031 as an unsolicited color-scheme report, matching
                    // Ghostty: system theme switches propagate live.
                    if process.is_some()
                        && let Some(report) = engine
                            .color_scheme_notification(flipped)
                            .map_err(|error| error.to_string())?
                    {
                        bytes.extend_from_slice(&report);
                    }
                }
                Command::Scroll(delta) => engine
                    .terminal
                    .scroll_viewport(ScrollViewport::Delta(delta)),
                Command::SearchDump { reply } => {
                    let dump = engine
                        .search_dump(SEARCH_DUMP_CAP_BYTES)
                        .map_err(|error| error.to_string())?;
                    let revision = shared.content_revision.load(Ordering::Acquire);
                    if reply.try_send(SearchReply { dump, revision }).is_ok() {
                        // A search-only batch does not publish a grid, but its
                        // reply still needs to wake the receiving pane.
                        shared.context.request_repaint_of(shared.viewport);
                    }
                }
                Command::Focus(focused) => {
                    if process.is_some()
                        && engine
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
                    if let Some(mut ended) = process.take() {
                        // Reap even a child that deliberately closed its TTY.
                        // The emulator remains, but no PTY commands can run.
                        let message = ended
                            .child
                            .try_wait()
                            .map_err(|error| error.to_string())?
                            .map_or_else(
                                || "Terminal closed".into(),
                                |exit| format!("Shell exited ({exit})"),
                            );
                        status = Status::Exited(message);
                        engine
                            .terminal
                            .on_pty_write(|_, _| {})
                            .map_err(|error| error.to_string())?;
                        write_sender = None;
                        drop(ended);
                    }
                }
                // A writer may have already queued its final error when the
                // reader observed EOF. It cannot turn an ended shell Failed.
                Command::IoError(error) if process.is_some() => return Err(error),
                Command::IoError(_) => {}
                Command::Wake => {}
            }
            if !bytes.is_empty()
                && let Some(sender) = &write_sender
            {
                sender.try_send(bytes).map_err(|_| "Shell input queue is full or disconnected; session stopped to avoid dropping input".to_owned())?;
            }
            if reply_failed.swap(false, Ordering::AcqRel) && process.is_some() {
                return Err("Terminal reply queue is full or disconnected".into());
            }
        }
        if !publish {
            continue;
        }
        // Hidden output-only batches keep parsing and status but skip the
        // grid render walk; publish(None) retains the last grid.
        let grid = if refresh {
            Some(engine.snapshot().map_err(|error| error.to_string())?)
        } else {
            None
        };
        shared.publish(grid, status.clone());
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
    fn search_dump_and_navigation_do_not_invalidate_search_content() {
        let root = tempfile::tempdir().unwrap();
        let session = start_script(root.path(), "stty -echo; printf READY; sleep 10");
        until(|| text(&session.snapshot()).contains("READY"));
        let revision = session.content_revision();
        let (sender, receiver) = mpsc::sync_channel(1);
        session.send(Command::Scroll(-1)).unwrap();
        session.send(Command::Focus(false)).unwrap();
        session
            .send(Command::Colors(Colors {
                foreground: egui::Color32::WHITE,
                background: egui::Color32::BLACK,
            }))
            .unwrap();
        session.request_search_dump(sender).unwrap();
        let reply = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(reply.revision, revision);
        assert_eq!(session.content_revision(), revision);
        assert!(reply.dump.lines.iter().any(|line| line.contains("READY")));
        session.send(Command::Output(b"NEW".to_vec())).unwrap();
        until(|| session.content_revision() > revision);
    }

    #[test]
    #[cfg(unix)]
    fn search_reply_wakes_the_pane_without_publishing_or_dirtying_output() {
        let root = tempfile::tempdir().unwrap();
        let session = start_script(root.path(), "stty -echo; printf READY; sleep 10");
        until(|| text(&session.snapshot()).contains("READY"));
        let before = session.snapshot().revision;
        // Drain startup's already queued repaint before installing a callback;
        // an immediate pending repaint legitimately coalesces later requests.
        for frame in 0..3 {
            session
                .shared
                .context
                .run_ui(
                    egui::RawInput {
                        time: Some(f64::from(frame)),
                        ..Default::default()
                    },
                    |_| {},
                )
                .drop_without_applying_deltas();
        }
        let (repaint, repaints) = mpsc::channel();
        session
            .shared
            .context
            .set_request_repaint_callback(move |request| {
                let _ = repaint.send(request.viewport_id);
            });
        let (sender, receiver) = mpsc::sync_channel(1);
        session.request_search_dump(sender).unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            repaints.recv_timeout(Duration::from_secs(1)).unwrap(),
            egui::ViewportId::ROOT
        );
        assert_eq!(session.snapshot().revision, before);
    }

    #[test]
    #[cfg(unix)]
    fn completed_shell_searches_scrolls_and_reflows_without_reviving_pty_io() {
        let root = tempfile::tempdir().unwrap();
        let session = start_script(
            root.path(),
            "i=0; while [ $i -lt 30 ]; do printf 'finished%s\\r\\n' \"$i\"; i=$((i+1)); done",
        );
        until(|| matches!(session.snapshot().status, Status::Exited(_)));
        assert!(session.can_search());
        let before = session.content_revision();
        let (sender, receiver) = mpsc::sync_channel(1);
        session.request_search_dump(sender).unwrap();
        let reply = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(reply.revision, before);
        let first_hit = reply
            .dump
            .lines
            .iter()
            .position(|line| line == "finished0")
            .unwrap()
            + reply.dump.first_line;
        assert!(reply.dump.lines.iter().any(|line| line == "finished29"));
        let grid = session.snapshot().grid.unwrap();
        assert!(grid.scroll_offset > first_hit);
        session
            .send(Command::Scroll(
                first_hit as isize - grid.scroll_offset as isize,
            ))
            .unwrap();
        until(|| {
            session
                .snapshot()
                .grid
                .is_some_and(|grid| grid.scroll_offset == first_hit)
        });
        assert!(text(&session.snapshot()).contains("finished0"));
        assert_eq!(session.content_revision(), before);
        assert!(matches!(session.snapshot().status, Status::Exited(_)));

        // Ended input and focus events must not write to the closed PTY or
        // force the viewport back to the bottom. Reflow still updates search.
        session
            .send(Command::Paste("MUST NOT RUN\n".into()))
            .unwrap();
        session.send(Command::Focus(true)).unwrap();
        session
            .send(Command::IoError("late writer error".into()))
            .unwrap();
        session
            .send(Command::Resize(GridSize {
                cols: 12,
                rows: 6,
                cell_width: 8,
                cell_height: 16,
            }))
            .unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        session.request_search_dump(sender).unwrap();
        let reflowed = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(reflowed.revision > before);
        until(|| session.snapshot().grid.is_some_and(|grid| grid.cols == 12));
        assert!(matches!(session.snapshot().status, Status::Exited(_)));
        assert!(
            !reflowed
                .dump
                .lines
                .iter()
                .any(|line| line.contains("MUST NOT RUN"))
        );
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
    fn real_pty_answers_opencode_startup_burst() {
        let root = tempfile::tempdir().unwrap();
        // opencode's startup detection: OSC 10/11 plus all 16 palette slots,
        // BEL-terminated, in one burst. Every answer must come back or it
        // falls back to dark. Echo stays on so the slave's copies come back
        // through the reader; the byte count proves nothing was dropped.
        let session = start_script(root.path(), "stty -icanon min 1; printf READY; sleep 10");
        until(|| text(&session.snapshot()).contains("READY"));
        let mut burst = b"\x1b]11;?\x07\x1b]10;?\x07".to_vec();
        for index in 0..16 {
            burst.extend_from_slice(format!("\x1b]4;{index};?\x07").as_bytes());
        }
        session.send(Command::Output(burst)).unwrap();
        until(|| text(&session.snapshot()).matches("rgb:").count() == 18);
        let out = text(&session.snapshot());
        for marker in ["^[]11;", "^[]10;", "^[]4;15;"] {
            assert!(
                out.contains(marker),
                "burst answer missing {marker:?}: {out:?}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn real_pty_counts_alert_bells_for_hidden_panes() {
        let root = tempfile::tempdir().unwrap();
        let session = start_script(root.path(), "printf '\\aREADY'; sleep 10");
        until(|| text(&session.snapshot()).contains("READY"));
        until(|| session.snapshot().bells >= 1);
        assert_eq!(session.snapshot().status, Status::Running);
    }

    #[test]
    #[cfg(unix)]
    fn real_pty_reports_theme_flips_to_programs_with_mode_2031() {
        let root = tempfile::tempdir().unwrap();
        // Opt into color-scheme reports, then read the 9-byte flip report (`ESC[?997;2n`).
        let session = start_script(
            root.path(),
            "stty -echo; stty -icanon min 1; printf '\\033[?2031hREADY'; dd bs=1 count=9 2>/dev/null | od -An -t x1",
        );
        until(|| text(&session.snapshot()).contains("READY"));
        // The fixture session starts dark; switching light flips the scheme.
        session
            .send(Command::Colors(Colors {
                foreground: egui::Color32::BLACK,
                background: egui::Color32::WHITE,
            }))
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
        let report = text(&snapshot)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            report.contains("1b 5b 3f 39 39 37 3b 32 6e"),
            "theme flip report missing: {report}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn hidden_sessions_skip_grid_renders_but_keep_status() {
        let root = tempfile::tempdir().unwrap();
        let session = start_script(
            root.path(),
            "printf READY; while true; do sleep 1; printf TICK; done",
        );
        until(|| text(&session.snapshot()).contains("READY"));
        let before = session.snapshot();
        let grid_before = before.grid.clone().expect("initial grid");
        // Program-output-only batches while hidden keep parsing and status
        // but reuse the last grid instead of rendering a new one.
        session.set_visible(false);
        until(|| session.snapshot().revision > before.revision);
        let hidden = session.snapshot();
        assert_eq!(hidden.status, Status::Running);
        assert!(
            hidden
                .grid
                .as_ref()
                .is_some_and(|grid| Arc::ptr_eq(&grid_before, grid)),
            "hidden batch must reuse the grid"
        );
        // Reshown batches render fresh so an idle shell never looks stale.
        session.set_visible(true);
        session.send(Command::Wake).unwrap();
        until(|| session.snapshot().revision > hidden.revision);
        let shown = session.snapshot();
        assert!(
            shown
                .grid
                .as_ref()
                .is_some_and(|grid| !Arc::ptr_eq(&grid_before, grid)),
            "visible batch must render fresh"
        );
    }

    #[test]
    #[cfg(unix)]
    fn real_pty_delivers_mouse_events_to_tracking_programs() {
        use libghostty_vt::{key::Mods, mouse};
        let root = tempfile::tempdir().unwrap();
        // Button tracking with SGR: one left press at column 3, row 2.
        let session = start_script(
            root.path(),
            "stty -echo; stty -icanon min 1; printf '\\033[?1002;1006hREADY'; dd bs=1 count=9 2>/dev/null | od -An -t x1",
        );
        until(|| text(&session.snapshot()).contains("READY"));
        session
            .send(Command::Mouse(MouseInput {
                action: mouse::Action::Press,
                button: Some(mouse::Button::Left),
                mods: Mods::empty(),
                x: 20.0,
                y: 16.0,
                geometry: mouse::EncoderSize {
                    screen_width: 640,
                    screen_height: 128,
                    cell_width: 8,
                    cell_height: 16,
                    padding_top: 0,
                    padding_bottom: 0,
                    padding_right: 0,
                    padding_left: 0,
                },
                any_button_pressed: true,
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(
            session.snapshot().status,
            Status::Exited(_) | Status::Failed(_)
        ) {
            assert!(
                Instant::now() < deadline,
                "mouse bytes were not consumed; grid: {}",
                text(&session.snapshot())
            );
            thread::sleep(Duration::from_millis(10));
        }
        let snapshot = session.snapshot();
        assert!(
            matches!(snapshot.status, Status::Exited(_)),
            "{:?}",
            snapshot.status
        );
        let text = text(&snapshot);
        assert!(
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains("1b 5b 3c 30 3b 33 3b 32 4d"),
            "mouse press not delivered: {text}"
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
                    bells: 0,
                }),
                stopped: AtomicBool::new(false),
                bells: Arc::new(AtomicU64::new(0)),
                visible: AtomicBool::new(false),
                repaint_pending: AtomicBool::new(false),
                context: egui::Context::default(),
                viewport: egui::ViewportId::ROOT,
                generation: 1,
                content_revision: AtomicU64::new(0),
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
                status: Status::Running,
                revision: 0,
                bells: 0,
            }),
            stopped: AtomicBool::new(false),
            bells: Arc::new(AtomicU64::new(0)),
            visible: AtomicBool::new(false),
            repaint_pending: AtomicBool::new(false),
            context: egui::Context::default(),
            viewport: egui::ViewportId::ROOT,
            generation: 1,
            content_revision: AtomicU64::new(0),
        };
        for _ in 0..100 {
            shared.publish(None, Status::Running);
        }
        assert!(!shared.repaint_pending.load(Ordering::Acquire));
        shared.publish(None, Status::Failed("shell failed".into()));
        assert!(
            shared.repaint_pending.load(Ordering::Acquire),
            "hidden health changes must wake Activity"
        );
        shared.repaint_pending.store(false, Ordering::Release);
        shared.visible.store(true, Ordering::Release);
        shared.publish(None, Status::Running);
        assert!(shared.repaint_pending.load(Ordering::Acquire));
    }

    #[test]
    fn spawned_shell_advertises_its_theme_to_child_programs() {
        let dark = shell_command(
            Path::new("/tmp"),
            Colors {
                foreground: egui::Color32::WHITE,
                background: egui::Color32::BLACK,
            },
        );
        assert_eq!(
            dark.get_env("COLORFGBG"),
            Some(std::ffi::OsStr::new("15;0"))
        );
        let light = shell_command(
            Path::new("/tmp"),
            Colors {
                foreground: egui::Color32::BLACK,
                background: egui::Color32::WHITE,
            },
        );
        assert_eq!(
            light.get_env("COLORFGBG"),
            Some(std::ffi::OsStr::new("0;15"))
        );
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
