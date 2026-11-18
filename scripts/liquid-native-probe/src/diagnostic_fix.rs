//! Measure the production diagnostic request, local model and proposal guard.
#![allow(dead_code)]
#[path = "../../../src/diagnostic_fix.rs"]
mod diagnostic_fix;
#[path = "../../../src/diagnostic_fix_request.rs"]
mod diagnostic_fix_request;
#[path = "../../../src/diagnostics.rs"]
mod diagnostics;
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
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::Instant,
};

mod document {
    pub(crate) use tiptoptyp_core::document::{DocumentKey, DocumentKind};
}
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
struct Input {
    id: String,
    language: String,
    source: String,
    line: usize,
    diagnostic: String,
    #[serde(default)]
    replacement: Option<String>,
    #[serde(default)]
    expect_no_fix: bool,
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut directory = None;
    let mut threads = 4;
    let mut range_check = false;
    let mut prompt_only = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cache-dir" => {
                directory = Some(PathBuf::from(args.next().ok_or("Missing cache folder")?))
            }
            "--threads" => {
                threads = args
                    .next()
                    .ok_or("Missing thread count")?
                    .parse()
                    .map_err(|_| "Invalid thread count")?
            }
            "--range-check" => range_check = true,
            "--prompt-only" => prompt_only = true,
            "--metadata" => {
                println!(
                    "{}",
                    serde_json::json!({"format":1,"model_id":diagnostic_fix::MODEL_ID,
                    "catalog_sha256":hex::encode(Sha256::digest(include_bytes!("../../../resources/diagnostic-fix/manifest.json"))),
                    "backend_sha256":hex::encode(Sha256::digest(include_bytes!("../../../src/diagnostic_fix.rs"))),
                    "setup_sha256":hex::encode(Sha256::digest(include_bytes!("../../../src/diagnostic_fix/setup.rs"))),
                    "request_sha256":hex::encode(Sha256::digest(include_bytes!("../../../src/diagnostic_fix_request.rs")))})
                );
                return Ok(());
            }
            _ => {
                return Err(
                    "Use --metadata, --range-check, or --cache-dir PATH [--threads N]".into(),
                );
            }
        }
    }
    if range_check {
        let catalog: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../resources/diagnostic-fix/manifest.json"
        ))
        .unwrap();
        let mut output = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        let started = Instant::now();
        liquid_runtime::resources::download_range(
            catalog["model"]["url"].as_str().unwrap(),
            100_000_000..101_048_576,
            1_099_627_808,
            &AtomicBool::new(false),
            &mut output,
        )?;
        let bytes = std::fs::read(output.path()).map_err(|error| error.to_string())?;
        println!(
            "{}",
            serde_json::json!({"format":1,"range":[100_000_000,101_048_576],"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes)),"elapsed_ms":started.elapsed().as_secs_f64()*1000.0})
        );
        return Ok(());
    }
    let directory = directory.ok_or("Use an absolute --cache-dir PATH")?;
    let mut client = diagnostic_fix::Client::default();
    let mut reader = io::stdin().lock();
    let mut writer = io::stdout().lock();
    loop {
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut reader)
            .take(32 * 1024 + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > 32 * 1024 || !bytes.ends_with(b"\n") {
            return Err("Invalid request frame".into());
        }
        let input: Input = serde_json::from_slice(&bytes).map_err(|_| "Invalid request JSON")?;
        let started = Instant::now();
        let result = (|| {
            let kind = match input.language.as_str() {
                "typst" => document::DocumentKind::Typst,
                "tex" => document::DocumentKind::Tex,
                _ => return Err("Invalid language".into()),
            };
            let diagnostic = diagnostics::Diagnostic {
                correction: None,
                provider: Some("evaluation".into()),
                severity: diagnostics::DiagnosticSeverity::Error,
                source: diagnostics::DiagnosticSource::Main,
                location: Some(diagnostics::DiagnosticLocation {
                    line: input.line,
                    column: 1,
                }),
                message: input.diagnostic,
                details: Vec::new(),
            };
            let request = diagnostic_fix_request::Request::new(&input.source, kind, &diagnostic)?;
            if prompt_only {
                return Ok(serde_json::json!({"prompt":request.prompt()}));
            }
            let replacement = client.generate(
                &directory,
                threads,
                &request.prompt(),
                &AtomicBool::new(false),
            )?;
            let proposal = request.proposal(replacement.clone());
            Ok::<_, String>(
                serde_json::json!({"replacement":replacement,"proposal_valid":proposal.is_ok(),"proposal_error":proposal.as_ref().err(),"corrected":proposal.ok().and_then(|proposal|proposal.apply_to(&input.source).ok())}),
            )
        })();
        let response = match result {
            Ok(value) => {
                serde_json::json!({"id":input.id,"ok":true,"result":value,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0})
            }
            Err(error) => {
                serde_json::json!({"id":input.id,"ok":false,"error":error,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0})
            }
        };
        serde_json::to_writer(&mut writer, &response).map_err(|error| error.to_string())?;
        writer
            .write_all(b"\n")
            .and_then(|_| writer.flush())
            .map_err(|error| error.to_string())?;
    }
}
