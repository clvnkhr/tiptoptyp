//! Pinned, self-contained LTeX+ server and Java runtime package contract.
#![allow(dead_code)] // Runtime and packaging use different parts of this contract.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const FORMAT: u32 = 1;
pub const VERSION: &str = "18.7.0";
pub const JDK_DIRECTORY: &str = "jdk-21.0.10+7";
pub const MANIFEST_NAME: &str = "manifest.json";
pub const SHARED_NAME: &str = "shared.tar";
pub const LAUNCHER: &str = "bin/ltex-ls-plus";
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MACOS_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
pub const SOURCE_URL: &str = "https://github.com/ltex-plus/ltex-ls-plus/tree/18.7.0";

#[derive(Clone, Copy, Debug)]
pub struct Upstream {
    pub target: &'static str,
    pub archive: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}
impl Upstream {
    pub fn url(self) -> String {
        format!(
            "https://github.com/ltex-plus/ltex-ls-plus/releases/download/{VERSION}/{}",
            self.archive
        )
    }
}
pub fn upstream(target: &str) -> Option<Upstream> {
    match target {
        "aarch64-apple-darwin" => Some(Upstream {
            target: "aarch64-apple-darwin",
            archive: "ltex-ls-plus-18.7.0-mac-aarch64.tar.gz",
            bytes: 315_181_906,
            sha256: "2255eadb7be3b58b93d17efcd6fb10809e40e2b28289fdb1af966a874dac13e4",
        }),
        "x86_64-apple-darwin" => Some(Upstream {
            target: "x86_64-apple-darwin",
            archive: "ltex-ls-plus-18.7.0-mac-x64.tar.gz",
            bytes: 316_381_124,
            sha256: "90cf632a89604172b8170701bd04745dc73de4ac48d2eb4ced879bed532eaa4f",
        }),
        _ => None,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub version: String,
    pub jdk_directory: String,
    pub files: Vec<Asset>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    /// Raw tar filename; its packaged file appends `.zst`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub bytes: u64,
    pub sha256: String,
    pub compressed_bytes: u64,
    pub compressed_sha256: String,
}
impl Asset {
    pub fn archive_name(&self) -> String {
        format!("{}.zst", self.name)
    }
}
impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("LTeX+ manifest is too large".into());
        }
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|_| "LTeX+ manifest is invalid".to_owned())?;
        manifest.validate()?;
        Ok(manifest)
    }
    pub fn files_for_target<'a>(&'a self, target: &'a str) -> impl Iterator<Item = &'a Asset> + 'a {
        self.files
            .iter()
            .filter(move |file| file.target.as_deref().is_none_or(|value| value == target))
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT || self.version != VERSION || self.jdk_directory != JDK_DIRECTORY {
            return Err("LTeX+ package version is unsupported".into());
        }
        let mut seen = BTreeSet::new();
        let mut total = 0u64;
        for file in &self.files {
            let valid_name = match file.target.as_deref() {
                None => file.name == SHARED_NAME,
                Some(target) => {
                    MACOS_TARGETS.contains(&target) && file.name == format!("{target}.tar")
                }
            };
            if !valid_name
                || !seen.insert(file.target.as_deref())
                || !(1..=MAX_ARCHIVE_BYTES).contains(&file.bytes)
                || !(1..=MAX_ARCHIVE_BYTES).contains(&file.compressed_bytes)
                || !valid_hash(&file.sha256)
                || !valid_hash(&file.compressed_sha256)
            {
                return Err("LTeX+ archive entry is invalid".into());
            }
            total = total
                .checked_add(file.bytes)
                .ok_or("LTeX+ package is too large")?;
        }
        if !seen.contains(&None) || seen.len() < 2 || total > MAX_TOTAL_BYTES {
            return Err(
                "LTeX+ package must contain shared server files and a Mac Java runtime".into(),
            );
        }
        Ok(())
    }
    pub fn has_target(&self, target: &str) -> bool {
        self.files
            .iter()
            .any(|file| file.target.as_deref() == Some(target))
    }
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest() -> Manifest {
        Manifest {
            format: FORMAT,
            version: VERSION.into(),
            jdk_directory: JDK_DIRECTORY.into(),
            files: std::iter::once((None, SHARED_NAME.to_owned()))
                .chain(
                    MACOS_TARGETS.map(|target| (Some(target.to_owned()), format!("{target}.tar"))),
                )
                .map(|(target, name)| Asset {
                    name,
                    target,
                    bytes: 10,
                    sha256: "a".repeat(64),
                    compressed_bytes: 8,
                    compressed_sha256: "b".repeat(64),
                })
                .collect(),
        }
    }
    #[test]
    fn package_selects_shared_files_and_only_the_matching_java_runtime() {
        let package = manifest();
        package.validate().unwrap();
        for target in MACOS_TARGETS {
            assert!(package.has_target(target));
            let files: Vec<_> = package.files_for_target(target).collect();
            assert_eq!(files.len(), 2);
            assert_eq!(files[0].archive_name(), "shared.tar.zst");
            assert_eq!(files[1].archive_name(), format!("{target}.tar.zst"));
            assert!(
                upstream(target)
                    .unwrap()
                    .url()
                    .ends_with(upstream(target).unwrap().archive)
            );
        }
        let mut single = package.clone();
        single.files.pop();
        single.validate().unwrap();
        assert!(!single.has_target(MACOS_TARGETS[1]));
    }
    #[test]
    fn rejects_missing_shared_runtime_duplicates_paths_and_bad_hashes() {
        let package = manifest();
        for index in [0, 1] {
            let mut broken = package.clone();
            if index == 0 {
                broken.files.remove(0);
            } else {
                broken.files.truncate(1);
            }
            assert!(broken.validate().is_err());
        }
        let mut broken = package.clone();
        broken.files.push(broken.files[0].clone());
        assert!(broken.validate().is_err());
        for name in [
            "../shared.tar",
            "/shared.tar",
            "shared.tar.zst",
            "runtime.tar",
        ] {
            let mut broken = package.clone();
            broken.files[0].name = name.into();
            assert!(broken.validate().is_err());
        }
        for hash in ["A".repeat(64), "g".repeat(64), "a".repeat(63)] {
            let mut broken = package.clone();
            broken.files[0].sha256 = hash;
            assert!(broken.validate().is_err());
        }
        let mut broken = package.clone();
        broken.files[1].target = Some("../x86_64-apple-darwin".into());
        assert!(broken.validate().is_err());
        let mut broken = package;
        broken.jdk_directory = "../../java".into();
        assert!(broken.validate().is_err());
    }
}
