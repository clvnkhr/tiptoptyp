//! Reproducible, isolated GUI profiling. Only processes started here are stopped.
use super::{host_target, repository_root, run_output, run_status, sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HELP: &str = "Usage: cargo xtask profile [options]
  --scenario main|settings|find|fonts|hover|hover-scroll|large|tabs|tabs-switch|pdf|no-window|multi-window  (default settings)
  --warmup SECONDS       0..300, after initial capture (default 3)
  --seconds SECONDS      1..300 (default 10)
  --sampler auto|none|sample|perf  (auto: sample on macOS, none elsewhere)
  --skip-build           reuse target/profiling/tiptoptyp; hash is recorded
  --binary PATH          profile a preserved optimized binary without rebuilding

Uses isolated fixture copies and non-persistent QA windows. Results remain in
.tiptoptyp/profiles/<unique-run>/. See docs/performance.md for interpretation.";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    scenario: String,
    warmup: u64,
    seconds: u64,
    sampler: String,
    skip_build: bool,
    binary: Option<PathBuf>,
}
impl Options {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut options = Self {
            scenario: "settings".into(),
            warmup: 3,
            seconds: 10,
            sampler: "auto".into(),
            skip_build: false,
            binary: None,
        };
        let mut args = arguments.iter();
        while let Some(flag) = args.next() {
            if flag == "--skip-build" {
                options.skip_build = true;
                continue;
            }
            let value = args
                .next()
                .ok_or_else(|| format!("{flag} requires a value"))?;
            match flag.as_str() {
                "--binary" => {
                    options.binary = Some(PathBuf::from(value));
                    options.skip_build = true;
                }
                "--scenario" if scenario_scene(value).is_some() => options.scenario = value.clone(),
                "--warmup" => options.warmup = seconds(value, 0)?,
                "--seconds" => options.seconds = seconds(value, 1)?,
                "--sampler" if ["auto", "none", "sample", "perf"].contains(&value.as_str()) => {
                    options.sampler = value.clone()
                }
                _ => return Err(format!("invalid option {flag} {value:?}\n{HELP}")),
            }
        }
        Ok(options)
    }
}
fn seconds(value: &str, minimum: u64) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| (minimum..=300).contains(value))
        .ok_or_else(|| format!("expected {minimum}..=300 whole seconds, got {value:?}"))
}
fn scenario_scene(scenario: &str) -> Option<&'static str> {
    match scenario {
        "main" | "large" | "no-window" | "multi-window" => Some("main"),
        "tabs" => Some("tabs"),
        "pdf" => Some("tabs-pdf"),
        "settings" => Some("settings-window"),
        "find" => Some("find-replace"),
        "fonts" => Some("settings-font-picker"),
        "hover" | "hover-scroll" => Some("function-tooltip"),
        "tabs-switch" => Some("tabs"),
        _ => None,
    }
}
fn sampler_for(requested: &str, os: &str) -> Result<&'static str, String> {
    match (requested, os) {
        ("auto", "macos") | ("sample", "macos") => Ok("sample"),
        ("auto" | "none", _) => Ok("none"),
        ("perf", "linux") => Ok("perf"),
        _ => Err(format!(
            "sampler {requested:?} is not supported on {os}; use --sampler none"
        )),
    }
}

pub(super) fn run(arguments: Vec<String>) -> Result<(), String> {
    if arguments == ["--help"] || arguments == ["-h"] {
        println!("{HELP}");
        return Ok(());
    }
    let options = Options::parse(&arguments)?;
    let sampler = sampler_for(&options.sampler, env::consts::OS)?;
    let root = repository_root();
    let target = host_target()?;
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let typst = root.join(format!("toolchain/bin/typst-{target}{extension}"));
    let tinymist = root.join(format!("toolchain/bin/tinymist-{target}{extension}"));
    for tool in [&typst, &tinymist] {
        if !tool.is_file() {
            return Err(format!(
                "missing {}; first run cargo xtask fetch-sidecars",
                tool.display()
            ));
        }
    }
    let poppler = run_output(
        Command::new("pdftoppm").arg("-v"),
        "checking Poppler (pdftoppm)",
    )?;
    if sampler != "none" {
        let program = if sampler == "sample" {
            "/usr/bin/sample"
        } else {
            "perf"
        };
        // Spawn failures are reported before building; help may legitimately exit nonzero.
        Command::new(program)
            .arg(if sampler == "perf" {
                "--version"
            } else {
                "--help"
            })
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("cannot start {program}: {error}"))?;
    }
    if !options.skip_build {
        let mut build = Command::new("cargo");
        build
            .current_dir(&root)
            .args([
                "build",
                "--locked",
                "--profile",
                "profiling",
                "--features",
                "profiling",
                "--bin",
                "tiptoptyp",
                "--target-dir",
            ])
            .arg(root.join("target"));
        apply_frame_pointers(&mut build);
        run_status(&mut build, "building optimized profiling executable")?;
    }
    let binary = options.binary.as_ref().map_or_else(
        || root.join(format!("target/profiling/tiptoptyp{extension}")),
        |path| root.join(path),
    );
    if !binary.is_file() {
        return Err(format!("missing {}", binary.display()));
    }
    let directory = create_run_directory(&root.join(".tiptoptyp/profiles"), &options.scenario)?;
    println!("Profile artifacts: {}", directory.display());
    let document = prepare_workspace(&directory, &options.scenario)?;
    let workspace = document.parent().unwrap();
    write_metadata(&directory, &binary, &document, &options, sampler, &poppler)?;

    let mut command = Command::new(&binary);
    command.current_dir(workspace);
    isolate_git(&mut command, &directory);
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("TIPTOPTYP_UI_")
            || key.to_string_lossy().starts_with("TIPTOPTYP_PROFILE_")
        {
            command.env_remove(key);
        }
    }
    command
        .env("TIPTOPTYP_PROFILE_DIR", &directory)
        .env(
            "TIPTOPTYP_PROFILE_MULTI_WINDOW",
            if options.scenario == "multi-window" {
                "1"
            } else {
                "0"
            },
        )
        .env(
            "TIPTOPTYP_PROFILE_NO_WINDOW",
            if options.scenario == "no-window" {
                "1"
            } else {
                "0"
            },
        )
        .env("TIPTOPTYP_PROFILE_WARMUP", options.warmup.to_string())
        .env("TIPTOPTYP_PROFILE_SECONDS", options.seconds.to_string())
        .env("TIPTOPTYP_TYPST", typst)
        .env("TIPTOPTYP_TINYMIST", tinymist)
        .args([
            "--ui-theme",
            "catppuccin-latte",
            "--ui-snapshot-scene",
            scenario_scene(&options.scenario).unwrap(),
        ])
        .arg(&document);
    if let Some(input) = match options.scenario.as_str() {
        "hover-scroll" => Some("hover-scroll"),
        "tabs-switch" => Some("tabs-switch"),
        _ => None,
    } {
        command.env("TIPTOPTYP_PROFILE_INPUT", input);
    }
    let mut app = ManagedChild::spawn(&mut command, &directory.join("app.log"))?;
    println!(
        "Waiting for initial capture, then {}s warmup + {}s measurement (PID {}).",
        options.warmup,
        options.seconds,
        app.0.id()
    );
    wait_ready(
        &mut app.0,
        &directory.join("ready"),
        Duration::from_secs(120),
    )?;
    wait_alive(&mut app.0, Duration::from_secs(options.warmup))?;
    println!("Measuring for {} seconds with {sampler}.", options.seconds);
    save_process_reading(&directory.join("cpu-before.txt"), app.0.id())?;
    if sampler == "none" {
        wait_alive(&mut app.0, Duration::from_secs(options.seconds))?;
    } else {
        let mut sample = if sampler == "sample" {
            let mut command = Command::new("/usr/bin/sample");
            command
                .arg(app.0.id().to_string())
                .arg(options.seconds.to_string())
                .arg("1")
                .arg("-file")
                .arg(directory.join("cpu.sample.txt"));
            command
        } else {
            let mut command = Command::new("perf");
            command
                .args(["record", "-F", "99", "-g", "--call-graph", "fp", "-p"])
                .arg(app.0.id().to_string())
                .arg("-o")
                .arg(directory.join("perf.data"))
                .args(["--", "sleep"])
                .arg(options.seconds.to_string());
            command
        };
        let mut sampler_process = ManagedChild::spawn(&mut sample, &directory.join("sampler.log"))?;
        let status = wait_exit(
            &mut sampler_process.0,
            Duration::from_secs(options.seconds + 30),
        )?;
        if !status.success() {
            return Err(format!(
                "{sampler} failed ({status}); see {}/sampler.log",
                directory.display()
            ));
        }
    }
    // Sampling analysis can outlive the measured process; retain that fact, not a fake zero.
    save_process_reading(&directory.join("cpu-after.txt"), app.0.id())?;
    let status = wait_exit(&mut app.0, Duration::from_secs(30))?;
    if !status.success() {
        return Err(format!(
            "profiled app failed ({status}); see {}/app.log",
            directory.display()
        ));
    }
    let summary = fs::read_to_string(directory.join("summary.json")).map_err(|e| e.to_string())?;
    if summary.is_empty() {
        return Err("app did not write a profiling summary".into());
    }
    println!("{summary}\nFinished: {}", directory.display());
    Ok(())
}

fn apply_frame_pointers(command: &mut Command) {
    if let Some(mut flags) = env::var_os("CARGO_ENCODED_RUSTFLAGS") {
        if !flags.is_empty() {
            flags.push("\x1f");
        }
        flags.push("-Cforce-frame-pointers=yes");
        command.env("CARGO_ENCODED_RUSTFLAGS", flags);
    } else {
        let mut flags = env::var_os("RUSTFLAGS").unwrap_or_default();
        if !flags.is_empty() {
            flags.push(" ");
        }
        flags.push("-Cforce-frame-pointers=yes");
        command.env("RUSTFLAGS", flags);
    }
}

fn fixture(scenario: &str) -> String {
    if scenario == "pdf" {
        let mut source = "#set page(paper: \"a6\", margin: 8pt)\n".to_owned();
        for page in 1..=40 {
            source.push_str(&format!(
                "= Residency page {page}\n\nA bounded PDF residency profiling fixture.\n\n#pagebreak()\n"
            ));
        }
        return source;
    }
    if scenario != "large" {
        return include_str!("../../docs/ui-snapshots/theme-fixture.typ").to_owned();
    }
    let mut source = "#set page(paper: \"a4\")\n= Large source workload\n\n".to_owned();
    for index in 0..5_000 {
        source.push_str(&format!(
            "// Unicode αβγ — reproducible source row {index}: Typst editor profiling\n"
        ));
        if index % 50 == 0 {
            source.push_str(&format!(
                "#let value_{index} = ({index}, \"sample\", true)\n"
            ));
        }
    }
    source
}

fn prepare_workspace(directory: &Path, scenario: &str) -> Result<PathBuf, String> {
    let workspace = directory.join("workspace");
    fs::create_dir(&workspace).map_err(|e| e.to_string())?;
    // App project discovery must stop here instead of walking to the checkout.
    fs::write(
        workspace.join("typst.toml"),
        "# Isolated profiling project.\n",
    )
    .map_err(|e| e.to_string())?;
    let document = workspace.join("main.typ");
    fs::write(&document, fixture(scenario)).map_err(|e| e.to_string())?;
    Ok(document)
}

fn isolate_git(command: &mut Command, ceiling: &Path) {
    // Project discovery and Git discovery are separate. Prevent inherited Git
    // overrides or a parent checkout from supplying a machine-dependent workload.
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command.env("GIT_CEILING_DIRECTORIES", ceiling);
}

fn create_run_directory(base: &Path, scenario: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    for attempt in 0..100 {
        let directory = base.join(format!(
            "{time}-{}-{scenario}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("could not allocate a fresh profile directory".into())
}

fn write_metadata(
    directory: &Path,
    binary: &Path,
    document: &Path,
    options: &Options,
    sampler: &str,
    poppler: &std::process::Output,
) -> Result<(), String> {
    let root = repository_root();
    let capture = |program: &str, arguments: &[&str]| -> String {
        match Command::new(program)
            .args(arguments)
            .current_dir(&root)
            .output()
        {
            Ok(output) => format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(error) => error.to_string(),
        }
    };
    let mut text = format!(
        "scenario={}\nwarmup_seconds={}\nmeasurement_seconds={}\nsampler={sampler}\nos={}\narch={}\nskip_build={}\nbinary={}\nbinary_sha256={}\nfixture_sha256={}\nmanifest_sha256={}\nRUSTFLAGS={:?}\nCARGO_ENCODED_RUSTFLAGS={:?}\nappended_build_flag={}\nlogical_parallelism={:?}\n\n",
        options.scenario,
        options.warmup,
        options.seconds,
        env::consts::OS,
        env::consts::ARCH,
        options.skip_build,
        binary.display(),
        sha256(binary)?,
        sha256(document)?,
        sha256(&root.join("toolchain/manifest.tsv"))?,
        env::var_os("RUSTFLAGS"),
        env::var_os("CARGO_ENCODED_RUSTFLAGS"),
        if options.skip_build {
            "unknown (build skipped)"
        } else {
            "-Cforce-frame-pointers=yes"
        },
        thread::available_parallelism().ok()
    );
    let extension = if cfg!(windows) { ".exe" } else { "" };
    for tool in ["typst", "tinymist"] {
        let path = root.join(format!(
            "toolchain/bin/{tool}-{}{extension}",
            host_target()?
        ));
        text.push_str(&format!("{tool}_sha256={}\n", sha256(&path)?));
    }
    if cfg!(unix) {
        text.push_str(&format!("kernel:\n{}\n", capture("uname", &["-sr"])));
    }
    if cfg!(target_os = "macos") {
        text.push_str(&format!("macos:\n{}\n", capture("sw_vers", &[])));
    }
    for (label, value) in [
        ("revision", capture("git", &["rev-parse", "HEAD"])),
        ("worktree", capture("git", &["status", "--short"])),
        ("rustc", capture("rustc", &["-vV"])),
        (
            "poppler",
            format!(
                "{}{}",
                String::from_utf8_lossy(&poppler.stdout),
                String::from_utf8_lossy(&poppler.stderr)
            ),
        ),
    ] {
        text.push_str(&format!("{label}:\n{value}\n"));
    }
    fs::write(directory.join("metadata.txt"), text).map_err(|e| e.to_string())
}

struct ManagedChild(Child);
impl ManagedChild {
    fn spawn(command: &mut Command, log: &Path) -> Result<Self, String> {
        // A watchdog must also stop descendants (sidecars or perf's timer).
        // Give only this owned command tree a separate process group.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(log)
            .map_err(|e| e.to_string())?;
        command
            .stdin(Stdio::null())
            .stdout(output.try_clone().map_err(|e| e.to_string())?)
            .stderr(output);
        command
            .spawn()
            .map(Self)
            .map_err(|error| format!("could not start {command:?}: {error}"))
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            #[cfg(unix)]
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", self.0.id())])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            #[cfg(windows)]
            let _ = Command::new("taskkill")
                .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn wait_ready(child: &mut Child, marker: &Path, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while !marker.is_file() {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("app exited before readiness: {status}"));
        }
        if Instant::now() >= deadline {
            return Err("app did not finish its initial capture within 120 seconds; check app.log and GUI/tool availability".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
fn wait_alive(child: &mut Child, duration: Duration) -> Result<(), String> {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("app exited during measurement: {status}"));
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
fn wait_exit(child: &mut Child, timeout: Duration) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "process {} exceeded its profiling deadline",
                child.id()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}
fn save_process_reading(path: &Path, pid: u32) -> Result<(), String> {
    let reading = if cfg!(unix) {
        match Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid=,pcpu=,time="])
            .output()
        {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
            _ => "process exited or CPU reading unavailable\n".into(),
        }
    } else {
        "CPU sampling readings are unavailable on this platform\n".into()
    };
    fs::write(path, reading).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }
    #[test]
    fn options_validate_before_building_or_launching() {
        assert_eq!(Options::parse(&[]).unwrap().scenario, "settings");
        assert_eq!(
            Options::parse(&args(&["--scenario", "no-window"]))
                .unwrap()
                .scenario,
            "no-window"
        );
        assert_eq!(scenario_scene("no-window"), Some("main"));
        assert_eq!(scenario_scene("multi-window"), Some("main"));
        assert_eq!(scenario_scene("tabs"), Some("tabs"));
        assert_eq!(scenario_scene("hover-scroll"), Some("function-tooltip"));
        assert_eq!(scenario_scene("tabs-switch"), Some("tabs"));
        assert_eq!(scenario_scene("pdf"), Some("tabs-pdf"));
        let preserved = Options::parse(&args(&["--binary", "baseline/tiptoptyp"])).unwrap();
        assert!(preserved.skip_build);
        assert_eq!(preserved.binary, Some(PathBuf::from("baseline/tiptoptyp")));
        for invalid in [
            &["--seconds", "0"][..],
            &["--warmup", "301"],
            &["--scenario", "../outside"],
            &["--sampler", "unknown"],
            &["--seconds"],
        ] {
            assert!(Options::parse(&args(invalid)).is_err());
        }
        assert!(
            Options::parse(&args(&[
                "--scenario",
                "large",
                "--warmup",
                "0",
                "--skip-build"
            ]))
            .unwrap()
            .skip_build
        );
        assert_eq!(sampler_for("auto", "macos").unwrap(), "sample");
        assert_eq!(sampler_for("auto", "windows").unwrap(), "none");
        assert!(sampler_for("sample", "linux").is_err());
    }
    #[test]
    fn fixtures_are_reproducible_and_do_not_expand_the_rendered_document() {
        let source = fixture("large");
        assert_eq!(source, fixture("large"));
        assert!(source.len() > 300_000);
        assert_eq!(
            source.lines().filter(|line| line.starts_with("//")).count(),
            5_000
        );
        assert_eq!(
            source
                .lines()
                .filter(|line| line.starts_with("#let"))
                .count(),
            100
        );
        let pdf = fixture("pdf");
        assert_eq!(pdf, fixture("pdf"));
        assert_eq!(pdf.matches("#pagebreak()").count(), 40);
        assert_eq!(fixture("settings"), fixture("main"));
    }

    #[test]
    fn run_directories_are_unique_and_preserve_prior_artifacts() {
        let base = env::temp_dir();
        let first = create_run_directory(&base, "profile-test").unwrap();
        fs::write(first.join("summary.json"), "previous result").unwrap();
        let second = create_run_directory(&base, "profile-test").unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read_to_string(first.join("summary.json")).unwrap(),
            "previous result"
        );
        fs::remove_file(first.join("summary.json")).unwrap();
        fs::remove_dir(first).unwrap();
        fs::remove_dir(second).unwrap();
    }

    #[test]
    fn fixture_workspace_bounds_both_project_and_git_discovery() {
        let directory = create_run_directory(&env::temp_dir(), "workspace-test").unwrap();
        let document = prepare_workspace(&directory, "main").unwrap();
        let workspace = document.parent().unwrap();
        assert!(workspace.join("typst.toml").is_file());
        assert_eq!(fs::read_to_string(&document).unwrap(), fixture("main"));
        let mut command = Command::new("git");
        isolate_git(&mut command, &directory);
        assert!(command.get_envs().any(|(key, value)| {
            key == "GIT_CEILING_DIRECTORIES" && value == Some(directory.as_os_str())
        }));
        fs::remove_file(workspace.join("typst.toml")).unwrap();
        fs::remove_file(&document).unwrap();
        fs::remove_dir(workspace).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn watchdog_detects_stalls_and_cleans_up_its_owned_process() {
        let directory = create_run_directory(&env::temp_dir(), "watchdog-test").unwrap();
        let log = directory.join("child.log");
        let mut child = ManagedChild::spawn(Command::new("sleep").arg("30"), &log).unwrap();
        let pid = child.0.id();
        assert!(wait_exit(&mut child.0, Duration::ZERO).is_err());
        assert!(wait_ready(&mut child.0, &directory.join("missing"), Duration::ZERO).is_err());
        drop(child);
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        fs::remove_file(log).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
