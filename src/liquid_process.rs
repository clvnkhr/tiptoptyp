//! Bounded child-process transport. Inference and all pipe I/O stay on workers.
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const MAX_FRAME_BYTES: usize = 64 * 1024;
const LIVE: u8 = 0;
const CLOSED: u8 = 1;
const INVALID: u8 = 2;
const CLOSED_ERROR: &str = "The grammar worker closed its connection.";
const INVALID_ERROR: &str = "The grammar worker sent an invalid response.";
type Reply = Result<Vec<u8>, &'static str>;

pub(crate) struct JsonLineProcess {
    child: Option<crate::process::OwnedChild>,
    requests: Option<SyncSender<Vec<u8>>>,
    replies: Receiver<Reply>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    state: Arc<AtomicU8>,
    pending: bool,
}

impl JsonLineProcess {
    pub(crate) fn spawn(command: &mut Command) -> Result<Self, String> {
        if !Path::new(command.get_program()).is_absolute() {
            return Err("The grammar worker must have an absolute executable path.".into());
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = crate::process::OwnedChild::spawn(command)
            .map_err(|_| "Cannot start the grammar worker.".to_owned())?;
        let mut stdin = child
            .child_mut()
            .stdin
            .take()
            .ok_or("Cannot open the grammar worker connection.")?;
        let stdout = child
            .child_mut()
            .stdout
            .take()
            .ok_or("Cannot open the grammar worker connection.")?;
        let (requests, request_receiver) = mpsc::sync_channel::<Vec<u8>>(1);
        let (reply_sender, replies) = mpsc::sync_channel(1);
        let state = Arc::new(AtomicU8::new(LIVE));
        let mut process = Self {
            child: Some(child),
            requests: Some(requests),
            replies,
            reader: None,
            writer: None,
            state,
            pending: false,
        };
        let reader_state = Arc::clone(&process.state);
        let writer_state = Arc::clone(&process.state);
        let writer_replies = reply_sender.clone();
        process.reader = Some(
            thread::Builder::new()
                .name("grammar-replies".into())
                .spawn(move || read_replies(BufReader::new(stdout), reply_sender, reader_state))
                .map_err(|_| "Cannot start the grammar worker connection.".to_owned())?,
        );
        process.writer = Some(
            thread::Builder::new()
                .name("grammar-requests".into())
                .spawn(move || {
                    while let Ok(frame) = request_receiver.recv() {
                        if stdin
                            .write_all(&frame)
                            .and_then(|()| stdin.flush())
                            .is_err()
                        {
                            writer_state.store(INVALID, Ordering::Release);
                            let _ = writer_replies.try_send(Err(CLOSED_ERROR));
                            break;
                        }
                    }
                })
                .map_err(|_| "Cannot start the grammar worker connection.".to_owned())?,
        );
        Ok(process)
    }

    /// Queue exactly one bounded wire line. The request format belongs to the
    /// caller; replies must be JSON. A blocked pipe write is covered by receive's
    /// deadline because the caller never writes directly to the child pipe.
    pub(crate) fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        if body.is_empty() || body.len() >= MAX_FRAME_BYTES - 1 || body.contains(&b'\n') {
            return Err("The grammar worker request is invalid or too large.".into());
        }
        if self.pending || self.state.load(Ordering::Acquire) != LIVE {
            self.stop();
            return Err("The grammar worker connection is unavailable.".into());
        }
        let mut frame = Vec::with_capacity(body.len() + 1);
        frame.extend_from_slice(body);
        frame.push(b'\n');
        if self
            .requests
            .as_ref()
            .is_none_or(|requests| requests.try_send(frame).is_err())
        {
            self.stop();
            return Err("The grammar worker connection is unavailable.".into());
        }
        self.pending = true;
        Ok(())
    }

    pub(crate) fn receive(&mut self, timeout: Duration) -> Result<Vec<u8>, String> {
        if self.child.is_none() {
            return Err(CLOSED_ERROR.into());
        }
        let reply = self
            .replies
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => "The grammar worker timed out.",
                mpsc::RecvTimeoutError::Disconnected => CLOSED_ERROR,
            });
        self.pending = false;
        let result = match reply {
            Ok(Ok(frame)) if self.state.load(Ordering::Acquire) != INVALID => Ok(frame),
            Ok(Ok(_)) => Err(INVALID_ERROR),
            Ok(Err(error)) | Err(error) => Err(error),
        };
        if result.is_err() || self.state.load(Ordering::Acquire) != LIVE {
            self.stop();
        }
        result.map_err(str::to_owned)
    }

    fn stop(&mut self) {
        self.requests.take();
        // OwnedChild kills the private Unix process group and reaps the child.
        // Close both pipes before joining, including descendants holding a pipe.
        self.child.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.state.store(CLOSED, Ordering::Release);
    }
}

impl Drop for JsonLineProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_frame(reader: &mut impl BufRead) -> Reply {
    let mut frame = Vec::new();
    let size = reader
        .take(MAX_FRAME_BYTES as u64)
        .read_until(b'\n', &mut frame)
        .map_err(|_| "Cannot read the grammar worker response.")?;
    if size == 0 {
        return Err(CLOSED_ERROR);
    }
    if size >= MAX_FRAME_BYTES || frame.last() != Some(&b'\n') {
        return Err("The grammar worker response is incomplete or too large.");
    }
    // Parse without echoing any worker text or serde error into an app message.
    let body = &frame[..frame.len() - 1];
    std::str::from_utf8(body).map_err(|_| INVALID_ERROR)?;
    serde_json::from_slice::<serde_json::Value>(body).map_err(|_| INVALID_ERROR)?;
    frame.pop();
    Ok(frame)
}

fn read_replies(mut reader: impl BufRead, replies: SyncSender<Reply>, state: Arc<AtomicU8>) {
    loop {
        let reply = read_frame(&mut reader);
        let terminal = reply.is_err();
        if let Err(error) = reply.as_ref() {
            state.fetch_max(
                if *error == CLOSED_ERROR {
                    CLOSED
                } else {
                    INVALID
                },
                Ordering::Release,
            );
        }
        match replies.try_send(reply) {
            Ok(()) if !terminal => {}
            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => break,
            Err(mpsc::TrySendError::Full(_)) => {
                // A valid complete frame followed by EOF remains consumable.
                // Extra data or malformed output poison this connection.
                if !terminal || state.load(Ordering::Acquire) == INVALID {
                    state.store(INVALID, Ordering::Release);
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    #[test]
    fn reply_framing_rejects_private_malformed_truncated_and_invalid_utf8_text() {
        for bytes in [
            &b"private document content\n"[..],
            &b"{\"private\":true}"[..],
            &b"\xff\n"[..],
            &b"\n"[..],
        ] {
            let error = read_frame(&mut Cursor::new(bytes)).unwrap_err();
            assert!(!error.contains("private"));
        }
        assert_eq!(
            read_frame(&mut Cursor::new(b"{\"ok\":true}\n")).unwrap(),
            b"{\"ok\":true}"
        );
    }

    #[test]
    fn oversized_output_is_bounded_before_newline_and_io_errors_stay_private() {
        let mut stream = Cursor::new(vec![b'x'; MAX_FRAME_BYTES * 16]);
        assert!(read_frame(&mut stream).is_err());
        assert_eq!(stream.position(), MAX_FRAME_BYTES as u64);
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("private child contents"))
            }
        }
        let error = read_frame(&mut BufReader::new(Broken)).unwrap_err();
        assert_eq!(error, "Cannot read the grammar worker response.");
    }

    #[test]
    fn reply_flood_never_blocks_reader_on_a_full_channel() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let state = Arc::new(AtomicU8::new(LIVE));
        read_replies(Cursor::new(b"{}\n{}\n{}\n"), sender, Arc::clone(&state));
        assert_eq!(state.load(Ordering::Acquire), INVALID);
        assert_eq!(receiver.recv().unwrap().unwrap(), b"{}");
    }

    #[test]
    fn valid_frame_before_eof_is_preserved() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let state = Arc::new(AtomicU8::new(LIVE));
        read_replies(Cursor::new(b"{}\n"), sender, Arc::clone(&state));
        assert_eq!(state.load(Ordering::Acquire), CLOSED);
        assert_eq!(receiver.recv().unwrap().unwrap(), b"{}");
    }

    #[cfg(unix)]
    fn shell(script: &str) -> JsonLineProcess {
        JsonLineProcess::spawn(Command::new("/bin/sh").args(["-c", script])).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn handshake_requests_and_changed_worker_configuration_are_independent() {
        for value in [1, 2] {
            let script = format!(
                "printf '{{\"ready\":true}}\\n'; while IFS= read -r line; do printf '{{\"value\":{value}}}\\n'; done"
            );
            let mut process = shell(&script);
            assert_eq!(
                process.receive(Duration::from_secs(5)).unwrap(),
                b"{\"ready\":true}"
            );
            assert!(process.send(b"a\nb").is_err());
            for request in [b"1 3 4".as_slice(), b"2 5 6\n"] {
                process.send(request).unwrap();
                let reply = process.receive(Duration::from_secs(5)).unwrap();
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&reply).unwrap()["value"],
                    value
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn timeout_reaps_pipe_holding_descendants_and_fresh_worker_recovers() {
        let mut process = shell("/bin/sleep 30");
        assert!(
            process
                .receive(Duration::from_millis(30))
                .unwrap_err()
                .contains("timed out")
        );
        assert!(process.child.is_none() && process.reader.is_none() && process.writer.is_none());
        assert!(process.send(b"request").is_err());
        let mut replacement = shell("printf '{}\\n'");
        assert_eq!(replacement.receive(Duration::from_secs(5)).unwrap(), b"{}");
    }

    #[cfg(unix)]
    #[test]
    fn blocked_stdin_write_obeys_receive_timeout_and_drop_joins_workers() {
        let mut process = shell("/bin/sleep 30");
        process.send(&vec![b'x'; MAX_FRAME_BYTES - 2]).unwrap();
        assert!(process.receive(Duration::from_millis(30)).is_err());
        assert!(process.child.is_none() && process.reader.is_none() && process.writer.is_none());
    }

    #[test]
    fn relative_executable_is_rejected_without_starting_it() {
        assert!(JsonLineProcess::spawn(&mut Command::new("sh")).is_err());
    }
}
