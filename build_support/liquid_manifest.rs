//! Portable Liquid ONNX Runtime payload shared by the app and package builder.
#![allow(dead_code)] // Runtime and packaging consume different parts of the contract.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const FORMAT: u32 = 3;
pub const MODEL_ID: &str = "lfm25-encoder-350m-onnxruntime-v1";
pub const UPSTREAM_REVISION: &str = "4e2f724c10985e9ec90f34189c22be5d5f2e41e0";
pub const MANIFEST_NAME: &str = "manifest.json";
pub const MODEL_NAME: &str = "liquid.onnx";
pub const TOKENIZER_NAME: &str = "tokenizer.json";
pub const HELPER_NAME: &str = "liquid-onnx";
pub const MACOS_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024 + 64 * 1024 * 1024;
const MAX_NATIVE_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_HELPER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOKENIZER_BYTES: u64 = 32 * 1024 * 1024;

const NATIVE_FILES: [&str; 2] = [HELPER_NAME, "libonnxruntime.dylib"];

pub fn native_names(target: &str) -> Option<&'static [&'static str]> {
    MACOS_TARGETS.contains(&target).then_some(&NATIVE_FILES)
}
pub fn runtime_version(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-apple-darwin" => Some("1.31.0"),
        "x86_64-apple-darwin" => Some("1.19.2"),
        _ => None,
    }
}
pub fn minimum_macos(target: &str) -> Option<(u32, u32)> {
    match target {
        "aarch64-apple-darwin" => Some((14, 0)),
        "x86_64-apple-darwin" => Some((12, 0)),
        _ => None,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub model_id: String,
    pub upstream_revision: String,
    pub files: Vec<Asset>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub name: String,
    /// Shared model/tokenizer assets omit the target. Native code always has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub bytes: u64,
    pub sha256: String,
    pub compressed_bytes: u64,
    pub compressed_sha256: String,
}
impl Asset {
    pub fn relative_path(&self) -> String {
        match self.target.as_deref() {
            Some(target) => format!("runtime/{target}/{}", self.name),
            None => self.name.clone(),
        }
    }
    pub fn archive_relative_path(&self) -> String {
        format!("{}.zst", self.relative_path())
    }
    pub fn is_native(&self) -> bool {
        self.target.is_some()
    }
}
impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("Liquid manifest is too large".into());
        }
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|_| "Liquid manifest is invalid".to_owned())?;
        manifest.validate()?;
        Ok(manifest)
    }
    /// A complete universal package includes both runtimes. A running process
    /// verifies/extracts only the shared files and its current architecture.
    pub fn files_for_target<'a>(&'a self, target: &'a str) -> impl Iterator<Item = &'a Asset> + 'a {
        self.files
            .iter()
            .filter(move |file| file.target.as_deref().is_none_or(|value| value == target))
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT || self.model_id != MODEL_ID {
            return Err("Liquid model version is unsupported".into());
        }
        if self.upstream_revision != UPSTREAM_REVISION {
            return Err("Liquid model revision is invalid".into());
        }
        let mut remaining = BTreeSet::from([(None, MODEL_NAME), (None, TOKENIZER_NAME)]);
        for target in MACOS_TARGETS {
            for &name in native_names(target).expect("declared Mac target") {
                remaining.insert((Some(target), name));
            }
        }
        let mut total = 0u64;
        for file in &self.files {
            if !remaining.remove(&(file.target.as_deref(), file.name.as_str()))
                || file.bytes == 0
                || file.bytes > MAX_FILE_BYTES
                || file.compressed_bytes == 0
                || file.compressed_bytes > MAX_FILE_BYTES
                || (file.name == TOKENIZER_NAME && file.bytes > MAX_TOKENIZER_BYTES)
                || (file.is_native() && file.bytes > MAX_NATIVE_FILE_BYTES)
                || (file.name == HELPER_NAME && file.bytes > MAX_HELPER_BYTES)
                || [&file.sha256, &file.compressed_sha256].iter().any(|value| {
                    value.len() != 64
                        || !value
                            .bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
            {
                return Err("Liquid model or runtime file entry is invalid".into());
            }
            total = total
                .checked_add(file.bytes)
                .ok_or("Liquid package is too large")?;
        }
        if !remaining.is_empty() || total > MAX_TOTAL_BYTES {
            return Err(
                "Liquid package must contain its model, tokenizer and both Mac runtimes".into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest() -> Manifest {
        let mut files: Vec<_> = [MODEL_NAME, TOKENIZER_NAME]
            .into_iter()
            .map(|name| asset(None, name))
            .collect();
        for target in MACOS_TARGETS {
            files.extend(
                native_names(target)
                    .unwrap()
                    .iter()
                    .map(|name| asset(Some(target), name)),
            );
        }
        Manifest {
            format: FORMAT,
            model_id: MODEL_ID.into(),
            upstream_revision: UPSTREAM_REVISION.into(),
            files,
        }
    }
    fn asset(target: Option<&str>, name: &str) -> Asset {
        Asset {
            name: name.into(),
            target: target.map(str::to_owned),
            bytes: 8,
            sha256: "b".repeat(64),
            compressed_bytes: 4,
            compressed_sha256: "c".repeat(64),
        }
    }
    #[test]
    fn universal_contract_requires_exact_shared_assets_and_both_runtime_closures() {
        let complete = manifest();
        complete.validate().unwrap();
        assert_eq!(complete.files.len(), 6);
        for target in MACOS_TARGETS {
            let selected: Vec<_> = complete.files_for_target(target).collect();
            assert_eq!(selected.len(), 4);
            assert_eq!(selected.iter().filter(|file| !file.is_native()).count(), 2);
            assert!(
                selected
                    .iter()
                    .filter_map(|file| file.target.as_deref())
                    .all(|actual| actual == target)
            );
        }
        for index in 0..complete.files.len() {
            let mut value = manifest();
            value.files.remove(index);
            assert!(value.validate().is_err());
        }
        let mut value = manifest();
        value.files.push(value.files[2].clone());
        assert!(value.validate().is_err());
    }
    #[test]
    fn rejects_old_formats_unsafe_paths_unsupported_targets_and_noncanonical_hashes() {
        for name in [
            "../liquid.onnx",
            "/liquid.onnx",
            "liquid.pt",
            "weights.bin",
            "tokenizer.json.zst",
        ] {
            let mut value = manifest();
            value.files[0].name = name.into();
            assert!(value.validate().is_err());
        }
        for target in [
            "../aarch64-apple-darwin",
            "",
            "universal-apple-darwin",
            "x86_64-unknown-linux-gnu",
        ] {
            let mut value = manifest();
            value.files[2].target = Some(target.into());
            assert!(value.validate().is_err());
        }
        for hash in ["g".repeat(64), "B".repeat(64), "b".repeat(63)] {
            let mut value = manifest();
            value.files[0].sha256 = hash;
            assert!(value.validate().is_err());
        }
        let mut value = manifest();
        value.files[0].compressed_sha256 = "C".repeat(64);
        assert!(value.validate().is_err());
        let mut value = manifest();
        value.format = FORMAT - 1;
        assert!(value.validate().is_err());
        let mut value = manifest();
        value.model_id = "lfm25-encoder-350m-onnx18-v1".into();
        assert!(value.validate().is_err());
        assert!(Manifest::parse(&vec![b' '; MAX_MANIFEST_BYTES as usize + 1]).is_err());
    }
    #[test]
    fn size_limits_cover_large_models_without_allowing_unbounded_native_assets() {
        for (index, bytes) in [
            (0, MAX_FILE_BYTES + 1),
            (1, MAX_TOKENIZER_BYTES + 1),
            (2, MAX_HELPER_BYTES + 1),
            (3, MAX_NATIVE_FILE_BYTES + 1),
        ] {
            let mut value = manifest();
            value.files[index].bytes = bytes;
            assert!(value.validate().is_err());
        }
        let mut value = manifest();
        value.files[0].bytes = 1_500_000_000;
        value.validate().unwrap();
    }
    #[test]
    fn paths_keep_each_architecture_together_and_shared_model_single() {
        let complete = manifest();
        assert_eq!(complete.files[0].archive_relative_path(), "liquid.onnx.zst");
        assert_eq!(
            complete.files[2].archive_relative_path(),
            "runtime/aarch64-apple-darwin/liquid-onnx.zst"
        );
        assert_eq!(
            complete.files[4].relative_path(),
            "runtime/x86_64-apple-darwin/liquid-onnx"
        );
        assert_eq!(minimum_macos(MACOS_TARGETS[0]), Some((14, 0)));
        assert_eq!(runtime_version(MACOS_TARGETS[1]), Some("1.19.2"));
    }
}
