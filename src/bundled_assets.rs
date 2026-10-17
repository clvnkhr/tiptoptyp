//! Shared packaged fonts and handwriting models, without per-architecture copies.
//! Fonts are retained once before the first frame. Raw model bytes are temporary
//! and are loaded only by the existing handwriting worker.
use std::borrow::Cow;

#[path = "../build_support/bundled_assets.rs"]
mod registry;

macro_rules! define_runtime_assets {
    ($(($id:ident, $name:literal, $source:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum AssetId { $($id),+ }

        impl AssetId {
            const ALL: &[Self] = &[$(Self::$id),+];

            pub(crate) const fn name(self) -> &'static str {
                match self { $(Self::$id => $name),+ }
            }
        }

        #[cfg(not(all(feature = "production", not(test))))]
        fn embedded(asset: AssetId) -> &'static [u8] {
            match asset {
                $(AssetId::$id => include_bytes!(concat!("../", $source))),+
            }
        }

        #[cfg(all(feature = "production", not(test)))]
        fn retained(asset: AssetId) -> &'static std::sync::OnceLock<Result<Vec<u8>, String>> {
            match asset {
                $(AssetId::$id => {
                    static BYTES: std::sync::OnceLock<Result<Vec<u8>, String>> =
                        std::sync::OnceLock::new();
                    &BYTES
                }),+
            }
        }
    };
}
registry::shared_asset_registry!(define_runtime_assets);

impl AssetId {
    const fn is_font(self) -> bool {
        !matches!(self, Self::Detypify | Self::DetexifySamples)
    }
}

/// Validate the complete package layout, making fonts available before UI paint.
/// Model presence is checked without initializing or inflating either recognizer.
pub(crate) fn validate() -> Result<(), String> {
    for &asset in AssetId::ALL {
        if asset.is_font() {
            try_font_bytes(asset)?;
        } else {
            #[cfg(all(feature = "production", not(test)))]
            archive_path(asset)?;
        }
    }
    Ok(())
}

fn try_font_bytes(asset: AssetId) -> Result<&'static [u8], String> {
    debug_assert!(asset.is_font());
    #[cfg(not(all(feature = "production", not(test))))]
    {
        Ok(embedded(asset))
    }
    #[cfg(all(feature = "production", not(test)))]
    {
        retained(asset)
            .get_or_init(|| read_packaged(asset))
            .as_deref()
            .map_err(Clone::clone)
    }
}

/// Existing font definitions can keep borrowed bytes across theme changes.
pub(crate) fn font_bytes(asset: AssetId) -> &'static [u8] {
    try_font_bytes(asset)
        .unwrap_or_else(|error| panic!("Could not load bundled font {}: {error}", asset.name()))
}

/// The recognizer's existing OnceLock owns its decoded model/sample data. Avoid
/// retaining another large copy of the compressed resource's raw bytes afterward.
pub(crate) fn model_bytes(asset: AssetId) -> Result<Cow<'static, [u8]>, String> {
    debug_assert!(!asset.is_font());
    #[cfg(not(all(feature = "production", not(test))))]
    {
        Ok(Cow::Borrowed(embedded(asset)))
    }
    #[cfg(all(feature = "production", not(test)))]
    {
        read_packaged(asset).map(Cow::Owned)
    }
}

#[cfg(all(feature = "production", not(test)))]
fn archive_path(asset: AssetId) -> Result<std::path::PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    for root in crate::bundled_payload::resource_roots(&executable) {
        let root = root.join("assets/shared");
        let archive = root.join(format!("{}.gz", asset.name()));
        if archive.exists() {
            if crate::bundled_payload::is_packaged_payload(&archive) {
                return Ok(archive);
            }
            return Err(format!(
                "Bundled asset {} has an invalid manifest or archive. Reinstall the complete app.",
                asset.name(),
            ));
        }
    }
    Err(format!(
        "Bundled asset {} is missing. Reinstall the complete app.",
        asset.name(),
    ))
}

#[cfg(all(feature = "production", not(test)))]
fn read_packaged(asset: AssetId) -> Result<Vec<u8>, String> {
    let archive = archive_path(asset)?;
    let path = crate::bundled_payload::prepare_shared_asset(&archive)
        .map_err(|error| format!("Could not prepare bundled asset {}: {error}", asset.name()))?;
    std::fs::read(path)
        .map_err(|error| format!("Could not read bundled asset {}: {error}", asset.name()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn runtime_and_package_registry_cover_the_same_unique_assets() {
        assert_eq!(AssetId::ALL.len(), 13);
        assert_eq!(registry::SHARED_ASSETS.len(), AssetId::ALL.len());
        let mut names = HashSet::new();
        for (&asset, specification) in AssetId::ALL.iter().zip(registry::SHARED_ASSETS) {
            assert_eq!(asset.name(), specification.name);
            assert!(names.insert(specification.name));
            assert!(specification.name.is_ascii());
            assert!(specification.source.starts_with("assets/"));
            assert_eq!(
                std::fs::read(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(specification.source)
                )
                .unwrap(),
                embedded(asset),
            );
        }
        assert_eq!(
            AssetId::ALL.iter().filter(|asset| asset.is_font()).count(),
            11
        );
    }

    #[test]
    fn font_bytes_are_reused_and_models_keep_the_original_development_bytes() {
        validate().unwrap();
        for &asset in AssetId::ALL.iter().filter(|asset| asset.is_font()) {
            assert!(std::ptr::eq(font_bytes(asset), font_bytes(asset)));
            assert!(!font_bytes(asset).is_empty());
        }
        for asset in [AssetId::Detypify, AssetId::DetexifySamples] {
            let bytes = model_bytes(asset).unwrap();
            assert!(matches!(bytes, Cow::Borrowed(_)));
            assert_eq!(&*bytes, embedded(asset));
        }
    }
}
