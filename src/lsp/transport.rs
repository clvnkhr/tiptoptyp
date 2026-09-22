//! Bounded JSON-RPC framing. No session, process, UI or feature dependencies.

use serde::Serialize;
use serde_json::Value;
use std::io::{self, BufRead, Read, Write};

const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn write_lsp_message(
    writer: &mut impl Write,
    message: &impl Serialize,
) -> io::Result<()> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "JSON-RPC message is too large",
        ));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len())?;
    writer.write_all(&payload)?;
    writer.flush()
}

pub(crate) fn read_lsp_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    let mut total_header_bytes = 0usize;
    let mut saw_header = false;

    loop {
        let mut line = Vec::new();
        // Read at most one byte beyond the remaining budget, even without a newline.
        let limit = MAX_HEADER_LINE_BYTES.min(MAX_HEADER_BYTES - total_header_bytes) + 1;
        let read = (&mut *reader)
            .take(limit as u64)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            if !saw_header {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF in JSON-RPC headers",
            ));
        }
        saw_header = true;
        total_header_bytes = total_header_bytes.saturating_add(read);
        if line.len() > MAX_HEADER_LINE_BYTES || total_header_bytes > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON-RPC headers are too large",
            ));
        }

        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            break;
        }
        let header = std::str::from_utf8(&line)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let (name, value) = header.split_once(':').ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "malformed JSON-RPC header")
        })?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate Content-Length header",
                ));
            }
            let length = value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length header")
            })?;
            if length == 0 || length > MAX_MESSAGE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JSON-RPC payload length is outside the accepted range",
                ));
            }
            content_length = Some(length);
        }
    }

    let content_length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    let mut payload = vec![0; content_length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn unterminated_header_read_is_bounded_before_allocation() {
        let bytes = vec![b'x'; MAX_HEADER_LINE_BYTES * 4];
        let mut reader = Cursor::new(bytes);
        assert_eq!(
            read_lsp_message(&mut reader).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(reader.position(), (MAX_HEADER_LINE_BYTES + 1) as u64);
    }

    #[test]
    fn many_short_headers_cannot_exceed_the_total_budget() {
        let bytes = b"X: x\n".repeat(MAX_HEADER_BYTES);
        let mut reader = Cursor::new(bytes);
        assert_eq!(
            read_lsp_message(&mut reader).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(reader.position(), (MAX_HEADER_BYTES + 1) as u64);
    }

    #[test]
    fn invalid_lengths_are_rejected_before_reading_a_payload() {
        for length in [
            "0".to_owned(),
            "-1".to_owned(),
            "invalid".to_owned(),
            (MAX_MESSAGE_BYTES + 1).to_string(),
        ] {
            let header = format!("Content-Length: {length}\r\n");
            let mut reader = Cursor::new(format!("{header}\r\n{{}}"));
            assert_eq!(
                read_lsp_message(&mut reader).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(reader.position(), header.len() as u64);
        }
    }

    #[test]
    fn clean_eof_differs_from_truncated_headers_and_payloads() {
        assert_eq!(read_lsp_message(&mut Cursor::new(b"")).unwrap(), None);
        for bytes in [
            b"Content-Length: 2".as_slice(),
            b"Content-Length: 2\r\n\r\n{",
        ] {
            assert_eq!(
                read_lsp_message(&mut Cursor::new(bytes))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
        for bytes in [b"bad header\r\n".as_slice(), b"Content-Length: 1\r\n\r\nx"] {
            assert_eq!(
                read_lsp_message(&mut Cursor::new(bytes))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
}
