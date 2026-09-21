#[test]
fn version_reports_compiled_identity_without_starting_the_gui() {
    for flag in ["--version", "-V"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_tiptoptyp"))
            .arg(flag)
            // Version must return before profiling or GUI initialization.
            .env("TIPTOPTYP_PROFILE_SECONDS", "invalid")
            .output()
            .unwrap();
        assert!(output.status.success());
        let name = if cfg!(feature = "production") {
            "tiptoptyp"
        } else {
            "tiptoptyp Dev"
        };
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            format!(
                "{name} {} ({})",
                env!("CARGO_PKG_VERSION"),
                env!("TIPTOPTYP_BUILD_ID")
            )
        );
    }
}
