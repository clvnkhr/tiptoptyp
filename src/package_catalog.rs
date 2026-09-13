//! Discovery and merging for installed and published Typst packages.
//!
//! The catalog owns no UI or worker state. Local scanning and the blocking
//! registry request are intended to run away from the UI thread.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use typst_syntax::package::{PackageManifest, PackageSpec, PackageVersion};

const OFFICIAL_INDEX_URL: &str = "https://packages.typst.org/preview/index.json";
const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_INDEX_BYTES: u64 = 32 * 1024 * 1024;
const PACKAGES_SUBDIRECTORY: &str = "typst/packages";
const OFFICIAL_NAMESPACE: &str = "preview";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackageRootKind {
    Data,
    Cache,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageRoot {
    pub(crate) path: PathBuf,
    pub(crate) kind: PackageRootKind,
    pub(crate) custom: bool,
}

impl PackageRoot {
    #[cfg(test)]
    fn data(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            kind: PackageRootKind::Data,
            custom: true,
        }
    }

    #[cfg(test)]
    fn cache(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            kind: PackageRootKind::Cache,
            custom: true,
        }
    }

    fn standard(path: PathBuf, kind: PackageRootKind) -> Self {
        Self {
            path,
            kind,
            custom: false,
        }
    }
}

/// Ordered Typst package roots. Data roots precede cache roots, matching
/// Typst's package lookup precedence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PackageRoots {
    roots: Vec<PackageRoot>,
}

impl PackageRoots {
    pub(crate) fn standard() -> Self {
        Self::with_custom(Vec::new(), Vec::new())
    }

    /// Adds explicit package roots alongside the platform defaults. Arguments
    /// point at the directory containing namespace folders (for example,
    /// `/custom/packages`, not its parent data directory).
    pub(crate) fn with_custom(
        custom_data_roots: Vec<PathBuf>,
        custom_cache_roots: Vec<PathBuf>,
    ) -> Self {
        let mut roots = Vec::new();
        let standard = standard_package_roots();
        append_custom_roots(&mut roots, custom_data_roots, PackageRootKind::Data);
        roots.extend(
            standard
                .iter()
                .filter(|root| root.kind == PackageRootKind::Data)
                .cloned(),
        );
        append_custom_roots(&mut roots, custom_cache_roots, PackageRootKind::Cache);
        roots.extend(
            standard
                .into_iter()
                .filter(|root| root.kind == PackageRootKind::Cache),
        );
        Self {
            roots: deduplicate_roots(roots),
        }
    }

    #[cfg(test)]
    fn from_roots(roots: Vec<PackageRoot>) -> Self {
        Self {
            roots: deduplicate_roots(roots),
        }
    }

    fn roots(&self) -> &[PackageRoot] {
        &self.roots
    }
}

fn append_custom_roots(
    roots: &mut Vec<PackageRoot>,
    mut paths: Vec<PathBuf>,
    kind: PackageRootKind,
) {
    paths.sort();
    roots.extend(paths.into_iter().map(|path| PackageRoot {
        path,
        kind,
        custom: true,
    }));
}

fn deduplicate_roots(roots: Vec<PackageRoot>) -> Vec<PackageRoot> {
    let mut seen = BTreeSet::new();
    roots
        .into_iter()
        .filter(|root| seen.insert(root.path.clone()))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum Platform {
    Linux,
    MacOs,
    Windows,
}

fn standard_package_roots() -> Vec<PackageRoot> {
    #[cfg(target_os = "macos")]
    let platform = Platform::MacOs;
    #[cfg(target_os = "windows")]
    let platform = Platform::Windows;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let platform = Platform::Linux;

    standard_package_roots_for(platform, |key| env::var_os(key))
}

fn standard_package_roots_for(
    platform: Platform,
    get_environment: impl Fn(&str) -> Option<OsString>,
) -> Vec<PackageRoot> {
    let home = || nonempty_environment(&get_environment, "HOME").map(PathBuf::from);
    let user_profile = || {
        nonempty_environment(&get_environment, "USERPROFILE")
            .map(PathBuf::from)
            .or_else(home)
    };

    let (data, cache) = match platform {
        Platform::Linux => {
            let data = absolute_environment_path(&get_environment, "XDG_DATA_HOME")
                .or_else(|| home().map(|path| path.join(".local/share")));
            let cache = absolute_environment_path(&get_environment, "XDG_CACHE_HOME")
                .or_else(|| home().map(|path| path.join(".cache")));
            (data, cache)
        }
        Platform::MacOs => (
            home().map(|path| path.join("Library/Application Support")),
            home().map(|path| path.join("Library/Caches")),
        ),
        Platform::Windows => (
            nonempty_environment(&get_environment, "APPDATA")
                .map(PathBuf::from)
                .or_else(|| user_profile().map(|path| path.join("AppData/Roaming"))),
            nonempty_environment(&get_environment, "LOCALAPPDATA")
                .map(PathBuf::from)
                .or_else(|| user_profile().map(|path| path.join("AppData/Local"))),
        ),
    };

    [
        (data, PackageRootKind::Data),
        (cache, PackageRootKind::Cache),
    ]
    .into_iter()
    .filter_map(|(base, kind)| {
        base.map(|path| PackageRoot::standard(path.join(PACKAGES_SUBDIRECTORY), kind))
    })
    .collect()
}

fn nonempty_environment(
    get_environment: &impl Fn(&str) -> Option<OsString>,
    key: &str,
) -> Option<OsString> {
    get_environment(key).filter(|value| !value.is_empty())
}

fn absolute_environment_path(
    get_environment: &impl Fn(&str) -> Option<OsString>,
    key: &str,
) -> Option<PathBuf> {
    nonempty_environment(get_environment, key)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PackageMetadata {
    pub(crate) entrypoint: String,
    pub(crate) authors: Vec<String>,
    pub(crate) license: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) homepage: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) categories: Vec<String>,
    pub(crate) disciplines: Vec<String>,
    pub(crate) compiler: Option<String>,
    pub(crate) is_template: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageInstallation {
    pub(crate) package_path: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) root: PackageRoot,
}

/// Remove exactly one installed release, never a root or a linked directory.
pub(crate) fn uninstall(installation: &PackageInstallation) -> Result<(), String> {
    let relative = installation
        .package_path
        .strip_prefix(&installation.root.path)
        .map_err(|_| "Package is outside its root")?;
    let parts = relative
        .components()
        .map(|part| match part {
            std::path::Component::Normal(name) => name.to_str().ok_or("Invalid package path"),
            _ => Err("Invalid package path"),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if parts.len() != 3 {
        return Err("Expected one namespace/name/version directory".into());
    }
    let root = installation
        .root
        .path
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let canonical = installation
        .package_path
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if canonical != root.join(relative) || !canonical.starts_with(&root) {
        return Err("Refusing to uninstall through a symlink".into());
    }
    for ancestor in [
        canonical.clone(),
        canonical.parent().unwrap().to_path_buf(),
        canonical.parent().unwrap().parent().unwrap().to_path_buf(),
    ] {
        if std::fs::symlink_metadata(ancestor)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Refusing to uninstall through a symlink".into());
        }
    }
    parse_installed_package(
        &installation.root,
        parts[0],
        parts[1],
        parts[2],
        &installation.package_path,
    )
    .map_err(|e| format!("Package verification failed: {}", e.message))?;
    std::fs::remove_dir_all(&installation.package_path).map_err(|e| e.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InstalledPackage {
    namespace: String,
    name: String,
    version: PackageVersion,
    metadata: PackageMetadata,
    installation: PackageInstallation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AvailablePackage {
    name: String,
    version: PackageVersion,
    metadata: PackageMetadata,
    updated_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageScanWarning {
    pub(crate) path: PathBuf,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct InstalledPackageScan {
    packages: Vec<InstalledPackage>,
    warnings: Vec<PackageScanWarning>,
}

fn scan_installed_packages(roots: &PackageRoots) -> InstalledPackageScan {
    let mut scan = InstalledPackageScan::default();
    for root in roots.roots() {
        scan_package_root(root, &mut scan);
    }
    scan.packages.sort_by(installed_package_order);
    scan
}

fn installed_package_order(left: &InstalledPackage, right: &InstalledPackage) -> Ordering {
    left.namespace
        .cmp(&right.namespace)
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| right.version.cmp(&left.version))
        .then_with(|| {
            root_priority(&left.installation.root).cmp(&root_priority(&right.installation.root))
        })
        .then_with(|| {
            left.installation
                .package_path
                .cmp(&right.installation.package_path)
        })
}

fn scan_package_root(root: &PackageRoot, scan: &mut InstalledPackageScan) {
    let Some(namespaces) = directory_children(&root.path, &mut scan.warnings) else {
        return;
    };
    for namespace_path in namespaces {
        let Some(namespace) = utf8_file_name(&namespace_path, &mut scan.warnings) else {
            continue;
        };
        let Some(packages) = directory_children(&namespace_path, &mut scan.warnings) else {
            continue;
        };
        for package_path in packages {
            let Some(name) = utf8_file_name(&package_path, &mut scan.warnings) else {
                continue;
            };
            let Some(versions) = directory_children(&package_path, &mut scan.warnings) else {
                continue;
            };
            for version_path in versions {
                let Some(version) = utf8_file_name(&version_path, &mut scan.warnings) else {
                    continue;
                };
                match parse_installed_package(root, &namespace, &name, &version, &version_path) {
                    Ok(package) => scan.packages.push(package),
                    Err(warning) => scan.warnings.push(warning),
                }
            }
        }
    }
}

fn directory_children(
    directory: &Path,
    warnings: &mut Vec<PackageScanWarning>,
) -> Option<Vec<PathBuf>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            warnings.push(PackageScanWarning {
                path: directory.to_path_buf(),
                message: format!("could not read directory: {error}"),
            });
            return None;
        }
    };
    let mut children = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => match entry.file_type() {
                Ok(file_type) if file_type.is_dir() || file_type.is_symlink() => {
                    if entry.path().is_dir() {
                        children.push(entry.path());
                    }
                }
                Ok(_) => {}
                Err(error) => warnings.push(PackageScanWarning {
                    path: entry.path(),
                    message: format!("could not inspect directory entry: {error}"),
                }),
            },
            Err(error) => warnings.push(PackageScanWarning {
                path: directory.to_path_buf(),
                message: format!("could not read directory entry: {error}"),
            }),
        }
    }
    children.sort();
    Some(children)
}

fn utf8_file_name(path: &Path, warnings: &mut Vec<PackageScanWarning>) -> Option<String> {
    match path.file_name().and_then(OsStr::to_str) {
        Some(name) => Some(name.to_owned()),
        None => {
            warnings.push(PackageScanWarning {
                path: path.to_path_buf(),
                message: "package path component is not valid UTF-8".to_owned(),
            });
            None
        }
    }
}

fn parse_installed_package(
    root: &PackageRoot,
    namespace: &str,
    name: &str,
    version: &str,
    package_path: &Path,
) -> Result<InstalledPackage, PackageScanWarning> {
    let manifest_path = package_path.join("typst.toml");
    let spec =
        parse_package_spec(namespace, name, version).map_err(|message| PackageScanWarning {
            path: package_path.to_path_buf(),
            message,
        })?;
    let source = fs::read_to_string(&manifest_path).map_err(|error| PackageScanWarning {
        path: manifest_path.clone(),
        message: format!("could not read package manifest: {error}"),
    })?;
    let manifest =
        toml::from_str::<PackageManifest>(&source).map_err(|error| PackageScanWarning {
            path: manifest_path.clone(),
            message: format!("could not parse package manifest: {error}"),
        })?;
    if manifest.package.name.as_str() != spec.name.as_str() {
        return Err(PackageScanWarning {
            path: manifest_path,
            message: format!(
                "manifest name {:?} does not match package directory {:?}",
                manifest.package.name, spec.name
            ),
        });
    }
    if manifest.package.version != spec.version {
        return Err(PackageScanWarning {
            path: manifest_path,
            message: format!(
                "manifest version {} does not match package directory {}",
                manifest.package.version, spec.version
            ),
        });
    }

    Ok(InstalledPackage {
        namespace: spec.namespace.to_string(),
        name: spec.name.to_string(),
        version: spec.version,
        metadata: metadata_from_manifest(&manifest),
        installation: PackageInstallation {
            package_path: package_path.to_path_buf(),
            manifest_path,
            root: root.clone(),
        },
    })
}

fn parse_package_spec(namespace: &str, name: &str, version: &str) -> Result<PackageSpec, String> {
    format!("@{namespace}/{name}:{version}")
        .parse::<PackageSpec>()
        .map_err(|error| format!("invalid package path: {error}"))
}

fn metadata_from_manifest(manifest: &PackageManifest) -> PackageMetadata {
    let package = &manifest.package;
    PackageMetadata {
        entrypoint: package.entrypoint.to_string(),
        authors: package.authors.iter().map(ToString::to_string).collect(),
        license: package.license.as_ref().map(ToString::to_string),
        description: package.description.as_ref().map(ToString::to_string),
        homepage: package.homepage.as_ref().map(ToString::to_string),
        repository: package.repository.as_ref().map(ToString::to_string),
        keywords: package.keywords.iter().map(ToString::to_string).collect(),
        categories: package.categories.iter().map(ToString::to_string).collect(),
        disciplines: package
            .disciplines
            .iter()
            .map(ToString::to_string)
            .collect(),
        compiler: package.compiler.map(|version| version.to_string()),
        is_template: manifest.template.is_some(),
    }
}

#[derive(Debug, Deserialize)]
struct OfficialIndexEntry {
    name: String,
    version: String,
    entrypoint: String,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    disciplines: Vec<String>,
    #[serde(default)]
    compiler: Option<String>,
    #[serde(default)]
    template: Option<OfficialTemplate>,
    #[serde(default, rename = "updatedAt")]
    updated_at: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OfficialTemplate {
    #[allow(dead_code)]
    path: String,
    #[allow(dead_code)]
    entrypoint: String,
    #[serde(default)]
    #[allow(dead_code)]
    thumbnail: Option<String>,
}

fn parse_official_index(source: &str) -> Result<Vec<AvailablePackage>, CatalogError> {
    let entries = serde_json::from_str::<Vec<OfficialIndexEntry>>(source)
        .map_err(|error| CatalogError::index(format!("invalid official package index: {error}")))?;
    let mut seen = BTreeSet::new();
    let mut packages = Vec::with_capacity(entries.len());
    for entry in entries {
        let spec = parse_package_spec(OFFICIAL_NAMESPACE, &entry.name, &entry.version)
            .map_err(CatalogError::index)?;
        if !seen.insert((spec.name.to_string(), spec.version)) {
            return Err(CatalogError::index(format!(
                "duplicate official package {}:{}",
                spec.name, spec.version
            )));
        }
        packages.push(AvailablePackage {
            name: spec.name.to_string(),
            version: spec.version,
            metadata: PackageMetadata {
                entrypoint: entry.entrypoint,
                authors: entry.authors,
                license: entry.license,
                description: entry.description,
                homepage: entry.homepage,
                repository: entry.repository,
                keywords: entry.keywords,
                categories: entry.categories,
                disciplines: entry.disciplines,
                compiler: entry.compiler,
                is_template: entry.template.is_some(),
            },
            updated_at: entry.updated_at,
        });
    }
    packages.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| right.version.cmp(&left.version))
    });
    Ok(packages)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogErrorKind {
    Network,
    Index,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogError {
    pub(crate) kind: CatalogErrorKind,
    pub(crate) message: String,
}

impl CatalogError {
    fn network(message: impl Into<String>) -> Self {
        Self {
            kind: CatalogErrorKind::Network,
            message: message.into(),
        }
    }

    fn index(message: impl Into<String>) -> Self {
        Self {
            kind: CatalogErrorKind::Index,
            message: message.into(),
        }
    }
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CatalogError {}

fn fetch_official_index() -> Result<Vec<AvailablePackage>, CatalogError> {
    fetch_official_index_from(OFFICIAL_INDEX_URL, DEFAULT_FETCH_TIMEOUT)
}

fn fetch_official_index_from(
    url: &str,
    timeout: Duration,
) -> Result<Vec<AvailablePackage>, CatalogError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(timeout)
        .timeout_connect(timeout)
        .build();
    let response = agent.get(url).call().map_err(|error| {
        CatalogError::network(format!("could not fetch package index: {error}"))
    })?;
    let body = read_index_body(response.into_reader(), MAX_INDEX_BYTES)?;
    parse_official_index(&body)
}

fn read_index_body(reader: impl Read, limit: u64) -> Result<String, CatalogError> {
    let mut body = String::new();
    reader
        .take(limit + 1)
        .read_to_string(&mut body)
        .map_err(|error| CatalogError::network(format!("could not read package index: {error}")))?;
    if body.len() as u64 > limit {
        return Err(CatalogError::network(format!(
            "package index exceeds the {limit}-byte response limit"
        )));
    }
    Ok(body)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageRelease {
    pub(crate) version: PackageVersion,
    pub(crate) available: bool,
    pub(crate) installations: Vec<PackageInstallation>,
    pub(crate) metadata: PackageMetadata,
    pub(crate) updated_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageRecord {
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) latest_available: Option<PackageVersion>,
    pub(crate) releases: Vec<PackageRelease>,
}

impl PackageRecord {
    pub(crate) fn is_installed(&self) -> bool {
        self.releases
            .iter()
            .any(|release| !release.installations.is_empty())
    }

    pub(crate) fn latest_installed(&self) -> Option<PackageVersion> {
        self.releases
            .iter()
            .filter(|release| !release.installations.is_empty())
            .map(|release| release.version)
            .max()
    }

    /// Every locally present release, newest first. Keeping this separate
    /// from `display_release` is important when the registry advertises a
    /// newer version: the package browser must still enumerate all older
    /// local copies and every root that contains them.
    pub(crate) fn installed_releases(&self) -> impl Iterator<Item = &PackageRelease> {
        self.releases
            .iter()
            .filter(|release| !release.installations.is_empty())
    }

    pub(crate) fn has_update(&self) -> bool {
        self.latest_available
            .zip(self.latest_installed())
            .is_some_and(|(available, installed)| available > installed)
    }

    pub(crate) fn display_release(&self) -> Option<&PackageRelease> {
        self.latest_available
            .and_then(|version| {
                self.releases
                    .iter()
                    .find(|release| release.version == version)
            })
            .or_else(|| self.releases.first())
    }

    fn matches_term(&self, term: &str) -> bool {
        let matches = |candidate: &str| candidate.to_lowercase().contains(term);
        matches(&self.namespace)
            || matches(&self.name)
            || matches(&format!("@{}/{}", self.namespace, self.name))
            || self.releases.iter().any(|release| {
                matches(&release.version.to_string())
                    || matches(&release.metadata.entrypoint)
                    || release
                        .metadata
                        .description
                        .as_deref()
                        .is_some_and(&matches)
                    || release.metadata.authors.iter().any(|value| matches(value))
                    || release.metadata.keywords.iter().any(|value| matches(value))
                    || release
                        .metadata
                        .categories
                        .iter()
                        .any(|value| matches(value))
                    || release
                        .metadata
                        .disciplines
                        .iter()
                        .any(|value| matches(value))
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PackageCatalog {
    packages: Vec<PackageRecord>,
}

impl PackageCatalog {
    fn from_sources(
        mut installed: Vec<InstalledPackage>,
        available: Vec<AvailablePackage>,
    ) -> Self {
        let mut packages =
            BTreeMap::<(String, String), BTreeMap<PackageVersion, PackageRelease>>::new();
        installed.sort_by(installed_package_order);
        for package in installed {
            let releases = packages
                .entry((package.namespace, package.name))
                .or_default();
            let release = releases
                .entry(package.version)
                .or_insert_with(|| PackageRelease {
                    version: package.version,
                    available: false,
                    installations: Vec::new(),
                    metadata: package.metadata.clone(),
                    updated_at: None,
                });
            if !release
                .installations
                .iter()
                .any(|installed| installed.package_path == package.installation.package_path)
            {
                release.installations.push(package.installation);
            }
        }
        for package in available {
            let releases = packages
                .entry((OFFICIAL_NAMESPACE.to_owned(), package.name))
                .or_default();
            let release = releases
                .entry(package.version)
                .or_insert_with(|| PackageRelease {
                    version: package.version,
                    available: true,
                    installations: Vec::new(),
                    metadata: package.metadata.clone(),
                    updated_at: package.updated_at,
                });
            release.available = true;
            release.metadata = package.metadata;
            release.updated_at = package.updated_at;
        }

        let packages = packages
            .into_iter()
            .map(|((namespace, name), releases)| {
                let mut releases = releases.into_values().collect::<Vec<_>>();
                releases.sort_by_key(|release| std::cmp::Reverse(release.version));
                for release in &mut releases {
                    release.installations.sort_by(|left, right| {
                        root_priority(&left.root)
                            .cmp(&root_priority(&right.root))
                            .then_with(|| left.package_path.cmp(&right.package_path))
                    });
                }
                let latest_available = releases
                    .iter()
                    .filter(|release| release.available)
                    .map(|release| release.version)
                    .max();
                PackageRecord {
                    namespace,
                    name,
                    latest_available,
                    releases,
                }
            })
            .collect();
        Self { packages }
    }

    #[cfg(test)]
    pub(crate) fn packages(&self) -> &[PackageRecord] {
        &self.packages
    }

    /// Case-insensitive, whitespace-separated AND filtering over package
    /// identity and searchable manifest metadata.
    pub(crate) fn filtered(&self, query: &str) -> Vec<&PackageRecord> {
        let terms = query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        self.packages
            .iter()
            .filter(|package| terms.iter().all(|term| package.matches_term(term)))
            .collect()
    }
}

fn root_priority(root: &PackageRoot) -> (u8, u8) {
    let kind = match root.kind {
        PackageRootKind::Data => 0,
        PackageRootKind::Cache => 1,
    };
    (kind, u8::from(!root.custom))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageCatalogLoad {
    pub(crate) catalog: PackageCatalog,
    pub(crate) warnings: Vec<PackageScanWarning>,
    /// A registry failure does not discard installed packages.
    pub(crate) official_index_error: Option<CatalogError>,
}

impl PackageCatalogLoad {
    /// Load the local view without waiting for the published registry. The UI
    /// can show installed packages immediately while `load` fetches the
    /// optional online index in the background.
    pub(crate) fn load_installed(roots: &PackageRoots) -> Self {
        let scan = scan_installed_packages(roots);
        Self {
            catalog: PackageCatalog::from_sources(scan.packages, Vec::new()),
            warnings: scan.warnings,
            official_index_error: None,
        }
    }

    pub(crate) fn load(roots: &PackageRoots) -> Self {
        load_catalog_with(roots, fetch_official_index)
    }
}

fn load_catalog_with(
    roots: &PackageRoots,
    fetch: impl FnOnce() -> Result<Vec<AvailablePackage>, CatalogError>,
) -> PackageCatalogLoad {
    let scan = scan_installed_packages(roots);
    let (available, official_index_error) = match fetch() {
        Ok(packages) => (packages, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    PackageCatalogLoad {
        catalog: PackageCatalog::from_sources(scan.packages, available),
        warnings: scan.warnings,
        official_index_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, io::Cursor};

    fn environment(values: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
            .collect::<HashMap<_, _>>();
        move |key| values.get(key).cloned()
    }

    fn package_manifest(name: &str, version: &str, description: &str) -> String {
        format!(
            r#"[package]
name = "{name}"
version = "{version}"
entrypoint = "lib.typ"
authors = ["A. Writer"]
license = "MIT"
description = "{description}"
keywords = ["diagram"]
categories = ["visualization"]

[tool.catalog-test]
enabled = true
"#
        )
    }

    fn install(root: &Path, namespace: &str, name: &str, version: &str, manifest: &str) {
        let package = root.join(namespace).join(name).join(version);
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("typst.toml"), manifest).unwrap();
        fs::write(package.join("lib.typ"), "#let value = 1").unwrap();
    }

    fn version(source: &str) -> PackageVersion {
        source.parse().unwrap()
    }

    #[test]
    fn uninstall_removes_one_verified_release_and_preserves_other_versions() {
        let temp = tempfile::tempdir().unwrap();
        for version in ["1.0.0", "2.0.0"] {
            install(
                temp.path(),
                "local",
                "sample",
                version,
                &package_manifest("sample", version, "Sample"),
            );
        }
        let root = PackageRoot::data(temp.path());
        let package_path = temp.path().join("local/sample/1.0.0");
        let installation = PackageInstallation {
            manifest_path: package_path.join("typst.toml"),
            package_path: package_path.clone(),
            root,
        };
        let mut invalid = installation.clone();
        invalid.package_path = temp.path().join("local/sample");
        assert!(uninstall(&invalid).is_err());
        uninstall(&installation).unwrap();
        assert!(!package_path.exists());
        assert!(temp.path().join("local/sample/2.0.0/lib.typ").exists());
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_refuses_linked_releases_and_parent_directories() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let root = temp.path().join("root");
        install(
            &outside,
            "local",
            "sample",
            "1.0.0",
            &package_manifest("sample", "1.0.0", "Sample"),
        );
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(outside.join("local"), root.join("local")).unwrap();
        let package_path = root.join("local/sample/1.0.0");
        let installation = PackageInstallation {
            manifest_path: package_path.join("typst.toml"),
            package_path,
            root: PackageRoot::data(root),
        };
        assert!(uninstall(&installation).is_err());
        assert!(outside.join("local/sample/1.0.0/lib.typ").exists());
    }

    #[test]
    fn standard_linux_roots_honor_absolute_xdg_paths() {
        let roots = standard_package_roots_for(
            Platform::Linux,
            environment(&[
                ("HOME", "/home/alice"),
                ("XDG_DATA_HOME", "/data"),
                ("XDG_CACHE_HOME", "/cache"),
            ]),
        );
        assert_eq!(
            roots,
            vec![
                PackageRoot::standard(PathBuf::from("/data/typst/packages"), PackageRootKind::Data,),
                PackageRoot::standard(
                    PathBuf::from("/cache/typst/packages"),
                    PackageRootKind::Cache,
                ),
            ]
        );
    }

    #[test]
    fn relative_or_empty_xdg_paths_fall_back_to_home() {
        let roots = standard_package_roots_for(
            Platform::Linux,
            environment(&[
                ("HOME", "/home/alice"),
                ("XDG_DATA_HOME", "relative"),
                ("XDG_CACHE_HOME", ""),
            ]),
        );
        assert_eq!(
            roots[0].path,
            PathBuf::from("/home/alice/.local/share/typst/packages")
        );
        assert_eq!(
            roots[1].path,
            PathBuf::from("/home/alice/.cache/typst/packages")
        );
    }

    #[test]
    fn standard_macos_roots_use_application_support_and_caches() {
        let roots =
            standard_package_roots_for(Platform::MacOs, environment(&[("HOME", "/Users/alice")]));
        assert_eq!(
            roots
                .iter()
                .map(|root| root.path.as_path())
                .collect::<Vec<_>>(),
            [
                Path::new("/Users/alice/Library/Application Support/typst/packages"),
                Path::new("/Users/alice/Library/Caches/typst/packages"),
            ]
        );
    }

    #[test]
    fn standard_windows_roots_honor_roaming_and_local_app_data() {
        let roots = standard_package_roots_for(
            Platform::Windows,
            environment(&[
                ("APPDATA", "C:\\Users\\alice\\AppData\\Roaming"),
                ("LOCALAPPDATA", "C:\\Users\\alice\\AppData\\Local"),
            ]),
        );
        assert_eq!(
            roots,
            vec![
                PackageRoot::standard(
                    PathBuf::from("C:\\Users\\alice\\AppData\\Roaming").join("typst/packages"),
                    PackageRootKind::Data,
                ),
                PackageRoot::standard(
                    PathBuf::from("C:\\Users\\alice\\AppData\\Local").join("typst/packages"),
                    PackageRootKind::Cache,
                ),
            ]
        );
    }

    #[test]
    fn windows_roots_fall_back_to_the_user_profile_and_missing_home_is_safe() {
        let roots = standard_package_roots_for(
            Platform::Windows,
            environment(&[("USERPROFILE", "C:\\Users\\alice")]),
        );
        assert_eq!(
            roots
                .iter()
                .map(|root| root.path.clone())
                .collect::<Vec<_>>(),
            [
                PathBuf::from("C:\\Users\\alice")
                    .join("AppData/Roaming")
                    .join(PACKAGES_SUBDIRECTORY),
                PathBuf::from("C:\\Users\\alice")
                    .join("AppData/Local")
                    .join(PACKAGES_SUBDIRECTORY),
            ]
        );
        assert!(standard_package_roots_for(Platform::MacOs, environment(&[])).is_empty());
        assert!(standard_package_roots_for(Platform::Linux, environment(&[])).is_empty());
    }

    #[test]
    fn custom_roots_are_deterministic_precede_defaults_and_are_deduplicated() {
        let roots = PackageRoots::with_custom(
            vec![PathBuf::from("/z-data"), PathBuf::from("/a-data")],
            vec![PathBuf::from("/cache")],
        );
        assert_eq!(roots.roots()[0].path, Path::new("/a-data"));
        assert_eq!(roots.roots()[1].path, Path::new("/z-data"));
        assert!(roots.roots()[0].custom);
        assert_eq!(roots.roots()[0].kind, PackageRootKind::Data);
        assert!(
            roots
                .roots()
                .windows(2)
                .all(|roots| roots[0].kind != PackageRootKind::Cache
                    || roots[1].kind == PackageRootKind::Cache)
        );

        let duplicate = PackageRoots::from_roots(vec![
            PackageRoot::data("/same"),
            PackageRoot::cache("/same"),
        ]);
        assert_eq!(duplicate.roots().len(), 1);
        assert_eq!(duplicate.roots()[0].kind, PackageRootKind::Data);
    }

    #[test]
    fn installed_scan_reads_manifests_and_orders_records_deterministically() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        install(
            &data,
            "local",
            "zeta",
            "1.0.0",
            &package_manifest("zeta", "1.0.0", "Last"),
        );
        install(
            &data,
            "preview",
            "alpha",
            "1.1.0",
            &package_manifest("alpha", "1.1.0", "New"),
        );
        install(
            &data,
            "preview",
            "alpha",
            "1.0.0",
            &package_manifest("alpha", "1.0.0", "Old"),
        );

        let scan =
            scan_installed_packages(&PackageRoots::from_roots(vec![PackageRoot::data(&data)]));
        assert!(scan.warnings.is_empty());
        assert_eq!(
            scan.packages
                .iter()
                .map(|package| format!(
                    "@{}/{}:{}",
                    package.namespace, package.name, package.version
                ))
                .collect::<Vec<_>>(),
            [
                "@local/zeta:1.0.0",
                "@preview/alpha:1.1.0",
                "@preview/alpha:1.0.0",
            ]
        );
        assert_eq!(
            scan.packages[1].metadata.description.as_deref(),
            Some("New")
        );
        assert_eq!(scan.packages[1].metadata.authors, ["A. Writer"]);
    }

    #[cfg(unix)]
    #[test]
    fn installed_scan_follows_a_symlinked_namespace_directory() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let packages = directory.path().join("packages");
        let checkout = directory.path().join("package-checkout");
        install(
            &checkout,
            "preview",
            "linked",
            "1.0.0",
            &package_manifest("linked", "1.0.0", "Linked checkout"),
        );
        fs::create_dir_all(&packages).unwrap();
        symlink(checkout.join("preview"), packages.join("preview")).unwrap();

        let scan = scan_installed_packages(&PackageRoots::from_roots(vec![PackageRoot::data(
            &packages,
        )]));
        assert!(scan.warnings.is_empty());
        assert_eq!(scan.packages.len(), 1);
        assert_eq!(scan.packages[0].name, "linked");
    }

    #[test]
    fn installed_scan_reports_invalid_manifests_and_path_mismatches() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        install(root, "preview", "broken", "1.0.0", "not toml");
        install(
            root,
            "preview",
            "wrong-name",
            "1.0.0",
            &package_manifest("other", "1.0.0", "Mismatch"),
        );
        install(
            root,
            "preview",
            "wrong-version",
            "1.0.0",
            &package_manifest("wrong-version", "2.0.0", "Mismatch"),
        );
        install(
            root,
            "invalid namespace",
            "valid-name",
            "1.0.0",
            &package_manifest("valid-name", "1.0.0", "Invalid path"),
        );
        fs::create_dir_all(root.join("preview/missing/1.0.0")).unwrap();

        let scan =
            scan_installed_packages(&PackageRoots::from_roots(vec![PackageRoot::data(root)]));
        assert!(scan.packages.is_empty());
        assert_eq!(scan.warnings.len(), 5);
        assert!(
            scan.warnings
                .iter()
                .any(|warning| warning.message.contains("parse"))
        );
        assert!(
            scan.warnings
                .iter()
                .any(|warning| warning.message.contains("name"))
        );
        assert!(
            scan.warnings
                .iter()
                .any(|warning| warning.message.contains("version"))
        );
        assert!(
            scan.warnings
                .iter()
                .any(|warning| warning.message.contains("invalid package path"))
        );
        assert!(
            scan.warnings
                .iter()
                .any(|warning| warning.message.contains("could not read package manifest"))
        );
    }

    #[test]
    fn missing_roots_are_silent() {
        let directory = tempfile::tempdir().unwrap();
        let scan = scan_installed_packages(&PackageRoots::from_roots(vec![PackageRoot::data(
            directory.path().join("missing"),
        )]));
        assert!(scan.packages.is_empty());
        assert!(scan.warnings.is_empty());
    }

    const INDEX: &str = r#"[
      {
        "name": "diagrammer",
        "version": "1.0.0",
        "entrypoint": "lib.typ",
        "authors": ["Ada"],
        "license": "MIT",
        "description": "Draw diagrams",
        "repository": "https://example.test/diagrammer",
        "keywords": ["graph"],
        "categories": ["visualization"],
        "updatedAt": 1700000000,
        "futureField": true
      },
      {
        "name": "diagrammer",
        "version": "1.2.0",
        "entrypoint": "src/lib.typ",
        "description": "Draw newer diagrams",
        "template": {"path": "template", "entrypoint": "main.typ"},
        "updatedAt": 1800000000
      },
      {
        "name": "alpha",
        "version": "0.2.0",
        "entrypoint": "lib.typ"
      }
    ]"#;

    #[test]
    fn official_index_parser_accepts_metadata_unknown_fields_and_sorts() {
        let packages = parse_official_index(INDEX).unwrap();
        assert_eq!(
            packages
                .iter()
                .map(|package| format!("{}:{}", package.name, package.version))
                .collect::<Vec<_>>(),
            ["alpha:0.2.0", "diagrammer:1.2.0", "diagrammer:1.0.0"]
        );
        assert!(packages[1].metadata.is_template);
        assert_eq!(packages[1].updated_at, Some(1_800_000_000));
        assert_eq!(packages[2].metadata.keywords, ["graph"]);
    }

    #[test]
    fn official_index_parser_rejects_invalid_and_duplicate_records() {
        for source in [
            r#"[{"name":"bad name","version":"1.0.0","entrypoint":"lib.typ"}]"#,
            r#"[{"name":"okay","version":"1.0","entrypoint":"lib.typ"}]"#,
            r#"[
                {"name":"same","version":"1.0.0","entrypoint":"lib.typ"},
                {"name":"same","version":"1.0.0","entrypoint":"other.typ"}
            ]"#,
            r#"{"name":"not-an-array"}"#,
        ] {
            assert!(parse_official_index(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn merge_tracks_installations_available_versions_latest_and_updates() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        let cache = directory.path().join("cache");
        let manifest = package_manifest("diagrammer", "1.0.0", "Installed copy");
        install(&data, "preview", "diagrammer", "1.0.0", &manifest);
        install(&cache, "preview", "diagrammer", "1.0.0", &manifest);
        install(
            &data,
            "preview",
            "diagrammer",
            "0.8.0",
            &package_manifest("diagrammer", "0.8.0", "Older installed copy"),
        );
        install(
            &data,
            "local",
            "private-tools",
            "3.0.0",
            &package_manifest("private-tools", "3.0.0", "Private"),
        );
        let scan = scan_installed_packages(&PackageRoots::from_roots(vec![
            PackageRoot::data(&data),
            PackageRoot::cache(&cache),
        ]));
        let catalog =
            PackageCatalog::from_sources(scan.packages, parse_official_index(INDEX).unwrap());

        assert_eq!(
            catalog
                .packages()
                .iter()
                .map(|package| format!("@{}/{}", package.namespace, package.name))
                .collect::<Vec<_>>(),
            [
                "@local/private-tools",
                "@preview/alpha",
                "@preview/diagrammer"
            ]
        );
        let diagrammer = &catalog.packages()[2];
        assert_eq!(diagrammer.latest_available, Some(version("1.2.0")));
        assert_eq!(diagrammer.latest_installed(), Some(version("1.0.0")));
        assert!(diagrammer.has_update());
        assert_eq!(diagrammer.releases.len(), 3);
        assert_eq!(diagrammer.releases[1].installations.len(), 2);
        assert_eq!(
            diagrammer.releases[1].installations[0].root.kind,
            PackageRootKind::Data
        );
        assert_eq!(
            diagrammer.releases[1].metadata.description.as_deref(),
            Some("Draw diagrams")
        );
        assert_eq!(
            diagrammer.display_release().unwrap().version,
            version("1.2.0")
        );
        let installed = diagrammer.installed_releases().collect::<Vec<_>>();
        assert_eq!(
            installed
                .iter()
                .map(|release| release.version)
                .collect::<Vec<_>>(),
            [version("1.0.0"), version("0.8.0")]
        );
        assert_eq!(
            installed
                .iter()
                .flat_map(|release| &release.installations)
                .map(|installation| installation.package_path.clone())
                .collect::<Vec<_>>(),
            [
                data.join("preview/diagrammer/1.0.0"),
                cache.join("preview/diagrammer/1.0.0"),
                data.join("preview/diagrammer/0.8.0"),
            ]
        );

        let private = &catalog.packages()[0];
        assert_eq!(private.latest_available, None);
        assert!(private.is_installed());
        assert_eq!(private.display_release().unwrap().version, version("3.0.0"));
    }

    #[test]
    fn data_manifest_metadata_wins_over_a_duplicate_cached_copy() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        let cache = directory.path().join("cache");
        install(
            &cache,
            "local",
            "shared",
            "1.0.0",
            &package_manifest("shared", "1.0.0", "Cached metadata"),
        );
        install(
            &data,
            "local",
            "shared",
            "1.0.0",
            &package_manifest("shared", "1.0.0", "Data metadata"),
        );
        // Even an arbitrary caller-provided root order cannot invert Typst's
        // data-before-cache precedence.
        let scan = scan_installed_packages(&PackageRoots::from_roots(vec![
            PackageRoot::cache(&cache),
            PackageRoot::data(&data),
        ]));
        let catalog = PackageCatalog::from_sources(scan.packages, Vec::new());
        let release = catalog.packages()[0].display_release().unwrap();
        assert_eq!(
            release.metadata.description.as_deref(),
            Some("Data metadata")
        );
        assert_eq!(release.installations[0].root.kind, PackageRootKind::Data);
    }

    #[test]
    fn filter_matches_identity_and_metadata_with_and_semantics() {
        let catalog =
            PackageCatalog::from_sources(Vec::new(), parse_official_index(INDEX).unwrap());
        assert_eq!(catalog.filtered("").len(), 2);
        assert_eq!(catalog.filtered("DIAgram").len(), 1);
        assert_eq!(catalog.filtered("preview diagrammer").len(), 1);
        assert_eq!(catalog.filtered("graph 1.0").len(), 1);
        assert_eq!(catalog.filtered("visualization Ada").len(), 1);
        assert!(catalog.filtered("graph 9.9").is_empty());
        assert!(catalog.filtered("missing").is_empty());
    }

    #[test]
    fn load_keeps_installed_catalog_when_registry_fails() {
        let directory = tempfile::tempdir().unwrap();
        install(
            directory.path(),
            "local",
            "offline",
            "1.0.0",
            &package_manifest("offline", "1.0.0", "Works offline"),
        );
        let roots = PackageRoots::from_roots(vec![PackageRoot::data(directory.path())]);
        let loaded = load_catalog_with(&roots, || Err(CatalogError::network("offline")));
        assert_eq!(loaded.catalog.packages().len(), 1);
        assert_eq!(
            loaded.official_index_error.unwrap().kind,
            CatalogErrorKind::Network
        );
    }

    #[test]
    fn installed_load_is_available_without_registry_access() {
        let directory = tempfile::tempdir().unwrap();
        install(
            directory.path(),
            "local",
            "offline",
            "1.0.0",
            &package_manifest("offline", "1.0.0", "Works offline"),
        );
        let roots = PackageRoots::from_roots(vec![PackageRoot::data(directory.path())]);
        let loaded = PackageCatalogLoad::load_installed(&roots);
        assert_eq!(loaded.catalog.packages().len(), 1);
        assert!(loaded.official_index_error.is_none());
        assert_eq!(
            loaded.catalog.packages()[0]
                .display_release()
                .and_then(|release| release.metadata.description.as_deref()),
            Some("Works offline")
        );
    }

    #[test]
    fn registry_response_reader_is_bounded_and_requires_utf8() {
        assert_eq!(read_index_body(Cursor::new(b"four"), 4).unwrap(), "four");
        let too_large = read_index_body(Cursor::new(b"fives"), 4).unwrap_err();
        assert_eq!(too_large.kind, CatalogErrorKind::Network);
        assert!(too_large.message.contains("4-byte"));

        let invalid_utf8 = read_index_body(Cursor::new([0xff]), 4).unwrap_err();
        assert_eq!(invalid_utf8.kind, CatalogErrorKind::Network);
        assert!(DEFAULT_FETCH_TIMEOUT <= Duration::from_secs(5));
    }
}
