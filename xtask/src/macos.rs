//! Build an actual universal Mach-O, rather than labelling a native build universal.
use super::*;

pub(super) const UNIVERSAL_TARGET: &str = "universal-apple-darwin";
pub(super) const MACOS_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

pub(super) fn default_target(host: &str) -> &str {
    if host.ends_with("apple-darwin") {
        UNIVERSAL_TARGET
    } else {
        host
    }
}

pub(super) fn targets(target: &str) -> Vec<&str> {
    if target == UNIVERSAL_TARGET {
        MACOS_TARGETS.to_vec()
    } else {
        vec![target]
    }
}

pub(super) fn build_release(target: &str, production: bool) -> Result<(), String> {
    let root = repository_root();
    let rustc = pinned_rustc(&root)?;
    let host = compiler_host(&rustc)?;
    if !host.ends_with("apple-darwin") {
        return Err(
            "building a macOS app requires macOS and its Xcode command line tools".to_owned(),
        );
    }
    let slices = targets(target);
    for slice in &slices {
        if !MACOS_TARGETS.contains(slice) {
            return Err(format!("unsupported macOS package target: {target}"));
        }
    }
    let target_directory = root.join("target");
    // Reuse the existing native Cargo cache; cross builds have their own target
    // directory. Retain the thin native result separately before publishing fat.
    let mut binaries = Vec::with_capacity(slices.len());
    for slice in &slices {
        ensure_target(&root, &rustc, slice)?;
        let mut cargo = Command::new("cargo");
        let native = *slice == host;
        let binary = if native {
            target_directory.join("release/tiptoptyp")
        } else {
            target_directory.join(slice).join("release/tiptoptyp")
        };
        if native && binary.exists() && verify_architectures(&binary, slice).is_err() {
            // Cargo fingerprints do not establish that a published fat binary
            // is still its thin output. Removing just that output lets Cargo
            // restore/relink its correct artifact while reusing all dependencies.
            fs::remove_file(&binary)
                .map_err(|error| format!("resetting native release executable: {error}"))?;
        }
        cargo
            .current_dir(&root)
            .env("RUSTC", &rustc)
            .env("MACOSX_DEPLOYMENT_TARGET", "12.0")
            .env_remove("CARGO_BUILD_TARGET")
            .args(cargo_arguments(slice, &host, production))
            .arg("--target-dir")
            .arg(&target_directory);
        run_status(&mut cargo, &format!("release build for {slice}"))?;
        verify_architectures(&binary, slice)?;
        if native {
            let preserved = target_directory
                .join("package-slices")
                .join(slice)
                .join("tiptoptyp");
            fs::create_dir_all(preserved.parent().expect("native slice directory"))
                .map_err(|error| error.to_string())?;
            fs::copy(&binary, &preserved)
                .map_err(|error| format!("preserving native release executable: {error}"))?;
            binaries.push(preserved);
        } else {
            binaries.push(binary);
        }
    }
    let release = target_directory.join("release");
    fs::create_dir_all(&release).map_err(|error| format!("creating release directory: {error}"))?;
    let staged = release.join(format!(".tiptoptyp-package-{}", std::process::id()));
    let result = (|| {
        if binaries.len() == 1 {
            fs::copy(&binaries[0], &staged)
                .map_err(|error| format!("staging thin macOS executable: {error}"))?;
        } else {
            let mut lipo = Command::new("lipo");
            lipo.arg("-create")
                .args(&binaries)
                .arg("-output")
                .arg(&staged);
            run_status(&mut lipo, "combining Apple Silicon and Intel executables")?;
            let permissions = fs::metadata(&binaries[0])
                .map_err(|error| format!("reading executable permissions: {error}"))?
                .permissions();
            fs::set_permissions(&staged, permissions)
                .map_err(|error| format!("setting universal executable permissions: {error}"))?;
        }
        verify_architectures(&staged, target)?;
        fs::rename(&staged, release.join("tiptoptyp"))
            .map_err(|error| format!("publishing macOS package executable: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn cargo_arguments<'a>(target: &'a str, host: &str, production: bool) -> Vec<&'a str> {
    let mut arguments = vec!["build", "--locked", "--release"];
    if target != host {
        arguments.extend(["--target", target]);
    }
    if production {
        arguments.extend(["--features", "production"]);
    }
    arguments
}

fn pinned_rustc(root: &Path) -> Result<PathBuf, String> {
    // Homebrew Cargo may otherwise pick its own rustc, which does not carry
    // the additional targets installed for the repository's pinned toolchain.
    let output = run_output(
        Command::new("rustup")
            .current_dir(root)
            .args(["which", "rustc"]),
        "locating the repository's pinned Rust compiler",
    )?;
    let path = String::from_utf8(output.stdout)
        .map_err(|_| "rustup returned a non-UTF-8 compiler path".to_owned())?;
    Ok(PathBuf::from(path.trim()))
}

fn compiler_host(rustc: &Path) -> Result<String, String> {
    let output = run_output(
        Command::new(rustc).arg("-vV"),
        "querying the selected Rust compiler's host target",
    )?;
    parse_compiler_host(&String::from_utf8_lossy(&output.stdout)).map(str::to_owned)
}

fn parse_compiler_host(report: &str) -> Result<&str, String> {
    let mut hosts = report
        .lines()
        .filter_map(|line| line.strip_prefix("host: "));
    let host = hosts
        .next()
        .filter(|host| !host.is_empty() && !host.chars().any(char::is_whitespace))
        .ok_or_else(|| {
            "the selected Rust compiler did not report a valid host target".to_owned()
        })?;
    if hosts.next().is_some() {
        return Err("the selected Rust compiler reported multiple host targets".to_owned());
    }
    Ok(host)
}

fn ensure_target(root: &Path, rustc: &Path, target: &str) -> Result<(), String> {
    let output = run_output(
        Command::new(rustc).args(["--print", "target-libdir", "--target", target]),
        "checking the macOS Rust standard library",
    )?;
    let path = String::from_utf8(output.stdout)
        .map_err(|_| "rustc returned a non-UTF-8 target directory".to_owned())?;
    if !Path::new(path.trim()).is_dir() {
        run_status(
            Command::new("rustup")
                .current_dir(root)
                .args(["target", "add", target]),
            &format!("installing the Rust standard library for {target}"),
        )?;
    }
    Ok(())
}

pub(super) fn verify_architectures(path: &Path, target: &str) -> Result<(), String> {
    let output = run_output(
        Command::new("lipo").arg("-archs").arg(path),
        &format!("checking architectures in {}", path.display()),
    )?;
    let actual = String::from_utf8(output.stdout)
        .map_err(|_| "lipo returned non-UTF-8 architecture names".to_owned())?;
    validate_architectures(&actual, target).map_err(|error| format!("{}: {error}", path.display()))
}

fn validate_architectures(actual: &str, target: &str) -> Result<(), String> {
    let mut actual = actual.split_whitespace().collect::<Vec<_>>();
    actual.sort_unstable();
    let mut expected = match target {
        UNIVERSAL_TARGET => vec!["arm64", "x86_64"],
        "aarch64-apple-darwin" => vec!["arm64"],
        "x86_64-apple-darwin" => vec!["x86_64"],
        _ => return Err(format!("unsupported macOS package target: {target}")),
    };
    expected.sort_unstable();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "expected {target} architectures {expected:?}, found {actual:?}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_packages_default_to_both_architectures_only_on_macos() {
        for host in MACOS_TARGETS {
            assert_eq!(default_target(host), UNIVERSAL_TARGET);
            assert_eq!(targets(default_target(host)), MACOS_TARGETS);
        }
        assert_eq!(
            default_target("x86_64-unknown-linux-gnu"),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            default_target("aarch64-pc-windows-msvc"),
            "aarch64-pc-windows-msvc"
        );
    }

    #[test]
    fn native_cache_host_comes_from_the_selected_compiler_report() {
        for host in MACOS_TARGETS {
            let report = format!("rustc 1.98.1\nbinary: rustc\nhost: {host}\nrelease: 1.98.1\n");
            assert_eq!(parse_compiler_host(&report).unwrap(), host);
        }
        for report in [
            "release: 1.98.1",
            "host: ",
            "host: aarch64 apple",
            "host: aarch64-apple-darwin\nhost: x86_64-apple-darwin",
        ] {
            assert!(parse_compiler_host(report).is_err());
        }
    }

    #[test]
    fn explicit_thin_targets_stay_thin() {
        for target in MACOS_TARGETS {
            assert_eq!(targets(target), [target]);
        }
    }

    #[test]
    fn native_build_reuses_host_cache_and_cross_build_selects_its_target() {
        let host = "aarch64-apple-darwin";
        assert_eq!(
            cargo_arguments(host, host, false),
            ["build", "--locked", "--release"]
        );
        assert_eq!(
            cargo_arguments(host, host, true),
            ["build", "--locked", "--release", "--features", "production"]
        );
        assert_eq!(
            cargo_arguments("x86_64-apple-darwin", host, true),
            [
                "build",
                "--locked",
                "--release",
                "--target",
                "x86_64-apple-darwin",
                "--features",
                "production"
            ]
        );
        assert_eq!(
            cargo_arguments("aarch64-apple-darwin", "x86_64-apple-darwin", false),
            [
                "build",
                "--locked",
                "--release",
                "--target",
                "aarch64-apple-darwin"
            ]
        );
    }

    #[test]
    fn architecture_checks_reject_native_binaries_mislabelled_as_universal() {
        assert!(validate_architectures("x86_64 arm64\n", UNIVERSAL_TARGET).is_ok());
        assert!(validate_architectures("arm64", UNIVERSAL_TARGET).is_err());
        assert!(validate_architectures("x86_64", UNIVERSAL_TARGET).is_err());
        assert!(validate_architectures("arm64 x86_64", "aarch64-apple-darwin").is_err());
        assert!(validate_architectures("x86_64", "aarch64-apple-darwin").is_err());
        assert!(validate_architectures("arm64", "aarch64-apple-darwin").is_ok());
        assert!(validate_architectures("x86_64", "x86_64-apple-darwin").is_ok());
    }
}
