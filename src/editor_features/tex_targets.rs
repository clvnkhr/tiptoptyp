//! Literal TeX links and preview assets, indexed once per source revision.
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use super::{AssetCallKind, PreviewAssetKind};
use crate::delimiters::TexDelimiterIndex;

#[derive(Default)]
pub(crate) struct Index {
    links: Vec<Link>,
    assets: Vec<Asset>,
}

struct Link {
    bytes: Range<usize>,
    target: String,
}

pub(crate) struct Asset {
    pub(crate) kind: AssetCallKind,
    pub(crate) call_bytes: Range<usize>,
    pub(crate) literal_bytes: Range<usize>,
    pub(crate) value_bytes: Range<usize>,
    value: String,
}

impl Index {
    pub(crate) fn new(source: &str, syntax: &TexDelimiterIndex) -> Self {
        let mut result = Self::default();
        let mut at = 0;
        while let Some(relative) = source[at..].find('\\') {
            let start = at + relative;
            let name_start = start + 1;
            at = source[name_start..]
                .find(|c: char| !c.is_ascii_alphabetic())
                .map_or(source.len(), |offset| name_start + offset);
            if at == name_start {
                at += source[at..].chars().next().map_or(0, char::len_utf8);
                continue;
            }
            if syntax.ignored_at(start) {
                continue;
            }
            let name = &source[name_start..at];
            if !matches!(name, "url" | "href" | "includegraphics" | "includepdf") {
                continue;
            }
            let mut next = at;
            if source.as_bytes().get(next) == Some(&b'*') {
                next += 1;
            }
            next = trivia(source, next, syntax);
            if source.as_bytes().get(next) == Some(&b'[') {
                let Some((_, end)) = group(source, next, b'[', syntax) else {
                    continue;
                };
                next = trivia(source, end, syntax);
            }
            let Some((value_bytes, end)) = group(source, next, b'{', syntax) else {
                continue;
            };
            let Some(value) = literal(&source[value_bytes.clone()], name) else {
                continue;
            };
            if matches!(name, "url" | "href") {
                let end = if name == "href" {
                    let Some((_, end)) = group(source, trivia(source, end, syntax), b'{', syntax)
                    else {
                        continue;
                    };
                    end
                } else {
                    end
                };
                result.links.push(Link {
                    bytes: start..end,
                    target: value,
                });
            } else {
                result.assets.push(Asset {
                    kind: if name == "includegraphics" {
                        AssetCallKind::Image
                    } else {
                        AssetCallKind::Include
                    },
                    call_bytes: start..end,
                    literal_bytes: next..end,
                    value_bytes,
                    value,
                });
            }
        }
        result
    }

    pub(crate) fn link_at(&self, byte: usize) -> Option<&str> {
        let row = self.links.get(
            self.links
                .partition_point(|link| link.bytes.start <= byte)
                .checked_sub(1)?,
        )?;
        row.bytes.contains(&byte).then_some(row.target.as_str())
    }

    pub(crate) fn asset_at(&self, byte: usize) -> Option<&Asset> {
        let row = self.assets.get(
            self.assets
                .partition_point(|asset| asset.call_bytes.start <= byte)
                .checked_sub(1)?,
        )?;
        row.call_bytes.contains(&byte).then_some(row)
    }
}

impl Asset {
    pub(crate) fn resolve(
        &self,
        source_path: &Path,
        workspace_root: &Path,
    ) -> Option<(PreviewAssetKind, PathBuf)> {
        let root =
            super::canonicalize_existing_prefix(&super::lexically_normalize(workspace_root)?);
        let source_path =
            super::canonicalize_existing_prefix(&super::lexically_normalize(source_path)?);
        if !source_path.starts_with(&root) {
            return None;
        }
        let mut candidates: Vec<(PreviewAssetKind, PathBuf)> = Vec::new();
        let mut values = vec![self.value.clone()];
        if Path::new(&self.value).extension().is_none() && self.kind == AssetCallKind::Image {
            values = ["pdf", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff"]
                .into_iter()
                .map(|extension| format!("{}.{extension}", self.value))
                .collect();
        }
        for value in &values {
            let Some(kind) = super::preview_asset_kind(value) else {
                continue;
            };
            if self.kind == AssetCallKind::Include && kind != PreviewAssetKind::Pdf {
                continue;
            }
            let target = Path::new(value);
            for candidate in if target.is_absolute() {
                vec![super::canonicalize_existing_prefix(
                    &super::lexically_normalize(target)?,
                )]
            } else {
                [source_path.parent()?.join(target), root.join(target)]
                    .into_iter()
                    .filter_map(|path| super::lexically_normalize(&path))
                    .map(|path| super::canonicalize_existing_prefix(&path))
                    .collect()
            } {
                if candidate.starts_with(&root)
                    && candidate.is_file()
                    && !candidates.iter().any(|(_, path)| path == &candidate)
                {
                    candidates.push((kind, candidate));
                }
            }
        }
        // Avoid opening the wrong file when TeX's compile directory or graphics
        // search paths would be needed to distinguish multiple literal matches.
        (candidates.len() == 1).then(|| candidates.pop().unwrap())
    }
}

fn trivia(source: &str, mut at: usize, syntax: &TexDelimiterIndex) -> usize {
    loop {
        let previous = at;
        while let Some(ch) = source[at..].chars().next().filter(|ch| ch.is_whitespace()) {
            at += ch.len_utf8();
        }
        if source.as_bytes().get(at) == Some(&b'%') && syntax.ignored_at(at) {
            at = source[at..]
                .find('\n')
                .map_or(source.len(), |offset| at + offset + 1);
        }
        if at == previous {
            return at;
        }
    }
}

fn group(
    source: &str,
    at: usize,
    open: u8,
    syntax: &TexDelimiterIndex,
) -> Option<(Range<usize>, usize)> {
    if source.as_bytes().get(at) != Some(&open) {
        return None;
    }
    let pair = syntax.pair_at(at)?;
    (pair[0] == (at..at + 1)).then_some((at + 1..pair[1].start, pair[1].end))
}

fn literal(source: &str, command: &str) -> Option<String> {
    let mut result = String::new();
    let mut chars = source.trim().chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let escaped = chars.next()?;
            if !matches!(escaped, '%' | '#' | '&' | '_' | '{' | '}' | ' ' | '~') {
                return None;
            }
            result.push(escaped);
        } else {
            let parameter = ch == '#'
                && (command != "url" && chars.clone().next().is_some_and(|ch| ch.is_ascii_digit()));
            if ch.is_control()
                || matches!(ch, '{' | '}')
                || ch == '#' && (!matches!(command, "url" | "href") || parameter)
            {
                return None;
            }
            result.push(ch);
        }
    }
    (!result.is_empty()).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tex_links_resolve_literal_targets_and_ignore_comments_macros_and_verbatim() {
        let source = "你好 \\href{https://example.com/a\\_b?x=1\\&y=2}{Readable label} \\url{https://example.org/%20}\n% \\url{https://wrong.test}\n\\verb|\\url{https://wrong.test}|\n\\href{\\computed}{Dynamic}\n";
        let index = Index::new(source, &TexDelimiterIndex::new(source));
        assert_eq!(
            index.link_at(source.find("Readable").unwrap()),
            Some("https://example.com/a_b?x=1&y=2")
        );
        assert_eq!(
            index.link_at(source.find("example.org").unwrap()),
            Some("https://example.org/%20")
        );
        for marker in ["wrong.test", "Dynamic"] {
            assert!(index.link_at(source.find(marker).unwrap()).is_none());
        }
    }

    #[test]
    fn tex_asset_targets_support_options_and_unique_extensionless_workspace_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("figures")).unwrap();
        std::fs::write(root.path().join("figures/plot.pdf"), b"pdf fixture").unwrap();
        let source = "\\includegraphics[width=.5\\textwidth]{figures/plot}\n\\includepdf[pages=-]{figures/plot.pdf}\n";
        let index = Index::new(source, &TexDelimiterIndex::new(source));
        for at in [
            source.find("figures/plot").unwrap(),
            source.rfind("figures/plot").unwrap(),
        ] {
            let asset = index.asset_at(at).unwrap();
            assert_eq!(
                asset.resolve(&root.path().join("main.tex"), root.path()),
                Some((
                    PreviewAssetKind::Pdf,
                    root.path().join("figures/plot.pdf").canonicalize().unwrap()
                ))
            );
        }
        std::fs::write(root.path().join("figures/plot.png"), b"png fixture").unwrap();
        assert!(
            index
                .asset_at(source.find("figures/plot").unwrap())
                .unwrap()
                .resolve(&root.path().join("main.tex"), root.path())
                .is_none()
        );
    }

    #[test]
    fn tex_href_accepts_literal_fragments_without_expanding_macro_parameters() {
        let source = "\\href{https://example.com/#section}{Anchor} \\href{https://example.com/#1}{Parameter}";
        let index = Index::new(source, &TexDelimiterIndex::new(source));
        assert_eq!(
            index.link_at(source.find("Anchor").unwrap()),
            Some("https://example.com/#section")
        );
        assert_eq!(index.link_at(source.find("Parameter").unwrap()), None);
    }

    #[test]
    fn tex_asset_targets_reject_dynamic_external_and_verbatim_paths() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outside.pdf"), b"outside").unwrap();
        let source = format!(
            "\\includegraphics{{{}/outside.pdf}}\n\\includegraphics{{\\dynamic}}\n\\begin{{verbatim}}\n\\includegraphics{{figure.pdf}}\n\\end{{verbatim}}\n",
            outside.path().display()
        );
        let index = Index::new(&source, &TexDelimiterIndex::new(&source));
        assert!(
            index
                .asset_at(source.find("outside.pdf").unwrap())
                .unwrap()
                .resolve(&root.path().join("main.tex"), root.path())
                .is_none()
        );
        assert!(index.asset_at(source.find("dynamic").unwrap()).is_none());
        assert!(index.asset_at(source.find("figure.pdf").unwrap()).is_none());
    }
}
