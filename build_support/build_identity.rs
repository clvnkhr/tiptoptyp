//! Build-time source provenance without observing unrelated Git activity.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, Output},
};

// Include Rust sources, local dependency sources, and data embedded in the app.
// Personal documents and ordinary repository documentation are not app inputs.
const INPUTS: &[&str] = &[
    "src",
    "core/src",
    "core/Cargo.toml",
    "assets",
    "resources",
    "vendor",
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "build_support",
    "toolchain/manifest.tsv",
    "manual-tests/templates",
    "docs/ui-snapshots/gallery-manifest.tsv",
    "scripts/fixtures/table-editor.typ",
    "scripts/fixtures/table-editor.tex",
    "tests/fixtures/arxiv-tables/platonic-metrics.tex",
];

/// Preserve the compiled identity format while describing the actual working
/// tree supplied to rustc, rather than the staging area's contents.
pub fn build_id(root: &Path, stamp: u64) -> String {
    let git = Git { root };
    let revision = git
        .text(&["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown-revision".into());
    let suffix = match git.dirty_inputs() {
        Some(false) => "",
        Some(true) => "-dirty",
        None => "-unknown-state",
    };
    format!("{revision}{suffix}.{stamp}")
}

/// Watch build inputs and only the Git files that can move the current HEAD.
/// `--git-path` handles both ordinary repositories and linked worktrees.
pub fn watch_paths(root: &Path) -> Vec<PathBuf> {
    let git = Git { root };
    let mut paths: BTreeSet<_> = INPUTS.iter().map(|input| root.join(input)).collect();
    if let Some(head) = git.path("HEAD") {
        paths.insert(head);
    }
    if let Some(reference) = git.text(&["symbolic-ref", "--quiet", "HEAD"])
        && let Some(path) = git.path(&reference)
    {
        if path.exists() {
            paths.insert(path);
        } else {
            // A missing watched file would make Cargo rerun on every build.
            // The closest existing parent observes creation of a loose ref;
            // packed-refs observes changes while that ref remains packed.
            if let Some(parent) = existing_parent(&path) {
                paths.insert(parent.to_owned());
            }
            if let Some(packed) = git.path("packed-refs").filter(|path| path.exists()) {
                paths.insert(packed);
            }
        }
    }
    paths.into_iter().collect()
}

fn existing_parent(path: &Path) -> Option<&Path> {
    path.ancestors().skip(1).find(|path| path.is_dir())
}

struct Git<'a> {
    root: &'a Path,
}

impl Git<'_> {
    fn command(&self) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(self.root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            // Git diff can refresh stat-only cache entries even when optional
            // locks are disabled; provenance observation must never do so.
            .args(["-c", "diff.autoRefreshIndex=false"]);
        // The package's own working directory determines its provenance, even
        // when a caller inherited overrides from a different Git operation.
        for name in [
            "GIT_DIR",
            "GIT_COMMON_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        ] {
            command.env_remove(name);
        }
        command
    }

    fn output(&self, arguments: &[&str]) -> Option<Output> {
        self.command().args(arguments).output().ok()
    }

    fn text(&self, arguments: &[&str]) -> Option<String> {
        let output = self.output(arguments)?;
        if !output.status.success() {
            return None;
        }
        let mut text = String::from_utf8(output.stdout).ok()?;
        if text.ends_with('\n') {
            text.pop();
            if text.ends_with('\r') {
                text.pop();
            }
        }
        (!text.is_empty()).then_some(text)
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        self.text(&["rev-parse", "--path-format=absolute", "--git-path", name])
            .map(PathBuf::from)
    }

    fn dirty_inputs(&self) -> Option<bool> {
        let output = self
            .command()
            .args([
                "diff",
                "--numstat",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "HEAD",
                "--",
            ])
            .args(INPUTS)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        // With automatic index refresh disabled, --quiet/--exit-code may report
        // stat-only changes as dirty. Numstat computes actual differences and
        // includes mode changes without copying source contents into its output.
        if !output.stdout.is_empty() {
            return Some(true);
        }
        let output = self
            .command()
            .args(["ls-files", "--others", "--exclude-standard", "-z", "--"])
            .args(INPUTS)
            .output()
            .ok()?;
        output.status.success().then_some(!output.stdout.is_empty())
    }
}
