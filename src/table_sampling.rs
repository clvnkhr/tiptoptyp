//! Bounded, cancellable compiler samples. Values are display-only: never source.
use crate::{
    editor_features::CellPosition, private_workspace::PrivateWorkspace, process::OwnedChild,
    tool_command::CommandCustomization,
};
use eframe::egui;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    ops::Range,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_OUTPUT: u64 = 8 * 1024 * 1024;
const MAX_CELL: usize = 16 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cell {
    pub position: CellPosition,
    pub span: CellPosition,
    pub value: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Grid {
    pub columns: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
}
impl Grid {
    pub(crate) fn value(&self, position: CellPosition) -> Option<&str> {
        if position.0 >= self.rows || position.1 >= self.columns {
            return None;
        }
        self.cells
            .iter()
            .find(|c| c.position == position)
            .map(|c| c.value.as_str())
    }
}
#[derive(Debug, Clone, Default)]
pub(crate) struct View {
    pub locked: BTreeMap<CellPosition, String>,
    pub grid: Option<Grid>,
    pub pending: bool,
    pub error: Option<String>,
}
impl View {
    pub(crate) fn locked_value(&self, position: CellPosition) -> Option<&str> {
        self.grid.as_ref()?.value(position)
    }
}
fn key(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::new((viewport, "table-runtime-samples"))
}
pub(crate) fn publish(context: &egui::Context, viewport: egui::ViewportId, view: Arc<View>) {
    context.data_mut(|data| data.insert_temp(key(viewport), view));
}
pub(crate) fn view(ui: &egui::Ui) -> Option<Arc<View>> {
    ui.ctx()
        .data(|data| data.get_temp(key(ui.ctx().viewport_id())))
}
pub(crate) fn clear(context: &egui::Context, viewport: egui::ViewportId) {
    context.data_mut(|data| {
        data.remove::<Arc<View>>(key(viewport));
    });
}

#[derive(Debug, Clone)]
pub(crate) enum Engine {
    Typst {
        command: CommandCustomization,
        fonts: Vec<PathBuf>,
        kind: String,
    },
    Tex {
        command: CommandCustomization,
        standard: bool,
        only_cached: bool,
        cells: BTreeMap<CellPosition, String>,
        columns: usize,
        rows: usize,
    },
}
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub root: PathBuf,
    pub source_dir: PathBuf,
    pub name: String,
    pub source: String,
    pub range: Range<usize>,
    pub call: String,
    pub program: PathBuf,
    pub engine: Engine,
}
pub(crate) fn computed_markup(source: &str) -> bool {
    fn hash(node: &typst_syntax::SyntaxNode) -> bool {
        matches!(
            node.kind(),
            typst_syntax::SyntaxKind::Hash | typst_syntax::SyntaxKind::MathIdent
        ) || node.children().any(hash)
    }
    hash(typst_syntax::Source::detached(source).root())
}

/// Sample a private mirror of the current buffer. Relative resources/imports
/// keep their project paths, and no PDF or generated source reaches user files.
pub(crate) fn sample(request: Request, cancel: &AtomicBool) -> Result<Grid, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Sampling cancelled".into());
    }
    let private = PrivateWorkspace::open(&request.root).map_err(|e| e.to_string())?;
    let label = format!("tiptoptyp-private-table-{}", next_nonce());
    let (replacement, expression) = match &request.engine {
        Engine::Typst { kind, .. } => (
            format!(
                "{{ show {kind}: it => [#metadata(it) <{label}>#it]; {} }}",
                request.call
            ),
            format!("query(<{label}>).map(it => it.value)"),
        ),
        Engine::Tex { .. } => (request.call.clone(), String::new()),
    };
    let mut source = request.source.clone();
    if source.get(request.range.clone()).is_none() {
        return Err("The table range changed".into());
    }
    source.replace_range(request.range.clone(), &replacement);
    let mirror = private
        .mirrored_source(&request.source_dir, &request.name, &source)
        .map_err(|e| e.to_string())?;
    let output = mirror.session_dir().join("sample-output");
    fs::create_dir(&output).map_err(|e| e.to_string())?;
    let stdout = mirror.session_dir().join("stdout");
    let stderr = mirror.session_dir().join("stderr");
    let mut command = Command::new(&request.program);
    command
        .current_dir(mirror.path().parent().unwrap())
        .env("NO_COLOR", "1");
    match &request.engine {
        Engine::Typst {
            command: custom,
            fonts,
            ..
        } => {
            // Preserve compiler inputs, fonts and package choices; only the
            // operation changes from watch to eval in this private request.
            let mut custom = custom.clone();
            let mut tokens =
                shlex::split(&custom.arguments).ok_or("Typst arguments have an unmatched quote")?;
            if tokens.first().is_some_and(|token| token == "watch") {
                tokens.remove(0);
            }
            custom.arguments = std::iter::once("eval".to_owned())
                .chain(tokens)
                .map(|token| {
                    shlex::try_quote(&token)
                        .map(|x| x.into_owned())
                        .map_err(|e| e.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
                .join(" ");
            command
                .arg(expression)
                .arg("--in")
                .arg(mirror.path())
                .arg("--root")
                .arg(mirror.mirror_root());
            for font in fonts {
                command.arg("--font-path").arg(font);
            }
            custom.apply(&mut command).map_err(|e| e.to_string())?;
        }
        Engine::Tex {
            command: custom,
            standard,
            only_cached,
            ..
        } => {
            if *standard {
                command
                    .args([
                        "-interaction=nonstopmode",
                        "-halt-on-error",
                        "-no-shell-escape",
                    ])
                    .arg(format!("-output-directory={}", output.display()));
            } else {
                command.arg("--outdir").arg(&output);
                let tokens = shlex::split(&custom.arguments)
                    .ok_or("TeX arguments have an unmatched quote")?;
                for required in ["--keep-logs", "--untrusted"] {
                    if !tokens.iter().any(|token| token == required) {
                        command.arg(required);
                    }
                }
                if *only_cached {
                    command.arg("--only-cached");
                }
            }
            command.arg(mirror.path());
            custom.apply(&mut command).map_err(|e| e.to_string())?;
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            File::create(&stdout).map_err(|e| e.to_string())?,
        ))
        .stderr(Stdio::from(
            File::create(&stderr).map_err(|e| e.to_string())?,
        ));
    let mut child =
        OwnedChild::spawn(&mut command).map_err(|e| format!("Cannot sample this table: {e}"))?;
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("Sampling cancelled".into());
        }
        for path in [&stdout, &stderr] {
            if fs::metadata(path).is_ok_and(|m| m.len() > MAX_OUTPUT) {
                return Err("Table samples exceeded the output limit".into());
            }
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                return Err(format!(
                    "Table sampling failed. {}",
                    bounded_text(&stderr)?
                        .chars()
                        .rev()
                        .take(1200)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<String>()
                ));
            }
            break;
        }
        if started.elapsed() > TIMEOUT {
            return Err("Table sampling timed out; the expressions remain locked".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    match request.engine {
        Engine::Typst { .. } => typst_grid(&bounded_text(&stdout)?),
        Engine::Tex {
            columns,
            rows,
            cells,
            ..
        } => {
            let log = output.join(&request.name).with_extension("log");
            tex_grid(&bounded_text(&log)?, columns, rows, &cells)
        }
    }
}
fn next_nonce() -> u64 {
    static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NONCE.fetch_add(1, Ordering::Relaxed)
}
fn bounded_text(path: &std::path::Path) -> Result<String, String> {
    let mut text = String::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_OUTPUT + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_OUTPUT {
        return Err("Table samples exceeded the output limit".into());
    }
    Ok(text)
}
fn append_text(output: &mut String, text: &str) {
    for ch in text.chars() {
        if output.len() + ch.len_utf8() > MAX_CELL {
            break;
        }
        output.push(ch);
    }
}
fn display_text(value: &serde_json::Value, output: &mut String, depth: usize) {
    if depth > 32 || output.len() >= MAX_CELL {
        return;
    }
    if let Some(text) = value.as_str() {
        append_text(output, text);
        return;
    }
    if let Some(values) = value.as_array() {
        for value in values {
            display_text(value, output, depth + 1);
        }
        return;
    }
    if let Some(text) = value.get("text").and_then(|x| x.as_str()) {
        append_text(output, text);
    } else if value["func"] == "space" {
        output.push(' ');
    } else if matches!(value["func"].as_str(), Some("linebreak" | "parbreak")) {
        output.push('\n');
    } else if let Some(children) = value.get("children") {
        display_text(children, output, depth + 1);
    } else if let Some(body) = value.get("body") {
        display_text(body, output, depth + 1);
    } else if let Some(expr) = value.get("alt").and_then(|x| x.as_str()) {
        append_text(output, expr);
    } else {
        output.push('…');
    }
}
pub(crate) fn typst_grid(text: &str) -> Result<Grid, String> {
    let instances: Vec<serde_json::Value> =
        serde_json::from_str(text).map_err(|e| format!("Cannot read table values: {e}"))?;
    let table = instances
        .first()
        .ok_or("The table did not run in this document; its expressions remain locked")?;
    let columns = table
        .get("columns")
        .map_or(Some(1), |v| {
            v.as_array()
                .map(Vec::len)
                .or_else(|| v.as_u64().and_then(|n| usize::try_from(n).ok()))
        })
        .ok_or("Cannot resolve table columns")?;
    if columns == 0 || columns > 128 {
        return Err("Sampled tables support 1–128 columns".into());
    }
    let mut children = Vec::new();
    fn collect<'a>(value: &'a serde_json::Value, cells: &mut Vec<&'a serde_json::Value>) {
        if cells.len() > 4096 {
            return;
        }
        match value["func"].as_str() {
            Some("header" | "footer") => {
                if let Some(children) = value["children"].as_array() {
                    for child in children {
                        collect(child, cells);
                    }
                }
            }
            Some("hline" | "vline") => {}
            _ => cells.push(value),
        }
    }
    for child in table["children"]
        .as_array()
        .ok_or("Cannot resolve table cells")?
    {
        collect(child, &mut children);
    }
    if children.len() > 4096 {
        return Err("Sampled table exceeds 4096 cells".into());
    }
    let mut occupied = vec![false; 4096];
    let mut grid = Grid {
        columns,
        ..Default::default()
    };
    let mut next = 0;
    for child in children {
        while next < occupied.len() && occupied[next] {
            next += 1;
        }
        let x = child["x"]
            .as_u64()
            .map_or(next % columns, |n| usize::try_from(n).unwrap_or(usize::MAX));
        let y = child["y"]
            .as_u64()
            .map_or(next / columns, |n| usize::try_from(n).unwrap_or(usize::MAX));
        let colspan = usize::try_from(child["colspan"].as_u64().unwrap_or(1)).unwrap_or(usize::MAX);
        let rowspan = usize::try_from(child["rowspan"].as_u64().unwrap_or(1)).unwrap_or(usize::MAX);
        if colspan == 0
            || rowspan == 0
            || x.saturating_add(colspan) > columns
            || y.saturating_add(rowspan).saturating_mul(columns) > occupied.len()
        {
            return Err("Unsupported sampled cell placement".into());
        }
        for row in y..y + rowspan {
            for column in x..x + colspan {
                let slot = &mut occupied[row * columns + column];
                if *slot {
                    return Err("Overlapping sampled cells".into());
                }
                *slot = true;
            }
        }
        let mut value = String::new();
        display_text(child.get("body").unwrap_or(child), &mut value, 0);
        grid.rows = grid.rows.max(y + rowspan);
        grid.cells.push(Cell {
            position: (y, x),
            span: (rowspan, colspan),
            value,
        });
    }
    Ok(grid)
}
pub(crate) fn tex_instrument(
    table: &crate::editor_features::TexTable,
    locked: &BTreeMap<CellPosition, String>,
) -> Result<String, String> {
    let mut table = table.clone();
    for (&(r, c), expected) in locked {
        let content = table
            .cell_content_range((r, c))
            .ok_or("The TeX cell geometry changed")?;
        let cell = table
            .cells
            .as_mut()
            .and_then(|rows| rows.get_mut(r))
            .and_then(|row| row.get_mut(c))
            .ok_or("The TeX cell geometry changed")?;
        if cell != expected {
            return Err("The TeX cell source changed before sampling".into());
        }
        let body = &cell[content.clone()];
        let instrumented = format!(
            "{{\\csname protected@edef\\endcsname\\TTTPrivateTableSample{{{body}}}\\typeout{{TTT-SAMPLE-BEGIN:{r}:{c}}}\\typeout{{\\meaning\\TTTPrivateTableSample}}\\typeout{{TTT-SAMPLE-END}}{body}}}"
        );
        cell.replace_range(content, &instrumented);
    }
    table.render_source_draft()
}
fn tex_grid(
    log: &str,
    columns: usize,
    rows: usize,
    expected: &BTreeMap<CellPosition, String>,
) -> Result<Grid, String> {
    let mut grid = Grid {
        columns,
        rows,
        ..Default::default()
    };
    let mut current = None;
    let mut value = String::new();
    for line in log.lines() {
        if let Some(at) = line.strip_prefix("TTT-SAMPLE-BEGIN:") {
            current = at
                .split_once(':')
                .and_then(|(r, c)| Some((r.parse::<usize>().ok()?, c.parse::<usize>().ok()?)));
            value.clear();
        } else if line == "TTT-SAMPLE-END" {
            if let Some(position) = current.take().filter(|at| expected.contains_key(at)) {
                grid.cells.retain(|cell| cell.position != position);
                let expansion = value
                    .split_once("->")
                    .map_or(value.as_str(), |(_, v)| v)
                    .trim();
                grid.cells.push(Cell {
                    position,
                    span: (1, 1),
                    value: tex_sample_symbol(expansion).unwrap_or(expansion).into(),
                });
            }
        } else if current.is_some() {
            if line.len() > MAX_CELL.saturating_sub(value.len()) {
                return Err(
                    "A TeX table sample exceeded the output limit; its expression remains locked"
                        .into(),
                );
            }
            value.push_str(line);
        }
    }
    if grid.cells.len() != expected.len() {
        return Err(
            "The compiler could not sample every variable cell; their expressions remain locked"
                .into(),
        );
    }
    Ok(grid)
}

// Only decode the two complete Pifont expansions used for table checks/crosses.
// Other compiler tokens remain visible; this is not a general TeX renderer.
fn tex_sample_symbol(value: &str) -> Option<&'static str> {
    fn padding(mut value: &str) -> &str {
        loop {
            value = value.trim_matches(|c: char| c.is_ascii_whitespace());
            if let Some(rest) = value.strip_prefix("{}") {
                value = rest;
            } else if let Some(rest) = value.strip_suffix("{}") {
                value = rest;
            } else {
                return value;
            }
        }
    }
    fn command(input: &mut &str, expected: &str) -> Option<()> {
        *input = padding(input);
        for _ in 0..16 {
            let rest = input.strip_prefix(r"\protect");
            if let Some(rest) =
                rest.filter(|rest| !rest.starts_with(|c: char| c.is_ascii_alphabetic()))
            {
                *input = padding(rest);
            } else {
                let rest = input.strip_prefix(expected)?;
                if rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
                    return None;
                }
                *input = rest;
                return Some(());
            }
        }
        None
    }
    fn argument(input: &mut &str, expected: &str) -> Option<()> {
        let rest = input.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let (value, rest) = rest.strip_prefix('{')?.split_once('}')?;
        if value != expected {
            return None;
        }
        *input = rest;
        Some(())
    }

    if value.len() > MAX_CELL {
        return None;
    }
    let input = padding(value);
    let (input, starred) = if let Some(math) = input.strip_suffix('$') {
        (
            math.trim_end_matches(|c: char| c.is_ascii_whitespace())
                .strip_suffix(r"$\ast")?,
            true,
        )
    } else {
        (input, false)
    };
    let mut input = padding(input);
    for _ in 0..16 {
        if !input.starts_with('{') {
            break;
        }
        let mut depth = 0usize;
        let closing = input.bytes().position(|byte| {
            match byte {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                _ => (),
            }
            depth == 0
        })?;
        if closing != input.len() - 1 {
            return None;
        }
        input = padding(&input[1..closing]);
    }
    for (name, value) in [
        (r"\fontfamily", "pzd"),
        (r"\fontencoding", "U"),
        (r"\fontseries", "m"),
        (r"\fontshape", "n"),
    ] {
        command(&mut input, name)?;
        argument(&mut input, value)?;
    }
    command(&mut input, r"\selectfont")?;
    command(&mut input, r"\char")?;
    input = input.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let (symbol, rest) = if let Some(rest) = input.strip_prefix("51") {
        ("✓", rest)
    } else {
        ("✗", input.strip_prefix("55")?)
    };
    if !padding(rest).is_empty() {
        return None;
    }
    Some(match (symbol, starred) {
        ("✓", true) => "✓∗",
        ("✗", true) => "✗∗",
        _ => symbol,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn table_samples_preserve_resolved_placement_headers_spans_and_unicode() {
        let table = json!([{"columns":["auto","auto","auto"],"children":[
            {"func":"header","children":[{"func":"cell","colspan":2,"body":{"func":"strong","body":{"func":"text","text":"Revenue λ"}}}]},
            {"func":"hline"}, {"func":"cell","body":{"func":"text","text":"60"}},
            {"func":"cell","x":1,"y":2,"rowspan":2,"body":{"func":"sequence","children":[{"func":"text","text":"A"},{"func":"space"},{"func":"text","text":"B"}]}}
        ]}]);
        let grid = typst_grid(&table.to_string()).unwrap();
        assert_eq!((grid.columns, grid.rows), (3, 4));
        assert_eq!(grid.value((0, 0)), Some("Revenue λ"));
        assert_eq!(grid.cells[0].span, (1, 2));
        assert_eq!(grid.value((0, 2)), Some("60"));
        assert_eq!(grid.value((2, 1)), Some("A B"));
    }
    #[test]
    fn table_samples_reject_unbounded_and_overlapping_placement() {
        for table in [
            json!([]),
            json!([{"columns":129,"children":[]}]),
            json!([{"columns":2,"children":[{"y":u64::MAX}]}]),
            json!([{"columns":2,"children":[{"colspan":u64::MAX}]}]),
            json!([{"columns":2,"children":[{"rowspan":u64::MAX}]}]),
            json!([{"columns":2,"children":[{"x":0,"y":0},{"x":0,"y":0}]}]),
        ] {
            assert!(typst_grid(&table.to_string()).is_err(), "{table}");
        }
        let mut output = String::new();
        display_text(&json!({"alt":"λ".repeat(MAX_CELL)}), &mut output, 0);
        assert_eq!(output.len(), MAX_CELL);
    }
    #[test]
    fn table_samples_detect_computed_markup_but_not_escaped_hashes() {
        assert!(computed_markup("#price"));
        assert!(computed_markup("$price$"));
        assert!(computed_markup("Total: #calc(2)"));
        assert!(!computed_markup(r"Total: \#price"));
        assert!(!computed_markup("plain *bold* text"));
    }
    #[test]
    fn table_samples_tex_instrumentation_preserves_structural_cell_wrappers() {
        for (cell, columns, tail) in [
            (r"\multicolumn{2}{c}{\revenue}", 2, ""),
            (r"\multirow{2}{*}{\textbf{\revenue}}", 1, r"\\"),
            (r"\thead{\multirow{2}{*}{\revenue}}", 1, r"\\"),
        ] {
            let source = format!(
                r"\begin{{tabular}}{{{}}}{cell}\\{tail}\end{{tabular}}",
                "l".repeat(columns)
            );
            let table = crate::editor_features::tex_table_at(&source, 2).unwrap();
            let locked = BTreeMap::from([((0, 0), cell.to_owned())]);
            let instrumented = tex_instrument(&table, &locked).unwrap();
            assert!(
                instrumented.contains(&format!(
                    "{cell_prefix}{{\\csname protected@edef",
                    cell_prefix = cell.split(r"\revenue").next().unwrap()
                )),
                "sampling must remain inside its original wrappers: {instrumented}"
            );
            let reparsed = crate::editor_features::tex_table_at(&instrumented, 2).unwrap();
            assert_eq!(reparsed.cell_span((0, 0)), table.cell_span((0, 0)));
            assert!(instrumented.contains("TTT-SAMPLE-BEGIN:0:0"));
            assert_eq!(table.source, source);
        }
    }

    #[test]
    fn table_samples_tex_instrumentation_rejects_stale_locked_cells() {
        let source = r"\begin{tabular}{ll}Label & \revenue\\\end{tabular}";
        let table = crate::editor_features::tex_table_at(source, 2).unwrap();
        assert!(tex_instrument(&table, &BTreeMap::from([((0, 1), r"\stale".into())])).is_err());
        assert!(tex_instrument(&table, &BTreeMap::from([((3, 3), r"\revenue".into())])).is_err());
        assert_eq!(table.source, source);
    }

    #[test]
    fn table_samples_tex_log_keeps_whole_expansions_and_requires_all_cells() {
        let expected = BTreeMap::from([((0, 1), r"\revenue".into())]);
        let log = "TTT-SAMPLE-BEGIN:0:1\nmacro:->60 λ\nTTT-SAMPLE-END\n";
        assert_eq!(
            tex_grid(log, 2, 1, &expected).unwrap().value((0, 1)),
            Some("60 λ")
        );
        assert!(tex_grid("", 2, 1, &expected).is_err());
        assert!(
            tex_grid(
                "TTT-SAMPLE-BEGIN:0:2\nmacro:->wrong\nTTT-SAMPLE-END",
                2,
                1,
                &expected
            )
            .is_err()
        );
    }

    #[test]
    fn table_samples_tex_log_rejects_oversized_values_before_decoding_a_valid_prefix() {
        let expected = BTreeMap::from([((0, 0), r"\cmark".into())]);
        let prefix = r"macro:->{\fontfamily{pzd}\fontencoding{U}\fontseries{m}\fontshape{n}\selectfont\char51}";
        for suffix in [
            "x".repeat(MAX_CELL),
            format!("{}\n{}", "x".repeat(MAX_CELL / 2), "x".repeat(MAX_CELL / 2)),
        ] {
            let log = format!("TTT-SAMPLE-BEGIN:0:0\n{prefix}\n{suffix}\nTTT-SAMPLE-END\n");
            let error = tex_grid(&log, 1, 1, &expected)
                .expect_err("an oversized suffix must not leave a decodable truncated prefix");
            assert!(error.contains("output limit"), "{error}");
        }
        let complete = "x".repeat(MAX_CELL - "macro:->".len());
        let log = format!("TTT-SAMPLE-BEGIN:0:0\nmacro:->{complete}\nTTT-SAMPLE-END\n");
        assert_eq!(
            tex_grid(&log, 1, 1, &expected).unwrap().value((0, 0)),
            Some(complete.as_str())
        );
    }

    #[test]
    fn table_samples_tex_pifont_expansions_display_as_checks_and_crosses() {
        let expected = BTreeMap::from([((0, 0), r"\cmark".into()), ((0, 1), r"\xmark".into())]);
        let expansion = r"{\protect \fontfamily  {pzd}\protect \fontencoding  {U}\protect \fontse
ries  {m}\protect \fontshape  {n}\protect \selectfont  \char ";
        let log = format!(
            "TTT-SAMPLE-BEGIN:0:0\nmacro:->{expansion}51}}\nTTT-SAMPLE-END\nTTT-SAMPLE-BEGIN:0:1\nmacro:->{expansion}55}}\nTTT-SAMPLE-END\n"
        );
        let grid = tex_grid(&log, 2, 1, &expected).unwrap();
        assert_eq!(grid.value((0, 0)), Some("✓"));
        assert_eq!(grid.value((0, 1)), Some("✗"));
        assert_eq!(expected[&(0, 0)], r"\cmark");
    }

    #[test]
    fn table_samples_tex_log_decodes_exact_pifont_math_asterisk_suffixes() {
        let expected = BTreeMap::from([
            ((0, 0), r"\cmark$\ast$".into()),
            ((0, 1), r"\xmark$\ast$".into()),
        ]);
        let expansion = r"{\protect \fontfamily  {pzd}\protect \fontencoding  {U}\protect \fontse
ries  {m}\protect \fontshape  {n}\protect \selectfont  \char ";
        let log = format!(
            "TTT-SAMPLE-BEGIN:0:0\nmacro:->{expansion}51}}$\\ast $\nTTT-SAMPLE-END\nTTT-SAMPLE-BEGIN:0:1\nmacro:->{expansion}55}}$\\ast $\nTTT-SAMPLE-END\n"
        );
        let grid = tex_grid(&log, 2, 1, &expected).unwrap();
        assert_eq!(grid.value((0, 0)), Some("✓∗"));
        assert_eq!(grid.value((0, 1)), Some("✗∗"));
        assert_eq!(expected[&(0, 0)], r"\cmark$\ast$");
    }

    #[test]
    fn table_samples_tex_symbol_decoder_only_accepts_complete_known_pifont_values() {
        let check =
            r"\fontfamily{pzd}\fontencoding{U}\fontseries{m}\fontshape{n}\selectfont\char51";
        assert_eq!(tex_sample_symbol(check), Some("✓"));
        assert_eq!(
            tex_sample_symbol(&format!("{check}{}", r"$\ast $")),
            Some("✓∗")
        );
        assert_eq!(
            tex_sample_symbol(&format!("{}{}", check.replace("51", "55"), r"$\ast$")),
            Some("✗∗")
        );
        assert_eq!(
            tex_sample_symbol(&format!("{{{{{check}}}}}{{}}")),
            Some("✓")
        );
        for unsupported in [
            check.replace("pzd", "cmr"),
            check.replace("pzd", " pzd "),
            check.replace("{U}", "{T1}"),
            check.replace("{m}", "{b}"),
            check.replace("{n}", "{it}"),
            check.replace("51", "52"),
            check.replace("51", "56"),
            check.replace("51", "5 1"),
            check.replace("fontfamily", "fontfam ily"),
            format!("{check}extra"),
            format!("{check}{}", r"$\star$"),
            format!("{check}{}", r"$\ast + 1$"),
            format!("{check}{}", r"$\ast$\write{stream}"),
            format!("{check}{}", r"\write{stream}"),
            format!("{{{check}"),
            format!("{}{}{}", "{".repeat(17), check, "}".repeat(17)),
            r"\char51".into(),
            "60 λ".into(),
            "x".repeat(MAX_CELL + 1),
        ] {
            assert_eq!(tex_sample_symbol(&unsupported), None, "{unsupported}");
        }
    }

    #[test]
    fn table_samples_cancel_before_creating_private_files() {
        let dir = tempfile::tempdir().unwrap();
        let request = Request {
            root: dir.path().into(),
            source_dir: dir.path().into(),
            name: "sample.typ".into(),
            source: "".into(),
            range: 0..0,
            call: "".into(),
            program: "unused".into(),
            engine: Engine::Typst {
                command: CommandCustomization::default(),
                fonts: vec![],
                kind: "table".into(),
            },
        };
        assert!(
            sample(request, &AtomicBool::new(true))
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(!dir.path().join(".tiptoptyp").exists());
    }
    #[test]
    #[cfg(unix)]
    fn table_samples_cancel_running_compiler_and_clean_private_mirror() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("sample-tool");
        fs::write(
            &script,
            "#!/bin/sh\nprintf ready > \"$TTT_STARTED\"\nexec sleep 60\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let marker = dir.path().join("started");
        let request = Request {
            root: dir.path().into(),
            source_dir: dir.path().into(),
            name: "sample.typ".into(),
            source: "#table([A])".into(),
            range: 1..11,
            call: "table([A])".into(),
            program: script,
            engine: Engine::Typst {
                command: CommandCustomization {
                    environment: serde_json::json!({"TTT_STARTED":marker}).to_string(),
                    ..Default::default()
                },
                fonts: vec![],
                kind: "table".into(),
            },
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || sample(request, &worker_cancel));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && !worker.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        cancel.store(true, Ordering::Release);
        let result = worker.join().unwrap();
        assert!(marker.exists(), "mock compiler did not start: {result:?}");
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(
            fs::read_dir(dir.path().join(".tiptoptyp"))
                .unwrap()
                .next()
                .is_none()
        );
    }
    #[test]
    #[ignore = "requires TIPTOPTYP_TEST_TYPST executable"]
    fn table_samples_resolve_actual_typst_variables_spreads_inherited_columns_and_cleanup() {
        let program = std::env::var_os("TIPTOPTYP_TEST_TYPST").expect("set TIPTOPTYP_TEST_TYPST");
        let directory = tempfile::tempdir().unwrap();
        // Include a project-relative import to exercise the private mirror.
        fs::write(directory.path().join("data.typ"), "#let amount = 60").unwrap();
        let source = "#import \"data.typ\": amount\n#set table(columns: 2)\n#let rows = ([Revenue], [#amount])\n#table(..rows)".to_owned();
        let start = source.find("table(..rows)").unwrap();
        fs::write(directory.path().join("sample.typ"), &source).unwrap();
        let grid = sample(
            Request {
                root: directory.path().into(),
                source_dir: directory.path().into(),
                name: "sample.typ".into(),
                source: source.clone(),
                range: start..source.len(),
                call: "table(..rows)".into(),
                program: program.into(),
                engine: Engine::Typst {
                    command: CommandCustomization::for_tool(crate::toolchain::ToolKind::Typst),
                    fonts: vec![],
                    kind: "table".into(),
                },
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!((grid.columns, grid.rows), (2, 1));
        assert_eq!(grid.value((0, 0)), Some("Revenue"));
        assert_eq!(grid.value((0, 1)), Some("60"));
        assert_eq!(
            fs::read_to_string(directory.path().join("sample.typ")).unwrap(),
            source
        );
        assert!(
            fs::read_dir(directory.path().join(".tiptoptyp"))
                .unwrap()
                .next()
                .is_none()
        );
    }
    #[test]
    #[ignore = "requires TIPTOPTYP_TEST_TECTONIC executable and TeX bundle"]
    fn table_samples_compile_and_resolve_tex_package_cells() {
        let program =
            std::env::var_os("TIPTOPTYP_TEST_TECTONIC").expect("set TIPTOPTYP_TEST_TECTONIC");
        let directory = tempfile::tempdir().unwrap();
        let source = include_str!("../tests/fixtures/tex-table-packages.tex");
        fs::write(directory.path().join("packages.tex"), source).unwrap();
        for label in [
            "Star",
            "Tabulary",
            "Xltabular",
            "Longtable",
            "Nice",
            "NiceStar",
            "Tabularray",
            "Longray",
            "Tallray",
            "Siunitx",
            "Dcolumn",
            "Array",
        ] {
            let cursor = source.find(&format!("{label} &")).unwrap();
            let mut table = crate::editor_features::tex_table_at(source, cursor).expect(label);
            let rows = table.cells.as_mut().expect(label);
            let r = rows.iter().position(|row| row[0] == label).expect(label);
            rows[r][0] = format!("Edited{label}");
            assert!(table.grid_source_edit().is_ok(), "{label}");
            let cells = BTreeMap::from([((r, 1), r"\revenue".into())]);
            let call = tex_instrument(&table, &cells).unwrap();
            let grid = sample(
                Request {
                    root: directory.path().into(),
                    source_dir: directory.path().into(),
                    name: "packages.tex".into(),
                    source: source.into(),
                    range: table.source_range.clone(),
                    call,
                    program: program.clone().into(),
                    engine: Engine::Tex {
                        command: CommandCustomization::for_tool(
                            crate::toolchain::ToolKind::Tectonic,
                        ),
                        standard: false,
                        only_cached: false,
                        cells,
                        columns: table.columns().unwrap(),
                        rows: table.cells.as_ref().unwrap().len(),
                    },
                },
                &AtomicBool::new(false),
            )
            .unwrap_or_else(|error| panic!("{label}: {error}"));
            assert_eq!(grid.value((r, 1)), Some("60"), "{label}");
        }
        assert_eq!(
            fs::read_to_string(directory.path().join("packages.tex")).unwrap(),
            source
        );
        assert!(
            fs::read_dir(directory.path().join(".tiptoptyp"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    #[ignore = "requires TIPTOPTYP_TEST_TECTONIC executable and TeX bundle"]
    fn table_samples_compile_merged_tex_macro_cells_without_changing_the_document() {
        let program =
            std::env::var_os("TIPTOPTYP_TEST_TECTONIC").expect("set TIPTOPTYP_TEST_TECTONIC");
        let minimal = r"\documentclass{article}
\usepackage{multirow,makecell,pifont}
\newcommand{\revenue}{60}
\newcommand{\xmark}{\ding{55}}
\begin{document}
\begin{tabular}{lll}
\multicolumn{2}{c}{\revenue} & \xmark\\
\multirow{2}{*}{\textbf{\revenue}} & \xmark$\ast$ & B\\
 & C & D\\
\end{tabular}
\end{document}";
        for (name, source, values) in [
            (
                "merged",
                minimal,
                vec![
                    ((0, 0), "60"),
                    ((1, 0), "60"),
                    ((0, 2), "✗"),
                    ((1, 1), "✗∗"),
                ],
            ),
            (
                "platonic",
                include_str!("../tests/fixtures/arxiv-tables/platonic-metrics.tex"),
                vec![((2, 1), "✓"), ((6, 2), "✓∗")],
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let filename = format!("{name}.tex");
            fs::write(directory.path().join(&filename), source).unwrap();
            assert!(
                source.is_ascii(),
                "this fixture uses character and byte offsets interchangeably"
            );
            let table = crate::editor_features::tex_table_at(
                source,
                source.find("\\begin{tabular}").unwrap(),
            )
            .unwrap();
            let rows = table.cells.as_ref().unwrap();
            let locked: BTreeMap<_, _> = values
                .iter()
                .map(|&((r, c), _)| ((r, c), rows[r][c].clone()))
                .collect();
            let call = tex_instrument(&table, &locked).unwrap();
            let grid = sample(
                Request {
                    root: directory.path().into(),
                    source_dir: directory.path().into(),
                    name: filename.clone(),
                    source: source.into(),
                    range: table.source_range.clone(),
                    call,
                    program: program.clone().into(),
                    engine: Engine::Tex {
                        command: CommandCustomization::for_tool(
                            crate::toolchain::ToolKind::Tectonic,
                        ),
                        standard: false,
                        only_cached: false,
                        cells: locked,
                        columns: table.columns().unwrap(),
                        rows: rows.len(),
                    },
                },
                &AtomicBool::new(false),
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            for (position, expected) in values {
                assert_eq!(grid.value(position), Some(expected), "{name}: {position:?}");
            }
            assert_eq!(
                fs::read_to_string(directory.path().join(&filename)).unwrap(),
                source
            );
            assert!(
                fs::read_dir(directory.path().join(".tiptoptyp"))
                    .unwrap()
                    .next()
                    .is_none()
            );
        }
    }

    #[test]
    #[ignore = "requires TIPTOPTYP_TEST_TECTONIC executable and TeX bundle"]
    fn table_samples_resolve_actual_tex_macro_and_cleanup() {
        let program =
            std::env::var_os("TIPTOPTYP_TEST_TECTONIC").expect("set TIPTOPTYP_TEST_TECTONIC");
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("data.tex"),
            r"\newcommand{\revenue}{60}",
        )
        .unwrap();
        let source = r"\documentclass{article}
\input{data}
\begin{document}
\begin{tabular}{ll}Revenue & \revenue\end{tabular}
\end{document}";
        let table = crate::editor_features::tex_table_at(source, source.find("Revenue &").unwrap())
            .unwrap();
        let cells = BTreeMap::from([((0, 1), r"\revenue".into())]);
        let call = tex_instrument(&table, &cells).unwrap();
        let range = table.source_range.clone();
        fs::write(directory.path().join("sample.tex"), source).unwrap();
        let grid = sample(
            Request {
                root: directory.path().into(),
                source_dir: directory.path().into(),
                name: "sample.tex".into(),
                source: source.into(),
                range,
                call,
                program: program.into(),
                engine: Engine::Tex {
                    command: CommandCustomization::for_tool(crate::toolchain::ToolKind::Tectonic),
                    standard: false,
                    only_cached: false,
                    cells,
                    columns: 2,
                    rows: 1,
                },
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(grid.value((0, 1)), Some("60"));
        assert_eq!(
            fs::read_to_string(directory.path().join("sample.tex")).unwrap(),
            source
        );
        assert!(
            fs::read_dir(directory.path().join(".tiptoptyp"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}
