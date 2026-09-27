//! Private TeX service document with explicit, reversible math-region boundaries.
use std::ops::Range;
use tiptoptyp_core::text::{
    LspRange, LspTextEdit, ScalarOffset, lsp_position_at_scalar, range_to_scalar_range,
};

pub(crate) struct Projection {
    pub source: String,
    regions: Vec<(Range<usize>, Range<usize>)>,
}
impl Projection {
    #[cfg(test)]
    pub(crate) fn new(display: &str) -> Self {
        Self::from_parsed(&typst_syntax::Source::detached(display))
    }
    pub(crate) fn from_parsed(parsed: &typst_syntax::Source) -> Self {
        let display = parsed.text();
        let mut source =
            "\\documentclass{article}\n\\usepackage{amsmath,amssymb}\n\\begin{document}\n"
                .to_string();
        let mut regions = Vec::new();
        let mut virtual_chars = source.chars().count();
        let (mut previous_byte, mut physical_chars) = (0, 0);
        for (index, range) in tiptoptyp::mitex_projection::dollar_payloads(parsed)
            .into_iter()
            .enumerate()
        {
            let header = format!("% tiptoptyp-math-{index}-start\n$");
            let footer = format!("$\n% tiptoptyp-math-{index}-end\n");
            source.push_str(&header);
            let start = virtual_chars + header.chars().count();
            let payload = &display[range.clone()];
            let count = payload.chars().count();
            source.push_str(payload);
            let end = start + count;
            source.push_str(&footer);
            virtual_chars = end + footer.chars().count();
            physical_chars += display[previous_byte..range.start].chars().count();
            let original = physical_chars..physical_chars + count;
            physical_chars += count;
            previous_byte = range.end;
            regions.push((original, start..end));
        }
        source.push_str("\\end{document}\n");
        Self { source, regions }
    }
    pub(crate) fn map_range(&self, display: &str, range: &LspRange) -> Option<LspRange> {
        let range = range_to_scalar_range(&self.source, range).into_range();
        let (original, virtual_range) = self
            .regions
            .iter()
            .find(|(_, r)| r.start <= range.start && range.end <= r.end)?;
        Some(LspRange {
            start: lsp_position_at_scalar(
                display,
                ScalarOffset::new(original.start + range.start - virtual_range.start),
            ),
            end: lsp_position_at_scalar(
                display,
                ScalarOffset::new(original.start + range.end - virtual_range.start),
            ),
        })
    }
    pub(crate) fn formatted_edits(
        &self,
        display: &str,
        formatted: &str,
    ) -> Result<Vec<LspTextEdit>, String> {
        self.regions
            .iter()
            .enumerate()
            .map(|(index, (original, _))| {
                let start = format!("% tiptoptyp-math-{index}-start");
                let end = format!("% tiptoptyp-math-{index}-end");
                if formatted.matches(&start).count() != 1 || formatted.matches(&end).count() != 1 {
                    return Err("Formatter returned ambiguous math boundaries".into());
                }
                let (_, tail) = formatted
                    .split_once(&start)
                    .ok_or("Formatter removed a math boundary")?;
                let (region, _) = tail
                    .split_once(&end)
                    .ok_or("Formatter removed a math boundary")?;
                let payload = region
                    .trim()
                    .strip_prefix('$')
                    .and_then(|s| s.strip_suffix('$'))
                    .ok_or("Formatter changed a math delimiter; leaving source unchanged")?;
                Ok(LspTextEdit {
                    range: LspRange {
                        start: lsp_position_at_scalar(display, ScalarOffset::new(original.start)),
                        end: lsp_position_at_scalar(display, ScalarOffset::new(original.end)),
                    },
                    new_text: payload.into(),
                })
            })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formatting_only_replaces_math_payloads_and_maps_unicode_locations() {
        let display = "😀 Typst #text[keep] $\\alpha+ x$ and $ y^2 $.";
        let projection = Projection::new(display);
        let formatted = projection.source.replace("\\alpha+ x", "\\alpha + x");
        let edits = projection.formatted_edits(display, &formatted).unwrap();
        let applied =
            tiptoptyp_core::text::apply_text_edits(display, &edits, [ScalarOffset::new(0); 2])
                .unwrap();
        assert_eq!(
            applied.text,
            "😀 Typst #text[keep] $\\alpha + x$ and $ y^2 $."
        );
        assert!(projection.formatted_edits(display, "broken").is_err());
        let (physical, virtual_range) = &projection.regions[0];
        let at = lsp_position_at_scalar(&projection.source, ScalarOffset::new(virtual_range.start));
        assert_eq!(
            projection
                .map_range(display, &LspRange { start: at, end: at })
                .unwrap()
                .start,
            lsp_position_at_scalar(display, ScalarOffset::new(physical.start))
        );
    }
}
