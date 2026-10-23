//! The downloaded model and native runtime keep their pinned license notices.
use super::{
    Config, DOWNLOAD_BASE, check_cancelled, current_target, manifest, resources, verify_contents,
    verify_file_cancellable,
};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    io::{Seek, Write},
    path::Path,
    sync::atomic::AtomicBool,
};
const MAX_NOTICE_BYTES: u64 = 512 * 1024;
const MAX_TOTAL_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    format: u32,
    files: Vec<Notice>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Notice {
    name: String,
    asset_name: String,
    #[serde(default)]
    target: Option<String>,
    bytes: u64,
    sha256: String,
}
impl Catalog {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 16 * 1024 {
            return Err("Liquid's notice catalog is too large.".into());
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| "Liquid's notice catalog is invalid.".to_owned())?;
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), String> {
        let mut expected = BTreeSet::from([
            ("LICENSE.txt".to_owned(), "LICENSE.txt".to_owned(), None),
            (
                "PROVENANCE.json".to_owned(),
                "PROVENANCE.json".to_owned(),
                None,
            ),
        ]);
        for target in manifest::MACOS_TARGETS {
            expected.insert((
                format!("licenses/{target}.txt"),
                format!("{target}-LICENSES.txt"),
                Some(target.to_owned()),
            ));
        }
        let mut total = 0u64;
        for file in &self.files {
            if !expected.remove(&(
                file.name.clone(),
                file.asset_name.clone(),
                file.target.clone(),
            )) || file.bytes == 0
                || file.bytes > MAX_NOTICE_BYTES
                || file.sha256.len() != 64
                || !file
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err("Liquid's notice catalog has an invalid entry.".into());
            }
            total = total
                .checked_add(file.bytes)
                .ok_or("Liquid's notices are too large.")?;
        }
        if self.format != 1 || !expected.is_empty() || total > MAX_TOTAL_BYTES {
            return Err(
                "Liquid's notice catalog must contain its model and both runtime notices.".into(),
            );
        }
        Ok(())
    }
}

pub(super) fn prepare(config: &Config, cancel: &AtomicBool, force: bool) -> Result<(), String> {
    let catalog = Catalog::parse(include_bytes!("../resources/liquid/notices.json"))?;
    prepare_with(
        &catalog,
        &config.model_directory(),
        cancel,
        force,
        |file, output, token| {
            resources::download(
                &format!("{DOWNLOAD_BASE}{}", file.asset_name),
                file.bytes,
                &file.sha256,
                token,
                output,
            )
        },
    )
}
fn regular_directory(path: &Path) -> Result<(), String> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(format!(
                "Liquid setup cannot create its notices folder: {error}"
            ));
        }
    }
    if !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err("Liquid's notices folder must be a regular directory.".into());
    }
    Ok(())
}
fn prepare_with(
    catalog: &Catalog,
    root: &Path,
    cancel: &AtomicBool,
    force: bool,
    mut fetch: impl FnMut(&Notice, &mut fs::File, &AtomicBool) -> Result<(), String>,
) -> Result<(), String> {
    check_cancelled(cancel)?;
    catalog.validate()?;
    regular_directory(root)?;
    regular_directory(&root.join("licenses"))?;
    for file in catalog.files.iter().filter(|file| {
        file.target
            .as_deref()
            .is_none_or(|target| target == current_target())
    }) {
        check_cancelled(cancel)?;
        let target = root.join(&file.name);
        let ready = verify_file_cancellable(&target, file.bytes, &file.sha256, cancel).is_ok();
        check_cancelled(cancel)?;
        if ready && !force {
            continue;
        }
        let mut output = tempfile::NamedTempFile::new_in(root)
            .map_err(|error| format!("Liquid setup cannot prepare its notices: {error}"))?;
        fetch(file, output.as_file_mut(), cancel)?;
        output
            .flush()
            .and_then(|()| output.rewind())
            .map_err(|error| format!("Liquid setup cannot read its notices: {error}"))?;
        verify_contents(&mut output, file.bytes, &file.sha256, cancel)?;
        check_cancelled(cancel)?;
        if !ready {
            output
                .as_file()
                .sync_all()
                .map_err(|error| format!("Liquid setup cannot save its notices: {error}"))?;
            check_cancelled(cancel)?;
            output
                .persist(target)
                .map_err(|error| format!("Liquid setup cannot publish its notices: {error}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::sync::atomic::Ordering;
    fn fixture() -> Catalog {
        let mut catalog =
            Catalog::parse(include_bytes!("../resources/liquid/notices.json")).unwrap();
        for file in &mut catalog.files {
            file.bytes = 7;
            file.sha256 = format!("{:x}", Sha256::digest(b"fixture"));
        }
        catalog
    }
    #[test]
    fn catalog_requires_exact_names_targets_and_flat_assets_with_bounded_canonical_hashes() {
        let valid = fixture();
        valid.validate().unwrap();
        assert_eq!(valid.files.len(), 4);
        for mutate in [0, 1, 2, 3, 4, 5, 6] {
            let mut value = valid.clone();
            match mutate {
                0 => value.files[0].name = "../LICENSE.txt".into(),
                1 => value.files[2].target = Some("other".into()),
                2 => value.files[2].asset_name = "other-LICENSES.txt".into(),
                3 => value.files[0].bytes = MAX_NOTICE_BYTES + 1,
                4 => value.files[0].sha256 = "A".repeat(64),
                5 => {
                    value.files.pop();
                }
                6 => value.format = 2,
                _ => unreachable!(),
            }
            assert!(value.validate().is_err());
        }
        let mut large = valid.clone();
        for file in &mut large.files {
            file.bytes = MAX_NOTICE_BYTES;
        }
        assert!(large.validate().is_err());
    }
    #[test]
    fn missing_notices_fetch_only_shared_and_current_target_then_work_offline() {
        let root = tempfile::tempdir().unwrap();
        let catalog = fixture();
        let cancel = AtomicBool::new(false);
        let mut fetched = Vec::new();
        prepare_with(&catalog, root.path(), &cancel, false, |file, output, _| {
            fetched.push(file.name.clone());
            output.write_all(b"fixture").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fetched,
            [
                "LICENSE.txt".to_owned(),
                "PROVENANCE.json".to_owned(),
                format!("licenses/{}.txt", current_target())
            ]
        );
        for target in manifest::MACOS_TARGETS {
            assert_eq!(
                root.path().join(format!("licenses/{target}.txt")).exists(),
                target == current_target()
            );
        }
        prepare_with(&catalog, root.path(), &cancel, false, |_, _, _| {
            panic!("ready notices must be offline")
        })
        .unwrap();
    }
    #[test]
    fn existing_healthy_model_needs_only_three_small_missing_notices() {
        let source = tempfile::tempdir().unwrap();
        let model = super::super::tests::fixture(source.path());
        let root = tempfile::tempdir().unwrap();
        let config = super::super::tests::config(root.path());
        let cancel = AtomicBool::new(false);
        super::super::prepare_assets(&model, &config, &cancel, false, |file, output, _| {
            std::io::copy(
                &mut fs::File::open(source.path().join(file.archive_relative_path())).unwrap(),
                output,
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        let model_file = config.model_directory().join(manifest::MODEL_NAME);
        let modified = fs::metadata(&model_file).unwrap().modified().unwrap();
        for file in &model.files {
            fs::remove_file(source.path().join(file.archive_relative_path())).unwrap();
        }
        super::super::prepare_assets(&model, &config, &cancel, false, |_, _, _| {
            panic!("existing model must not redownload")
        })
        .unwrap();
        let mut fetched = Vec::new();
        prepare_with(
            &fixture(),
            &config.model_directory(),
            &cancel,
            false,
            |file, output, _| {
                fetched.push(file.asset_name.clone());
                output.write_all(b"fixture").unwrap();
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(fetched.len(), 3);
        assert!(fetched.iter().all(|name| !name.ends_with(".zst")));
        assert_eq!(
            fs::metadata(model_file).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    fn corrupt_or_cancelled_notice_download_never_replaces_existing_notice() {
        let root = tempfile::tempdir().unwrap();
        let catalog = fixture();
        let cancel = AtomicBool::new(false);
        prepare_with(&catalog, root.path(), &cancel, false, |_, output, _| {
            output.write_all(b"fixture").unwrap();
            Ok(())
        })
        .unwrap();
        let target = root.path().join("LICENSE.txt");
        let modified = fs::metadata(&target).unwrap().modified().unwrap();
        prepare_with(&catalog, root.path(), &cancel, true, |_, output, _| {
            output.write_all(b"fixture").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), modified);
        for cancelled in [false, true] {
            cancel.store(false, Ordering::Release);
            assert!(
                prepare_with(&catalog, root.path(), &cancel, true, |_, output, token| {
                    output.write_all(b"partial").unwrap();
                    if cancelled {
                        token.store(true, Ordering::Release);
                    }
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(fs::read(&target).unwrap(), b"fixture");
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 3);
        }
    }
    #[cfg(unix)]
    #[test]
    fn notice_directory_cannot_redirect_writes_outside_cache() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("licenses")).unwrap();
        assert!(
            prepare_with(
                &fixture(),
                root.path(),
                &AtomicBool::new(false),
                false,
                |_, _, _| panic!("must reject before download")
            )
            .unwrap_err()
            .contains("regular directory")
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
