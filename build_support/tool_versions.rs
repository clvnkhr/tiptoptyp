//! Build-time version metadata shared with integration tests. No runtime IO.

use std::collections::{BTreeMap, BTreeSet};

pub fn versions(manifest: &str) -> Result<BTreeMap<&str, &str>, String> {
    let mut versions = BTreeMap::new();
    let mut targets = BTreeSet::new();
    for (index, line) in manifest.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        let [tool, target, _asset, version, _archive, _hash] = fields.as_slice() else {
            return Err(format!("manifest line {} must have six fields", index + 1));
        };
        if !matches!(*tool, "typst" | "tinymist") {
            return Err(format!("unknown manifest tool {tool}"));
        }
        if !targets.insert((*tool, *target)) {
            return Err(format!("duplicate manifest target {tool}/{target}"));
        }
        if let Some(previous) = versions.insert(*tool, *version)
            && previous != *version
        {
            return Err(format!(
                "{tool} versions disagree: {previous} and {version}"
            ));
        }
    }
    for tool in ["typst", "tinymist"] {
        if !versions.contains_key(tool) {
            return Err(format!("missing manifest tool {tool}"));
        }
    }
    Ok(versions)
}
