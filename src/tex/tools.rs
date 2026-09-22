use super::settings::TexSettings;
use crate::toolchain::{ToolKind, ToolResolution, resolve_tool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TexTools {
    pub(crate) tectonic: ToolResolution,
    pub(crate) texlab: ToolResolution,
    pub(crate) badness: ToolResolution,
    pub(crate) tex_fmt: ToolResolution,
}
impl TexTools {
    pub(crate) fn resolve(settings: &TexSettings) -> Self {
        Self {
            tectonic: resolve_tool(ToolKind::Tectonic, &settings.tectonic),
            texlab: resolve_tool(ToolKind::Texlab, &settings.texlab),
            badness: resolve_tool(ToolKind::Badness, &settings.badness),
            tex_fmt: resolve_tool(ToolKind::TexFmt, &settings.tex_fmt),
        }
    }
}
