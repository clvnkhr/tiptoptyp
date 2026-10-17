//! Imports for generated TeX, planned before committing one editor transaction.
//! Never evaluates TeX, removes user imports, or installs packages.
use std::{collections::BTreeSet, ops::Range};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Requirements {
    packages: BTreeSet<String>,
    libraries: BTreeSet<String>,
}
impl Requirements {
    pub(crate) fn package(&mut self, name: &str) {
        if !matches!(name, "" | "latex2e") {
            self.packages.insert(name.to_owned());
        }
    }
    fn commands(source: &str) -> Self {
        let mut result = Self::default();
        for token in tokens(source) {
            if token.name == "begin" {
                let package = match token.argument.as_deref().unwrap_or_default() {
                    "tabularx" => "tabularx",
                    "tabulary" => "tabulary",
                    "longtable" => "longtable",
                    "xltabular" => "xltabular",
                    "NiceTabular" | "NiceTabular*" | "NiceArray" | "NiceMatrix" | "pNiceMatrix"
                    | "bNiceMatrix" => "nicematrix",
                    "tblr" | "longtblr" | "talltblr" => "tabularray",
                    "align" | "align*" | "aligned" | "alignat" | "alignat*" | "gather"
                    | "gather*" | "gathered" | "multline" | "multline*" | "split" | "equation*"
                    | "matrix" | "pmatrix" | "bmatrix" | "Bmatrix" | "vmatrix" | "Vmatrix"
                    | "smallmatrix" | "cases" => "amsmath",
                    _ => "",
                };
                result.package(package);
            }
            let package = match token.name.as_str() {
                "toprule" | "midrule" | "bottomrule" | "cmidrule" | "addlinespace"
                | "specialrule" => "booktabs",
                "rowcolor" | "columncolor" | "cellcolor" => "colortbl",
                "rowcolors" | "textcolor" | "colorbox" | "fcolorbox" | "definecolor" => "xcolor",
                "hdashline" | "cdashline" => "arydshln",
                "multirow" => "multirow",
                "includegraphics" | "resizebox" | "rotatebox" | "scalebox" => "graphicx",
                "href" | "hyperref" | "autoref" => "hyperref",
                "url" | "nolinkurl" => "url",
                "text"
                | "dfrac"
                | "tfrac"
                | "operatorname"
                | "DeclareMathOperator"
                | "overset"
                | "underset"
                | "lvert"
                | "rvert"
                | "lVert"
                | "rVert" => "amsmath",
                "mathbb" | "mathfrak" => "amssymb",
                "mathscr" => "mathrsfs",
                "mathds" => "dsfont",
                "SI" | "si" | "num" | "qty" | "unit" | "numlist" | "numrange" | "sisetup" => {
                    "siunitx"
                }
                "newcolumntype" | "arraybackslash" | "extrarowheight" => "array",
                _ => symbol_package(&format!("\\{}", token.name)).unwrap_or(""),
            };
            result.package(package);
            if token.name == "UseTblrLibrary" {
                result.package("tabularray");
            }
            if token.name == "rowcolors" {
                result.package("colortbl");
            }
        }
        result
    }
    pub(crate) fn fragment(source: &str) -> Self {
        let mut result = Self::commands(source);
        // Parse each complete table once, on its own slice, so templates and
        // snippets get the same column requirements as the table workbench.
        let mut stack = Vec::new();
        for token in tokens(source) {
            if token.name == "begin" {
                stack.push((token.argument.clone(), token.start));
            } else if token.name == "end"
                && let Some((environment, start)) = stack.pop()
                && environment == token.argument
            {
                let slice = &source[start..token.end];
                if let Some(table) = crate::editor_features::tex_table_at(slice, 1) {
                    let required = Self::table(&table, slice);
                    result.packages.extend(required.packages);
                    result.libraries.extend(required.libraries);
                }
            }
        }
        result
    }
    pub(crate) fn symbol(text: &str, package: Option<&str>) -> Self {
        if [
            "\\mathcal{",
            "\\mathbf{",
            "\\mathrm{",
            "\\mathit{",
            "\\mathsf{",
            "\\mathtt{",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
        {
            return Self::default();
        }
        if let Some(package) = package {
            let mut result = Self::default();
            result.package(package);
            return result;
        }
        let mut result = Self::fragment(text);
        if let Some(package) = symbol_package(text) {
            result.package(package);
        }
        result
    }
    pub(crate) fn table(table: &crate::editor_features::TexTable, source: &str) -> Self {
        let mut result = Self::commands(source);
        let spec = &table.column_spec;
        // Ignore widths/options/modifier bodies: letters in a dimension or a
        // command are not column types. Repetitions contain another column spec.
        fn columns(spec: &str, result: &mut Requirements, depth: usize) {
            if depth > 32 {
                return;
            }
            let mut i = 0;
            while i < spec.len() {
                let c = spec.as_bytes()[i];
                if c == b'%' {
                    i = comment_end(spec, i);
                    continue;
                }
                if c == b'\\' {
                    i = command_end(spec, i);
                    continue;
                }
                if matches!(c, b'{' | b'[') {
                    if let Some((_, end)) = group(spec, i, c) {
                        i = end;
                        continue;
                    }
                    break;
                }
                let package = match c {
                    b'm' | b'b' | b'w' | b'W' | b'>' | b'<' | b'!' => "array",
                    b'S' => "siunitx",
                    b'D' => "dcolumn",
                    b':' => "arydshln",
                    _ => "",
                };
                result.package(package);
                i += 1;
                if c == b'*'
                    && let Some((_, end)) = group(spec, i, b'{')
                    && let Some((inner, end)) = group(spec, end, b'{')
                {
                    columns(&inner, result, depth + 1);
                    i = end;
                }
            }
        }
        columns(spec, &mut result, 0);
        if result.packages.contains("tabularray") {
            for library in ["booktabs", "siunitx", "amsmath"] {
                if result.packages.contains(library) {
                    result.libraries.insert(library.into());
                }
            }
        }
        result
    }
}

// Shared by both handwriting engines, snippets and generated source. Core
// commands win when multiple providers share a command name.
fn symbol_package(text: &str) -> Option<&'static str> {
    static PACKAGES: std::sync::OnceLock<std::collections::BTreeMap<String, Option<String>>> =
        std::sync::OnceLock::new();
    PACKAGES
        .get_or_init(|| {
            let symbols: Vec<crate::handwriting::Symbol> = serde_json::from_str(include_str!(
                "../../assets/handwriting/detexify-symbols.json"
            ))
            .expect("symbol package catalogue");
            let mut packages = std::collections::BTreeMap::new();
            for symbol in symbols {
                if let Some(tex) = symbol.tex {
                    let entry = packages.entry(tex).or_insert(symbol.package.clone());
                    if symbol.package.is_none() {
                        *entry = None;
                    }
                }
            }
            packages
        })
        .get(text)
        .and_then(|package| package.as_deref())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PreparedEdit {
    pub(crate) byte_range: Range<usize>,
    pub(crate) replacement: String,
    pub(crate) cursor: usize,
    pub(crate) added: Vec<String>,
}
/// Cursor uses Unicode scalar offsets in the source *after* the main edit.
pub(crate) fn prepare_edit(
    source: &str,
    range: Range<usize>,
    replacement: &str,
    cursor: usize,
    required: &Requirements,
) -> Result<PreparedEdit, String> {
    if source.get(range.clone()).is_none() {
        return Err("The generated edit range is no longer valid".into());
    }
    let mut candidate = source.to_owned();
    candidate.replace_range(range.clone(), replacement);
    let Some(import) = plan(&candidate, required)? else {
        return Ok(PreparedEdit {
            byte_range: range,
            replacement: replacement.into(),
            cursor,
            added: vec![],
        });
    };
    let at = candidate[..import.at].chars().count();
    let cursor = cursor
        + if at <= cursor {
            import.text.chars().count()
        } else {
            0
        };
    candidate.insert_str(import.at, &import.text);
    Ok(PreparedEdit {
        byte_range: 0..source.len(),
        replacement: candidate,
        cursor,
        added: import.added,
    })
}
/// Remap selections and snippet tabstops together with the import insertion.
pub(crate) fn ensure(
    source: &mut String,
    offsets: &mut [usize],
    required: &Requirements,
) -> Result<Vec<String>, String> {
    let Some(import) = plan(source, required)? else {
        return Ok(vec![]);
    };
    let at = source[..import.at].chars().count();
    let added = import.text.chars().count();
    for offset in offsets {
        if *offset >= at {
            *offset += added;
        }
    }
    source.insert_str(import.at, &import.text);
    Ok(import.added)
}
struct Import {
    at: usize,
    text: String,
    added: Vec<String>,
}
fn plan(source: &str, required: &Requirements) -> Result<Option<Import>, String> {
    if required.packages.is_empty() && required.libraries.is_empty() {
        return Ok(None);
    }
    let mut packages = BTreeSet::new();
    let mut libraries = BTreeSet::new();
    let mut insertion = None;
    let mut late = Vec::new();
    let mut configuration = Vec::new();
    for token in tokens(source) {
        if token.depth != 0 {
            continue;
        }
        if token.name == "begin" && token.argument.as_deref() == Some("document") {
            // Empty-option imports follow user/imported preambles, avoiding
            // option clashes with packages loaded inside an \input file.
            insertion = Some(token.start);
            break;
        }
        let configured = match token.name.as_str() {
            "newcolumntype" => "array",
            "DeclareMathOperator" => "amsmath",
            "definecolor" => "xcolor",
            "sisetup" => "siunitx",
            "UseTblrLibrary" => "tabularray",
            _ => "",
        };
        if required.packages.contains(configured) {
            configuration.push(token.start);
        }
        match token.name.as_str() {
            "documentclass" | "LoadClass" => {
                insertion = Some(after_command(source, token.end));
            }
            "usepackage" | "RequirePackage" | "UseTblrLibrary" => {
                let names = token
                    .argument
                    .as_deref()
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                if token.name == "UseTblrLibrary" {
                    libraries.extend(names.map(str::to_owned));
                } else {
                    let names: Vec<_> = names.collect();
                    let table_color = names.contains(&"xcolor");
                    for name in &names {
                        if matches!(*name, "arydshln" | "hyperref" | "cleveref") {
                            late.push(((*name).to_owned(), token.start));
                        }
                    }
                    packages.extend(names.into_iter().map(str::to_owned));
                    if token
                        .options
                        .as_deref()
                        .is_some_and(|opts| opts.split(',').any(|s| s.trim() == "table"))
                        && table_color
                    {
                        packages.insert("colortbl".into());
                    }
                }
                insertion = Some(after_command(source, token.end));
            }
            _ => {}
        }
    }
    // Known providers load these dependencies themselves. Never change an
    // existing option list (in particular xcolor): that causes option clashes.
    if [
        "tabularx",
        "tabulary",
        "nicematrix",
        "dcolumn",
        "colortbl",
        "xltabular",
    ]
    .iter()
    .any(|p| packages.contains(*p) || required.packages.contains(*p))
    {
        packages.insert("array".into());
    }
    if packages.contains("xltabular") {
        packages.extend(["longtable".into(), "tabularx".into()]);
    }
    if packages.contains("mathtools") {
        packages.insert("amsmath".into());
    }
    if packages.contains("hyperref") {
        packages.insert("url".into());
    }
    for library in ["booktabs", "siunitx", "amsmath"] {
        if libraries.contains(library) {
            packages.insert(library.into());
        }
    }
    let mut missing: Vec<_> = required.packages.difference(&packages).cloned().collect();
    // arydshln must follow array/longtable/colortbl.
    missing.sort_by_key(|name| (name == "arydshln", name.clone()));
    let missing_libraries: Vec<_> = required.libraries.difference(&libraries).cloned().collect();
    if missing.is_empty() && missing_libraries.is_empty() {
        return Ok(None);
    }
    let mut needed = missing.clone();
    needed.extend(
        missing_libraries
            .iter()
            .map(|name| format!("tabularray/{name}")),
    );
    let mut at = insertion.ok_or_else(|| format!("This TeX fragment has no preamble. Add {} to the main document's preamble before applying this edit.", needed.join(", ")))?;
    for start in configuration {
        at = at.min(start);
    }
    for (package, start) in late {
        if package != "arydshln"
            || missing.iter().any(|name| {
                matches!(
                    name.as_str(),
                    "array"
                        | "longtable"
                        | "colortbl"
                        | "tabularx"
                        | "tabulary"
                        | "xltabular"
                        | "nicematrix"
                        | "dcolumn"
                        | "siunitx"
                )
            })
        {
            at = at.min(start);
        }
    }
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = String::new();
    if at > 0 && !source[..at].ends_with('\n') {
        text.push_str(newline);
    }
    for package in &missing {
        text.push_str(&format!("\\usepackage{{{package}}}{newline}"));
    }
    if !missing_libraries.is_empty() {
        text.push_str(&format!(
            "\\UseTblrLibrary{{{}}}{newline}",
            missing_libraries.join(",")
        ));
    }
    let mut added = missing;
    added.extend(
        missing_libraries
            .into_iter()
            .map(|name| format!("tabularray/{name}")),
    );
    Ok(Some(Import { at, text, added }))
}
fn after_command(source: &str, end: usize) -> usize {
    let tail = &source[end..];
    let line_end = tail.find('\n').map_or(source.len(), |n| end + n + 1);
    let rest = source[end..line_end].trim_start();
    if rest.is_empty() || rest.starts_with('%') {
        line_end
    } else {
        end
    }
}
struct Token {
    name: String,
    start: usize,
    end: usize,
    depth: usize,
    argument: Option<String>,
    options: Option<String>,
}
/// Small lexical scanner, deliberately not a TeX expansion engine. Literal
/// examples/comments and imports inside macro bodies cannot count as imports.
fn tokens(source: &str) -> Vec<Token> {
    let mut result = Vec::new();
    let mut i = 0;
    let mut depth = 0usize;
    while i < source.len() {
        match source.as_bytes()[i] {
            b'%' => {
                i = comment_end(source, i);
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'\\' => {
                let start = i;
                let end = command_end(source, i);
                let name = &source[start + 1..end];
                i = end;
                if name == "verb" {
                    if source.as_bytes().get(i) == Some(&b'*') {
                        i += 1;
                    }
                    if let Some(delimiter) = source[i..].chars().next() {
                        i += delimiter.len_utf8();
                        i = source[i..]
                            .find(delimiter)
                            .map_or(source.len(), |n| i + n + delimiter.len_utf8());
                    }
                    continue;
                }
                if name == "string" {
                    i = if source.as_bytes().get(i) == Some(&b'\\') {
                        command_end(source, i)
                    } else {
                        next_char(source, i)
                    };
                    continue;
                }
                if name == "detokenize"
                    && let Some((_, end)) = group(source, i, b'{')
                {
                    i = end;
                    continue;
                }
                let mut options = None;
                let mut argument = None;
                if matches!(
                    name,
                    "begin"
                        | "end"
                        | "usepackage"
                        | "RequirePackage"
                        | "UseTblrLibrary"
                        | "documentclass"
                        | "LoadClass"
                ) {
                    if let Some((value, end)) = group(source, i, b'[') {
                        options = Some(value);
                        i = end;
                    }
                    if let Some((value, end)) = group(source, i, b'{') {
                        argument = Some(value.trim().into());
                        i = end;
                    }
                    // Optional release-date argument follows the package name.
                    if matches!(name, "usepackage" | "RequirePackage")
                        && let Some((_, end)) = group(source, i, b'[')
                    {
                        i = end;
                    }
                }
                if name == "begin"
                    && argument.as_deref().is_some_and(|env| {
                        matches!(
                            env,
                            "verbatim"
                                | "verbatim*"
                                | "Verbatim"
                                | "lstlisting"
                                | "minted"
                                | "comment"
                        )
                    })
                {
                    let closing = format!("\\end{{{}}}", argument.as_deref().unwrap());
                    i = source[i..]
                        .find(&closing)
                        .map_or(source.len(), |n| i + n + closing.len());
                    continue;
                }
                result.push(Token {
                    name: name.into(),
                    start,
                    end: i,
                    depth,
                    argument,
                    options,
                });
            }
            _ => i = next_char(source, i),
        }
    }
    result
}
fn command_end(source: &str, start: usize) -> usize {
    let mut end = (start + 1).min(source.len());
    while source
        .as_bytes()
        .get(end)
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'@')
    {
        end += 1;
    }
    if end == start + 1 {
        end = next_char(source, end);
    }
    end
}
fn next_char(source: &str, at: usize) -> usize {
    at + source
        .get(at..)
        .and_then(|s| s.chars().next())
        .map_or(0, char::len_utf8)
}
fn comment_end(source: &str, at: usize) -> usize {
    source[at..].find('\n').map_or(source.len(), |n| at + n + 1)
}
fn group(source: &str, mut at: usize, open: u8) -> Option<(String, usize)> {
    loop {
        while source
            .as_bytes()
            .get(at)
            .is_some_and(u8::is_ascii_whitespace)
        {
            at += 1;
        }
        if source.as_bytes().get(at) != Some(&b'%') {
            break;
        }
        at = comment_end(source, at);
    }
    if source.as_bytes().get(at) != Some(&open) {
        return None;
    }
    let close = if open == b'{' { b'}' } else { b']' };
    let mut level = 1;
    let mut value = String::new();
    at += 1;
    while at < source.len() {
        let c = source.as_bytes()[at];
        if c == b'%' {
            at = comment_end(source, at);
            continue;
        }
        if c == b'\\' {
            let end = command_end(source, at);
            value.push_str(&source[at..end]);
            at = end;
            continue;
        }
        if c == open {
            level += 1;
        } else if c == close {
            level -= 1;
            if level == 0 {
                return Some((value, at + 1));
            }
        }
        // Braced options may contain closing square brackets.
        if open == b'[' && c == b'{' {
            let (inner, end) = group(source, at, b'{')?;
            value.push('{');
            value.push_str(&inner);
            value.push('}');
            at = end;
            continue;
        }
        let end = next_char(source, at);
        value.push_str(&source[at..end]);
        at = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document(body: &str) -> String {
        format!("\\documentclass{{article}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n")
    }
    fn insert(source: &str, text: &str, required: Requirements) -> PreparedEdit {
        let at = source.find("\\end{document}").unwrap();
        prepare_edit(
            source,
            at..at,
            text,
            source[..at].chars().count() + text.chars().count(),
            &required,
        )
        .unwrap()
    }
    #[test]
    fn tex_imports_are_idempotent_preserve_options_comments_unicode_and_cursor() {
        let source = "\\documentclass{article}\r\n\\usepackage[dvipsnames]{xcolor} % keep this\r\n\\begin{document}\r\nPréface α\r\n\\end{document}\r\n";
        let text = "\\rowcolor{red} α";
        let edit = insert(source, text, Requirements::fragment(text));
        assert_eq!(edit.added, ["colortbl"]);
        assert!(edit.replacement.contains(
            "\\usepackage[dvipsnames]{xcolor} % keep this\r\n\\usepackage{colortbl}\r\n"
        ));
        assert_eq!(edit.replacement.chars().nth(edit.cursor), Some('\\'));
        assert!(
            edit.replacement[..edit.replacement.char_indices().nth(edit.cursor).unwrap().0]
                .ends_with(text)
        );
        assert!(!edit.replacement.replace("\r\n", "").contains('\n'));
        assert!(
            plan(&edit.replacement, &Requirements::fragment(text))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn tex_imports_ignore_comments_verbatim_macro_bodies_and_body_imports() {
        let source = document(
            "% \\usepackage{booktabs}\n\\verb|\\usepackage{booktabs}|\n\\begin{verbatim}\n\\usepackage{booktabs}\n\\end{verbatim}\n\\usepackage{booktabs}",
        );
        let source = source.replace(
            "\\begin{document}",
            "\\newcommand{\\fake}{\\usepackage{booktabs}}\n\\begin{document}",
        );
        let edit = insert(&source, "\\toprule", Requirements::fragment("\\toprule"));
        assert_eq!(edit.added, ["booktabs"]);
        assert_eq!(
            Requirements::fragment(
                "% \\toprule\n\\verb*|\\toprule| \\begin{minted}{tex}\\toprule\\end{minted} \\string\\toprule \\detokenize{\\toprule}"
            ),
            Requirements::default()
        );
    }
    #[test]
    fn tex_imports_recognize_multiline_lists_release_dates_and_table_option() {
        let source = "\\documentclass{article}\n\\RequirePackage[table,% remark\n dvipsnames]{xcolor, % remark\n booktabs}[2020/01/01]\n\\begin{document}\n\\end{document}\n";
        assert!(
            plan(source, &Requirements::fragment("\\toprule \\rowcolor{red}"))
                .unwrap()
                .is_none()
        );
        // An option on a different import must not imply xcolor/table.
        let source = source.replace(
            "[table,% remark\n dvipsnames]{xcolor, % remark\n booktabs}",
            "{xcolor}\n\\usepackage[table]{booktabs}",
        );
        assert_eq!(
            insert(
                &source,
                "\\rowcolor{red}",
                Requirements::fragment("\\rowcolor{red}")
            )
            .added,
            ["colortbl"]
        );
    }
    #[test]
    fn tex_imports_place_packages_after_class_and_before_body_even_on_same_line() {
        let source = "\\documentclass{article}\\begin{document}α\\end{document}";
        let edit = insert(
            source,
            "\\includegraphics{plot}",
            Requirements::fragment("\\includegraphics{plot}"),
        );
        assert!(
            edit.replacement
                .starts_with("\\documentclass{article}\n\\usepackage{graphicx}\n\\begin{document}")
        );
        let mut source = "\\begin{document}\\text{ok}\\end{document}".to_owned();
        ensure(&mut source, &mut [], &Requirements::fragment("\\text{ok}")).unwrap();
        assert!(source.starts_with("\\usepackage{amsmath}\n\\begin"));
    }
    #[test]
    fn tex_imports_leave_fragments_unchanged_when_no_safe_preamble_exists() {
        let source = "α body";
        let error = prepare_edit(
            source,
            3..3,
            "\\toprule",
            10,
            &Requirements::fragment("\\toprule"),
        )
        .unwrap_err();
        assert!(error.contains("main document's preamble"));
        let edit = prepare_edit(source, 3..3, "\\alpha", 9, &Requirements::default()).unwrap();
        assert!(edit.added.is_empty());
    }
    #[test]
    fn tex_imports_only_add_required_environment_and_column_dependencies() {
        for (environment, expected) in [
            ("tabular", ""),
            ("tabular*", ""),
            ("tabularx", "tabularx"),
            ("tabulary", "tabulary"),
            ("longtable", "longtable"),
            ("xltabular", "xltabular"),
            ("NiceTabular", "nicematrix"),
            ("NiceTabular*", "nicematrix"),
            ("tblr", "tabularray"),
            ("longtblr", "tabularray"),
            ("talltblr", "tabularray"),
        ] {
            let width = if matches!(
                environment,
                "tabular*" | "tabularx" | "tabulary" | "xltabular" | "NiceTabular*"
            ) {
                "{\\linewidth}"
            } else {
                ""
            };
            let spec = if matches!(environment, "tblr" | "longtblr" | "talltblr") {
                "{colspec={ll}}"
            } else {
                "{ll}"
            };
            let body = format!("\\begin{{{environment}}}{width}{spec}A & B\\end{{{environment}}}");
            let table = crate::editor_features::tex_table_at(&body, 1).unwrap();
            let edit = insert(&document(""), &body, Requirements::table(&table, &body));
            assert_eq!(
                edit.added,
                if expected.is_empty() {
                    vec![]
                } else {
                    vec![expected.to_owned()]
                },
                "{environment}"
            );
        }
        for (spec, expected) in [
            ("p{2cm}l", vec![]),
            ("m{2cm}l", vec!["array"]),
            ("*{2}{S[table-format=2.1]}", vec!["siunitx"]),
            ("D{.}{.}{2}l", vec!["dcolumn"]),
            (">{\\textbf{S}}l", vec!["array"]),
            ("l:r", vec!["arydshln"]),
        ] {
            let body = format!("\\begin{{tabular}}{{{spec}}}A & B\\end{{tabular}}");
            let table = crate::editor_features::tex_table_at(&body, 1).unwrap();
            assert_eq!(
                insert(&document(""), &body, Requirements::table(&table, &body)).added,
                expected,
                "{spec}"
            );
        }
    }
    #[test]
    fn tex_imports_add_tabularray_libraries_once_and_keep_existing_libraries() {
        let body = "\\begin{tblr}{colspec={lS[table-format=2.0]}}\\toprule A & 60\\\\\\bottomrule\\end{tblr}";
        let table = crate::editor_features::tex_table_at(body, 1).unwrap();
        let required = Requirements::table(&table, body);
        let source = document("").replace(
            "\\begin{document}",
            "\\usepackage{tabularray,siunitx}\n\\UseTblrLibrary{siunitx}\n\\begin{document}",
        );
        let edit = insert(&source, body, required.clone());
        assert_eq!(edit.added, ["booktabs", "tabularray/booktabs"]);
        assert!(edit.replacement.contains("\\UseTblrLibrary{booktabs}\n"));
        assert!(plan(&edit.replacement, &required).unwrap().is_none());
    }
    #[test]
    fn tex_imports_order_dashed_rules_after_their_dependencies() {
        let body =
            "\\begin{longtable}{m{2cm}:r}\\rowcolor{red}A & B\\\\\\hdashline\\end{longtable}";
        let table = crate::editor_features::tex_table_at(body, 1).unwrap();
        let edit = insert(&document(""), body, Requirements::table(&table, body));
        assert_eq!(edit.added, ["colortbl", "longtable", "arydshln"]);
        assert!(
            edit.replacement.find("\\usepackage{arydshln}").unwrap()
                > edit.replacement.find("\\usepackage{longtable}").unwrap()
        );
    }
    #[test]
    fn tex_imports_use_symbol_provider_metadata_and_core_commands_need_nothing() {
        assert_eq!(
            Requirements::symbol("\\Box", Some("wasysym")).packages,
            BTreeSet::from(["wasysym".into()])
        );
        assert_eq!(
            Requirements::symbol("\\upalpha", None).packages,
            BTreeSet::from(["upgreek".into()])
        );
        assert_eq!(
            Requirements::symbol("\\mathbb{A}", None).packages,
            BTreeSet::from(["amssymb".into()])
        );
        for text in ["\\alpha", "\\mathcal{A}", "\\$", "\\%"] {
            assert!(
                Requirements::symbol(text, None).packages.is_empty(),
                "{text}"
            );
        }
        assert!(
            Requirements::symbol("\\textbackslash", Some("latex2e"))
                .packages
                .is_empty()
        );
    }
    #[test]
    fn tex_imports_remap_all_snippet_offsets_without_changing_fields() {
        let mut source = document("\\text{α}");
        let start = source[..source.find('α').unwrap()].chars().count();
        let mut offsets = [start, start + 1, start];
        let original = source.clone();
        ensure(
            &mut source,
            &mut offsets,
            &Requirements::fragment("\\text{α}"),
        )
        .unwrap();
        let added = source.chars().count() - original.chars().count();
        assert_eq!(offsets, [start + added, start + 1 + added, start + added]);
        assert_eq!(source.chars().nth(offsets[0]), Some('α'));
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::*;
    fn insert(preamble: &str, text: &str) -> PreparedEdit {
        let source = format!(
            "\\documentclass{{article}}\n{preamble}\\begin{{document}}\n\\end{{document}}\n"
        );
        let at = source.find("\\end{document}").unwrap();
        prepare_edit(
            &source,
            at..at,
            text,
            source[..at].chars().count() + text.chars().count(),
            &Requirements::fragment(text),
        )
        .unwrap()
    }
    #[test]
    fn tex_imports_keep_imported_preamble_options_and_late_package_order() {
        let edit = insert("\\input{preamble}\n", "\\textcolor{red}{α}");
        assert!(
            edit.replacement.find("\\usepackage{xcolor}").unwrap()
                > edit.replacement.find("\\input{preamble}").unwrap()
        );
        let edit = insert(
            "\\usepackage{arydshln}\n\\usepackage{hyperref}\n",
            "\\rowcolor{red}",
        );
        assert!(
            edit.replacement.find("\\usepackage{colortbl}").unwrap()
                < edit.replacement.find("\\usepackage{arydshln}").unwrap()
        );
    }
    #[test]
    fn tex_imports_snippets_share_known_symbol_packages() {
        assert_eq!(
            Requirements::fragment("$\\leqslant$ \\verb|\\upalpha|").packages,
            BTreeSet::from(["amssymb".into()])
        );
    }
}

#[cfg(test)]
mod preamble_tests {
    use super::*;
    #[test]
    fn tex_imports_templates_load_packages_before_preamble_configuration() {
        for (setup, package) in [
            ("\\newcolumntype{L}{l}", "array"),
            ("\\DeclareMathOperator{\\diag}{diag}", "amsmath"),
            ("\\definecolor{accent}{rgb}{1,0,0}", "xcolor"),
            ("\\sisetup{detect-all}", "siunitx"),
        ] {
            let mut source = format!(
                "\\documentclass{{article}}\n{setup}\n\\begin{{document}}\n\\end{{document}}\n"
            );
            let requirements = Requirements::fragment(&source);
            ensure(&mut source, &mut [], &requirements).unwrap();
            assert!(
                source.find(&format!("\\usepackage{{{package}}}")).unwrap()
                    < source.find(setup).unwrap(),
                "{setup}"
            );
        }
    }
}

#[cfg(test)]
mod preset_tests {
    use super::*;
    use crate::snippets::{Language, Scope};

    fn fixture(snippet: &crate::snippets::Snippet) -> (String, Requirements) {
        let body = crate::completion_edit::expand_lsp_snippet(&snippet.source_to_lsp().unwrap())
            .unwrap()
            .text;
        let required = Requirements::fragment(&body);
        let body = if snippet.scope == Scope::Math {
            format!("${body}$")
        } else {
            body
        };
        let mut source =
            format!("\\documentclass{{article}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n");
        ensure(&mut source, &mut [], &required).unwrap();
        (source, required)
    }

    #[test]
    fn tex_imports_all_tex_presets_have_minimal_idempotent_dependencies() {
        for snippet in crate::snippet_presets::defaults()
            .iter()
            .filter(|s| s.language == Language::Tex)
        {
            let (source, required) = fixture(snippet);
            let expected = match snippet.prefix.as_str() {
                "norm" | "align" | "pmat" | "cases" => BTreeSet::from(["amsmath".into()]),
                "bb" => BTreeSet::from(["amssymb".into()]),
                _ => BTreeSet::new(),
            };
            assert_eq!(required.packages, expected, "{}", snippet.prefix);
            assert!(
                plan(&source, &required).unwrap().is_none(),
                "{}",
                snippet.prefix
            );
        }
    }

    #[test]
    fn tex_imports_norm_preset_fields_and_import_share_one_undo() {
        use crate::completion_edit::{CompletionCoordinates, CompletionTransaction};
        let snippet = crate::snippet_presets::defaults()
            .into_iter()
            .find(|s| s.language == Language::Tex && s.prefix == "norm")
            .unwrap();
        let source =
            "\\documentclass{article}\n\\begin{document}\nPréface $\\norm$\n\\end{document}\n";
        let cursor = source[..source.find("norm").unwrap() + 4].chars().count();
        let item = crate::snippets::items(
            std::slice::from_ref(&snippet),
            crate::document::DocumentKind::Tex,
            source,
            cursor,
        )
        .unwrap()
        .remove(0);
        let mut document = crate::document::DocumentSession::new(
            tiptoptyp_core::document::WindowSessionId::new(1),
            source,
            crate::document::DocumentKind::Tex,
        );
        let mut transaction = CompletionTransaction::prepare(
            document.key(),
            source,
            cursor,
            &item,
            CompletionCoordinates::Display,
        )
        .unwrap();
        let required = Requirements::fragment(&fixture(&snippet).0);
        transaction.add_tex_imports(&required).unwrap();
        let fields = transaction.fields.clone();
        let before = eframe::egui::text::CCursorRange::default();
        transaction.commit(&mut document, before).unwrap();
        assert!(
            document
                .source()
                .contains("\\usepackage{amsmath}\n\\begin{document}")
        );
        assert!(
            document
                .source()
                .contains("$\\left\\lVert x\\right\\rVert$")
        );
        let field = fields.iter().find(|(id, _)| *id == 1).unwrap().1.clone();
        assert_eq!(
            document
                .source()
                .chars()
                .skip(field.start)
                .take(field.len())
                .collect::<String>(),
            "x"
        );
        assert!(plan(document.source(), &required).unwrap().is_none());
        document.history_step(false, before);
        assert_eq!(document.source(), source);
        document.history_step(true, before);
        assert!(document.source().contains("\\usepackage{amsmath}"));
    }

    #[test]
    #[ignore = "manual TeX catalogue validation requires an installed pdflatex"]
    fn tex_imports_all_tex_presets_compile_with_installed_pdflatex() {
        let directory = tempfile::tempdir().unwrap();
        for snippet in crate::snippet_presets::defaults()
            .iter()
            .filter(|s| s.language == Language::Tex)
        {
            let (source, _) = fixture(snippet);
            let filename = format!("{}.tex", snippet.prefix);
            std::fs::write(directory.path().join(&filename), source).unwrap();
            let output = std::process::Command::new("pdflatex")
                .args([
                    "-interaction=nonstopmode",
                    "-halt-on-error",
                    "-no-shell-escape",
                    &filename,
                ])
                .current_dir(directory.path())
                .output()
                .expect("installed pdflatex");
            assert!(
                output.status.success(),
                "{}: {}",
                snippet.prefix,
                String::from_utf8_lossy(&output.stdout)
            );
            let pdf = directory.path().join(format!("{}.pdf", snippet.prefix));
            assert!(
                std::fs::metadata(pdf).unwrap().len() > 0,
                "{}",
                snippet.prefix
            );
            eprintln!(
                "TeX preset {}: compiled with automatically planned imports",
                snippet.prefix
            );
        }
    }
}
