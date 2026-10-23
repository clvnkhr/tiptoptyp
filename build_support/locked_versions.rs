//! Resolved dependency metadata; this parser is used only while building.
pub fn package_version(lock: &str, name: &str) -> Result<String, String> {
    let lock: toml::Value =
        toml::from_str(lock).map_err(|error| format!("Invalid Cargo.lock: {error}"))?;
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or("Cargo.lock has no package list")?;
    let mut matches = packages
        .iter()
        .filter(|package| package.get("name").and_then(toml::Value::as_str) == Some(name));
    let package = matches
        .next()
        .ok_or_else(|| format!("Cargo.lock has no {name}"))?;
    if matches.next().is_some() {
        return Err(format!(
            "Cargo.lock resolves multiple {name} versions; select the app dependency explicitly"
        ));
    }
    package
        .get("version")
        .and_then(toml::Value::as_str)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("Cargo.lock has no {name} version"))
}
