//! Pinned, worker-only Liquid downloads with bounded memory and prompt cancellation.
use super::check_cancelled;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{atomic::AtomicBool, mpsc},
    thread,
    time::{Duration, Instant},
};

pub(super) fn download(
    url: &str,
    bytes: u64,
    hash: &str,
    cancel: &AtomicBool,
    output: &mut impl Write,
) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Liquid's download URL must use HTTPS.".into());
    }
    let mut command = Command::new("/usr/bin/curl");
    command.args([
        "--disable",
        "--fail",
        "--location",
        "--max-redirs",
        "5",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "10",
        "--max-time",
        "900",
        "--speed-limit",
        "1024",
        "--speed-time",
        "30",
        "--silent",
        "--url",
        url,
    ]);
    download_command(&mut command, bytes, hash, cancel, output)
}
fn io_error(error: impl std::fmt::Display) -> String {
    format!("Liquid setup: {error}")
}
fn download_command(
    command: &mut Command,
    bytes: u64,
    hash: &str,
    cancel: &AtomicBool,
    output: &mut impl Write,
) -> Result<(), String> {
    check_cancelled(cancel)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = crate::process::OwnedChild::spawn(command).map_err(io_error)?;
    let mut pipe = child
        .child_mut()
        .stdout
        .take()
        .ok_or("Liquid download has no output stream.")?;
    let (sender, receiver) = mpsc::sync_channel::<Result<Vec<u8>, String>>(2);
    let reader = thread::Builder::new()
        .name("liquid-download-stream".into())
        .spawn(move || {
            loop {
                let mut chunk = vec![0; 64 * 1024];
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        chunk.truncate(count);
                        if sender.send(Ok(chunk)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(format!("Liquid download stream: {error}")));
                        break;
                    }
                }
            }
        })
        .map_err(io_error)?;
    let result = (|| {
        let deadline = Instant::now() + Duration::from_secs(15 * 60);
        let mut hasher = Sha256::new();
        let mut count = 0u64;
        loop {
            check_cancelled(cancel)?;
            if Instant::now() >= deadline {
                return Err("Liquid download timed out.".into());
            }
            match receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(chunk) => {
                    let chunk = chunk?;
                    count = count
                        .checked_add(chunk.len() as u64)
                        .ok_or("Liquid download is too large.")?;
                    if count > bytes {
                        return Err("Liquid download exceeds its pinned size.".into());
                    }
                    hasher.update(&chunk);
                    output.write_all(&chunk).map_err(io_error)?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        loop {
            check_cancelled(cancel)?;
            if let Some(status) = child.try_wait().map_err(io_error)? {
                if !status.success() {
                    return Err(format!(
                        "Liquid download failed ({status}). Check the connection and retry."
                    ));
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err("Liquid download timed out.".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        if count != bytes || format!("{:x}", hasher.finalize()) != hash {
            return Err("Liquid download failed its size or SHA-256 check.".into());
        }
        output.flush().map_err(io_error)
    })();
    // Close the bounded consumer before killing/reaping the downloader. Both a
    // blocked pipe read and a blocked channel send can then finish on every exit.
    drop(receiver);
    drop(child);
    crate::process::finish_reader_with_timeout(reader, Duration::from_millis(100));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[cfg(unix)]
    #[test]
    fn downloads_require_success_exact_size_and_checksum() {
        let cancel = AtomicBool::new(false);
        let hash = format!("{:x}", Sha256::digest(b"fixture"));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf fixture"]);
        let mut output = Vec::new();
        download_command(&mut command, 7, &hash, &cancel, &mut output).unwrap();
        assert_eq!(output, b"fixture");
        for (script, size, expected) in [
            ("printf fixture; exit 1", 7, hash.as_str()),
            ("printf fixture", 6, hash.as_str()),
            ("printf fixture", 8, hash.as_str()),
            ("printf fixture", 7, "invalid"),
        ] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            assert!(
                download_command(&mut command, size, expected, &cancel, &mut Vec::new()).is_err()
            );
        }
        assert!(
            download(
                "http://example.invalid/asset",
                7,
                &hash,
                &cancel,
                &mut Vec::new()
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn blocked_download_can_be_cancelled_without_waiting_for_a_network_timeout() {
        let cancel = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(30));
                cancel.store(true, Ordering::Release);
            });
            let mut command = Command::new("/bin/sleep");
            command.arg("30");
            assert!(
                download_command(&mut command, 4096, "unused", &cancel, &mut Vec::new())
                    .unwrap_err()
                    .contains("cancelled")
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn local_http_fixture_uses_real_streaming_transport() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let count = connection.read(&mut request).unwrap();
            assert!(request[..count].starts_with(b"GET /pinned HTTP/"));
            connection
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture",
                )
                .unwrap();
        });
        // HTTP is available solely through this injected test command; the
        // production entry point permits only HTTPS, including redirects.
        let mut command = Command::new("/usr/bin/curl");
        command.args([
            "--disable",
            "--fail",
            "--silent",
            "--noproxy",
            "*",
            "--url",
            &format!("http://{address}/pinned"),
        ]);
        let hash = format!("{:x}", Sha256::digest(b"fixture"));
        let mut output = Vec::new();
        download_command(&mut command, 7, &hash, &AtomicBool::new(false), &mut output).unwrap();
        server.join().unwrap();
        assert_eq!(output, b"fixture");
    }
}
