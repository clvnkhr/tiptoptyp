use super::settings::{BuildEngine, TexSettings};
use crate::toolchain::{ToolKind, ToolResolution, resolve_tool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TexTools {
    pub(crate) tectonic: ToolResolution,
    pub(crate) texlab: ToolResolution,
    pub(crate) badness: ToolResolution,
    pub(crate) tex_fmt: ToolResolution,
    distributions: [Option<std::path::PathBuf>; 3],
    pub(crate) synctex: Option<std::path::PathBuf>,
}
impl TexTools {
    pub(crate) fn distribution(&self, engine: BuildEngine) -> Option<&std::path::Path> {
        let index = match engine {
            BuildEngine::Tectonic => return None,
            BuildEngine::PdfLatex => 0,
            BuildEngine::XeLatex => 1,
            BuildEngine::LuaLatex => 2,
        };
        self.distributions[index].as_deref()
    }
    pub(crate) fn resolve(settings: &TexSettings) -> Self {
        Self {
            distributions: ["pdflatex", "xelatex", "lualatex"].map(distribution_tool),
            synctex: distribution_tool("synctex"),
            tectonic: resolve_tool(ToolKind::Tectonic, &settings.tectonic),
            texlab: resolve_tool(ToolKind::Texlab, &settings.texlab),
            badness: resolve_tool(ToolKind::Badness, &settings.badness),
            tex_fmt: resolve_tool(ToolKind::TexFmt, &settings.tex_fmt),
        }
    }
}

/// GUI launches may not inherit the login shell PATH. MacTeX's stable link
/// avoids hardcoding a distribution year. Resolution does not execute programs.
pub(crate) fn distribution_tool(name: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    paths
        .into_iter()
        .chain([std::path::PathBuf::from("/Library/TeX/texbin")])
        .map(|dir| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|path| path.is_file())
        .map(absolute_executable)
}

// TeX selects its format from argv[0]: resolving pdflatex's symlink to
// pdftex would silently select plain TeX. Only make the path absolute.
fn absolute_executable(path: std::path::PathBuf) -> std::path::PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(unix)]
    fn executable_resolution_preserves_the_tex_format_symlink_name() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("pdftex"), "engine").unwrap();
        let alias = directory.path().join("pdflatex");
        std::os::unix::fs::symlink("pdftex", &alias).unwrap();
        let resolved = super::absolute_executable(alias);
        assert!(resolved.is_absolute());
        assert_eq!(resolved.file_name().unwrap(), "pdflatex");
        assert_eq!(std::fs::read_to_string(resolved).unwrap(), "engine");
    }
}
