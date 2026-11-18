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

pub(crate) fn download(
    url: &str,
    bytes: u64,
    hash: &str,
    cancel: &AtomicBool,
    output: &mut impl Write,
) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Liquid's download URL must use HTTPS.".into());
    }
    let mut command = curl_command(url);
    download_command(&mut command, bytes, hash, cancel, output)
}

fn curl_command(url: &str) -> Command {
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
    command
}

/// A range is checked against its requested interval, then the caller verifies
/// the assembled artifact against its compiled full SHA-256 before publication.
pub(crate) fn download_range(
    url: &str,
    range: std::ops::Range<u64>,
    total: u64,
    cancel: &AtomicBool,
    output: &mut impl Write,
) -> Result<(), String> {
    if !url.starts_with("https://") || range.start >= range.end || range.end > total {
        return Err("Liquid's download range is invalid.".into());
    }
    let headers = tempfile::NamedTempFile::new().map_err(io_error)?;
    let mut command = curl_command(url);
    command
        .arg("--range")
        .arg(format!("{}-{}", range.start, range.end - 1))
        .arg("--dump-header")
        .arg(headers.path());
    stream_command(&mut command, range.end - range.start, None, cancel, output)?;
    if headers.as_file().metadata().map_err(io_error)?.len() > 64 * 1024 {
        return Err("Liquid download headers are too large.".into());
    }
    let bytes = std::fs::read(headers.path()).map_err(io_error)?;
    validate_range_headers(&bytes, &range, total)
}

fn validate_range_headers(
    headers: &[u8],
    range: &std::ops::Range<u64>,
    total: u64,
) -> Result<(), String> {
    if headers.len() > 64 * 1024 {
        return Err("Liquid download headers are too large.".into());
    }
    let headers = std::str::from_utf8(headers).map_err(io_error)?;
    let response = headers
        .rsplit("\r\n\r\n")
        .find(|part| part.starts_with("HTTP/"))
        .ok_or("Liquid range download has no HTTP response.")?;
    let mut lines = response.lines();
    if lines.next().and_then(|line| line.split_whitespace().nth(1)) != Some("206") {
        return Err("Liquid's model host did not support the requested byte range.".into());
    }
    let expected = format!("bytes {}-{}/{total}", range.start, range.end - 1);
    let ranges: Vec<_> = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-range"))
        .map(|(_, value)| value.trim())
        .collect();
    if ranges != [expected.as_str()] {
        return Err("Liquid's model host returned a different byte range.".into());
    }
    Ok(())
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
    stream_command(command, bytes, Some(hash), cancel, output)
}

fn stream_command(
    command: &mut Command,
    bytes: u64,
    hash: Option<&str>,
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
        if count != bytes || hash.is_some_and(|hash| hex::encode(hasher.finalize()) != hash) {
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

    #[test]
    fn ranged_model_download_requires_the_exact_final_response_interval() {
        let correct = b"HTTP/1.1 302 Found\r\nLocation: https://cdn.invalid/asset\r\n\r\nHTTP/2 206\r\nContent-Range: bytes 8-15/32\r\n\r\n";
        validate_range_headers(correct, &(8..16), 32).unwrap();
        for headers in [
            b"HTTP/2 200\r\nContent-Range: bytes 8-15/32\r\n\r\n".as_slice(),
            b"HTTP/2 206\r\nContent-Range: bytes 0-7/32\r\n\r\n".as_slice(),
            b"HTTP/2 206\r\nContent-Range: bytes 8-15/64\r\n\r\n".as_slice(),
            b"HTTP/2 206\r\nContent-Range: bytes 8-15/32\r\nContent-Range: bytes 8-15/32\r\n\r\n"
                .as_slice(),
        ] {
            assert!(validate_range_headers(headers, &(8..16), 32).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn downloads_require_success_exact_size_and_checksum() {
        let cancel = AtomicBool::new(false);
        let hash = hex::encode(Sha256::digest(b"fixture"));
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
        let hash = hex::encode(Sha256::digest(b"fixture"));
        let mut output = Vec::new();
        download_command(&mut command, 7, &hash, &AtomicBool::new(false), &mut output).unwrap();
        server.join().unwrap();
        assert_eq!(output, b"fixture");
    }
}
