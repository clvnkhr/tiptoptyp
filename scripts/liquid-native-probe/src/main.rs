//! JSONL adapter around the production native Liquid modules for reproducible
//! parity, cross-architecture and warm/cold measurements. No alternate inference
//! or correction implementation lives in this runner.
#![allow(dead_code)] // The included modules expose application-only entry points.
#[path = "../../../src/liquid_runtime.rs"]
mod liquid_runtime;
#[path = "../../../src/liquid_text.rs"]
mod liquid_text;
#[path = "../../../build_support/payload_codec.rs"]
pub(crate) mod payload_codec;
#[path = "../../../src/process.rs"]
mod process;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};

mod settings {
    pub(crate) struct AppSettings {
        pub(crate) liquid_cache_directory: String,
        pub(crate) liquid_threads: u8,
    }
    impl Default for AppSettings {
        fn default() -> Self {
            Self {
                liquid_cache_directory: String::new(),
                liquid_threads: 4,
            }
        }
    }
}
mod bundled_payload {
    pub(crate) use super::payload_codec as codec;
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: u64,
    text: String,
}
fn main() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--metadata"] {
        println!(
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "target": if cfg!(target_arch = "aarch64") { "aarch64-apple-darwin" } else { "x86_64-apple-darwin" },
                "catalog_sha256": format!("{:x}", Sha256::digest(include_bytes!("../../../resources/liquid/manifest.json"))),
                "notices_catalog_sha256": format!("{:x}", Sha256::digest(include_bytes!("../../../resources/liquid/notices.json"))),
            })
        );
        return Ok(());
    }
    let mut directory = None;
    let mut threads = 4;
    let mut args = arguments.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cache-dir" => {
                directory = Some(PathBuf::from(args.next().ok_or("Missing cache directory")?))
            }
            "--threads" => {
                threads = args
                    .next()
                    .ok_or("Missing thread count")?
                    .parse::<u8>()
                    .map_err(|_| "Invalid thread count")?
            }
            _ => {
                return Err("Use --metadata, or --cache-dir PATH [--threads 1..8]".into());
            }
        }
    }
    let directory = directory.ok_or("Use --cache-dir PATH")?;
    liquid_runtime::validate_settings(directory.to_str().ok_or("Invalid folder path")?, threads)?;
    if !Path::new(&directory).is_absolute() {
        return Err("Use an absolute cache directory".into());
    }
    let config = liquid_runtime::Config { directory, threads };
    let mut client = liquid_runtime::Client::default();
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut line = Vec::new();
        std::io::Read::by_ref(&mut input)
            .take(8 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if line.is_empty() {
            return Ok(());
        }
        if line.len() > 8 * 1024 * 1024 || !line.ends_with(b"\n") {
            return Err("Oversized or incomplete request".into());
        }
        let request: Request = serde_json::from_slice(&line).map_err(|_| "Invalid request")?;
        let started = Instant::now();
        let result = client.correct_stream(
            &config,
            &[request.text],
            &std::sync::atomic::AtomicBool::new(false),
            |_, _| Ok(()),
        );
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let response = match result {
            Ok(mut corrected) => {
                serde_json::json!({"id": request.id, "ok": true, "corrected": corrected.remove(0), "elapsed_ms": elapsed_ms})
            }
            Err(message) => {
                serde_json::json!({"id": request.id, "ok": false, "error": {"code": "native_correction_failed", "message": message}, "elapsed_ms": elapsed_ms})
            }
        };
        serde_json::to_writer(&mut output, &response).map_err(|e| e.to_string())?;
        output
            .write_all(b"\n")
            .and_then(|_| output.flush())
            .map_err(|e| e.to_string())?;
    }
}
