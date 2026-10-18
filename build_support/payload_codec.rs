//! The package and runtime accept one bounded, verified Zstandard frame.
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};

pub(crate) const WINDOW_LOG: u32 = 23;
const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Never publish a decoded file until this succeeds. The output and decoder's
/// history window have independent bounds; extra frames and trailing bytes are
/// rejected rather than silently ignored by the single-frame decoder.
pub(crate) fn decode(
    source: impl Read,
    bytes: u64,
    sha256: &str,
    output: &mut impl Write,
) -> io::Result<()> {
    if !(1..=MAX_PAYLOAD_BYTES).contains(&bytes) {
        return Err(invalid("Packaged payload has an unsupported size"));
    }
    let mut decoder = zstd::stream::read::Decoder::new(source)?.single_frame();
    decoder.window_log_max(WINDOW_LOG)?;
    let mut hasher = Sha256::new();
    let mut actual = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let limit = (bytes + 1 - actual).min(buffer.len() as u64) as usize;
        let count = decoder.read(&mut buffer[..limit])?;
        if count == 0 {
            break;
        }
        actual += count as u64;
        if actual > bytes {
            return Err(invalid("Packaged payload exceeds its declared size"));
        }
        hasher.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    if decoder.finish().read(&mut [0_u8])? != 0 {
        return Err(invalid(
            "Packaged payload has trailing data or extra frames",
        ));
    }
    if actual != bytes || format!("{:x}", hasher.finalize()) != sha256 {
        return Err(invalid(
            "Packaged payload failed size or SHA256 verification",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn encode_fixture(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    encoder.window_log(WINDOW_LOG).unwrap();
    encoder
        .set_pledged_src_size(Some(bytes.len() as u64))
        .unwrap();
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_fixture(archive: &[u8], bytes: &[u8]) -> io::Result<Vec<u8>> {
        let mut output = Vec::new();
        decode(
            archive,
            bytes.len() as u64,
            &format!("{:x}", Sha256::digest(bytes)),
            &mut output,
        )?;
        Ok(output)
    }

    #[test]
    fn single_frame_round_trips_and_rejects_truncation_and_checksum_damage() {
        let bytes = b"verified compressed resource";
        let archive = encode_fixture(bytes);
        assert_eq!(decode_fixture(&archive, bytes).unwrap(), bytes);
        for length in [0, 3, archive.len() - 1, archive.len() - 3] {
            assert!(decode_fixture(&archive[..length], bytes).is_err());
        }
        let mut damaged = archive;
        *damaged.last_mut().unwrap() ^= 0xff;
        assert!(decode_fixture(&damaged, bytes).is_err());
    }

    #[test]
    fn trailing_bytes_concatenated_frames_and_skippable_frames_are_rejected() {
        let bytes = b"one complete frame";
        let archive = encode_fixture(bytes);
        for suffix in [vec![0], encode_fixture(b"second frame")] {
            let mut concatenated = archive.clone();
            concatenated.extend(suffix);
            assert!(decode_fixture(&concatenated, bytes).is_err());
        }
        // A legal skippable frame is not a declared package resource.
        let mut skippable = vec![0x50, 0x2a, 0x4d, 0x18, 0, 0, 0, 0];
        skippable.extend(archive);
        assert!(decode_fixture(&skippable, bytes).is_err());
    }

    #[test]
    fn short_reads_preserve_frame_validation() {
        struct ShortReads<'a>(&'a [u8]);
        impl Read for ShortReads<'_> {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                let limit = output.len().min(1);
                self.0.read(&mut output[..limit])
            }
        }
        let bytes = b"a resource read in tiny pieces";
        let archive = encode_fixture(bytes);
        let hash = format!("{:x}", Sha256::digest(bytes));
        let mut output = Vec::new();
        decode(ShortReads(&archive), bytes.len() as u64, &hash, &mut output).unwrap();
        assert_eq!(output, bytes);
        assert!(
            decode(
                ShortReads(&archive[..archive.len() - 1]),
                bytes.len() as u64,
                &hash,
                &mut io::sink(),
            )
            .is_err()
        );
        let mut trailing = archive;
        trailing.push(0);
        assert!(
            decode(
                ShortReads(&trailing),
                bytes.len() as u64,
                &hash,
                &mut io::sink(),
            )
            .is_err()
        );
    }

    #[test]
    fn output_size_hash_and_decoder_window_are_bounded() {
        let bytes = vec![b'x'; 200_000];
        let archive = encode_fixture(&bytes);
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let mut output = Vec::new();
        assert!(decode(archive.as_slice(), 1, &hash, &mut output).is_err());
        assert!(output.len() <= 1);
        assert!(
            decode(
                archive.as_slice(),
                bytes.len() as u64,
                &"0".repeat(64),
                &mut io::sink()
            )
            .is_err()
        );
        for size in [0, MAX_PAYLOAD_BYTES + 1] {
            assert!(decode(archive.as_slice(), size, &hash, &mut io::sink()).is_err());
        }
        // Window descriptor 0x70 requests 16 MiB, above the package's 8 MiB cap.
        let oversized_window = [0x28, 0xb5, 0x2f, 0xfd, 0, 0x70, 1, 0, 0];
        assert!(decode(oversized_window.as_slice(), 1, &hash, &mut io::sink()).is_err());
    }
}
