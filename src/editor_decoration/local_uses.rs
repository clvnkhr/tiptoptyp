//! Local source bindings, never runtime evaluation or package IO. The caller
//! caches these byte ranges with the syntax layout instead of rescanning frames.
use std::{collections::HashMap, ops::Range};
use typst_syntax::{
    LinkedNode, Source, SyntaxKind,
    ast::{self, AstNode},
};

#[derive(Clone, Copy)]
struct Binding {
    local: bool,
    stamp: usize,
    root_stamp: usize,
}

#[derive(Default)]
struct Bindings {
    names: HashMap<String, Vec<Binding>>,
    globals: HashMap<String, Binding>,
    scopes: Vec<Vec<String>>,
    qualified: usize,
    next_stamp: usize,
}
impl Bindings {
    fn enter(&mut self) {
        self.scopes.push(Vec::new());
    }
    fn exit(&mut self) {
        if let Some(names) = self.scopes.pop() {
            for name in names.into_iter().rev() {
                let values = self.names.get_mut(&name).unwrap();
                values.pop();
                self.qualified -= usize::from(name.contains('.'));
                if values.is_empty() {
                    self.names.remove(&name);
                }
            }
        }
    }
    fn value(&mut self, name: &str, local: bool) -> Binding {
        self.next_stamp += 1;
        Binding {
            local,
            stamp: self.next_stamp,
            root_stamp: name
                .split_once('.')
                .and_then(|(root, _)| self.visible(root))
                .map_or(0, |binding| binding.stamp),
        }
    }
    fn bind(&mut self, name: &str, local: bool) {
        if self.scopes.is_empty() {
            self.enter();
        }
        let value = self.value(name, local);
        self.qualified += usize::from(name.contains('.'));
        self.names.entry(name.into()).or_default().push(value);
        self.scopes.last_mut().unwrap().push(name.into());
    }
    fn bind_global(&mut self, name: &str) {
        let value = self.value(name, true);
        if self.globals.insert(name.into(), value).is_none() {
            self.qualified += usize::from(name.contains('.'));
        }
    }
    fn visible(&self, name: &str) -> Option<&Binding> {
        let local = self.names.get(name).and_then(|values| values.last());
        match (local, self.globals.get(name)) {
            (Some(local), Some(global)) => Some(if local.stamp > global.stamp {
                local
            } else {
                global
            }),
            (local, global) => local.or(global),
        }
    }
    fn contains(&self, name: &str) -> bool {
        self.visible(name).is_some_and(|binding| {
            binding.local
                && name.split_once('.').is_none_or(|(root, _)| {
                    binding.root_stamp == self.visible(root).map_or(0, |root| root.stamp)
                })
        })
    }
}

pub(super) fn typst(
    parsed: &Source,
    source_offset: usize,
    source_length: usize,
) -> Vec<Range<usize>> {
    let mut walk = TypstUses {
        bindings: Bindings::default(),
        ranges: Vec::new(),
    };
    walk.bindings.enter();
    walk.visit(LinkedNode::new(parsed.root()));
    walk.ranges
        .into_iter()
        .filter_map(|range| {
            let start = range.start.checked_sub(source_offset)?;
            let end = range.end.checked_sub(source_offset)?;
            (end <= source_length).then_some(start..end)
        })
        .collect()
}

struct TypstUses {
    bindings: Bindings,
    ranges: Vec<Range<usize>>,
}
impl TypstUses {
    fn expr<'a>(&mut self, node: &LinkedNode<'a>, expr: ast::Expr<'a>) {
        if let Some(child) = node.find(expr.to_untyped().span()) {
            self.visit(child);
        }
    }
    fn expr_after<'a>(
        &mut self,
        node: &LinkedNode<'a>,
        expr: ast::Expr<'a>,
        delimiters: &[SyntaxKind],
    ) {
        // Typed AST accessors return safe defaults for incomplete syntax.
        // A last-expression fallback may actually be a declaration target.
        let Some(delimiter) = node
            .children()
            .find(|child| delimiters.contains(&child.kind()))
        else {
            return;
        };
        let Some(value) = node.find(expr.to_untyped().span()) else {
            return;
        };
        let range = value.range();
        if range.start >= delimiter.range().end && range.start < range.end {
            self.visit(value);
        }
    }
    fn named_value<'a>(&mut self, parent: &LinkedNode<'a>, named: ast::Named<'a>) {
        if let Some(node) = parent.find(named.to_untyped().span()) {
            self.expr_after(&node, named.expr(), &[SyntaxKind::Colon]);
        }
    }
    fn bind_pattern(&mut self, pattern: ast::Pattern<'_>) {
        for name in pattern.bindings() {
            self.bindings.bind(name.get().as_str(), true);
        }
    }
    fn children(&mut self, node: LinkedNode<'_>) {
        for child in node.children() {
            self.visit(child);
        }
    }
    fn visit(&mut self, node: LinkedNode<'_>) {
        if let Some(binding) = node.get().cast::<ast::LetBinding>() {
            let names = binding.kind().bindings();
            // Function sugar can refer to itself. Ordinary initializers still
            // refer to an earlier outer binding, not the newly declared name.
            let closure = matches!(binding.kind(), ast::LetBindingKind::Closure(_));
            if closure {
                for name in &names {
                    self.bindings.bind(name.get().as_str(), true);
                }
            }
            if let Some(init) = binding.init() {
                if closure {
                    self.expr(&node, init);
                } else {
                    self.expr_after(&node, init, &[SyntaxKind::Eq]);
                }
            }
            if !closure {
                for name in names {
                    self.bindings.bind(name.get().as_str(), true);
                }
            }
            return;
        }
        if let Some(closure) = node.get().cast::<ast::Closure>() {
            for param in closure.params().children() {
                if let ast::Param::Named(named) = param {
                    self.named_value(&node, named);
                }
            }
            self.bindings.enter();
            if let Some(name) = closure.name() {
                self.bindings.bind(name.get().as_str(), true);
            }
            for param in closure.params().children() {
                match param {
                    ast::Param::Pos(pattern) => self.bind_pattern(pattern),
                    ast::Param::Named(named) => {
                        self.bindings.bind(named.name().get().as_str(), true)
                    }
                    ast::Param::Spread(spread) => {
                        if let Some(name) = spread.sink_ident() {
                            self.bindings.bind(name.get().as_str(), true);
                        }
                    }
                }
            }
            self.expr_after(&node, closure.body(), &[SyntaxKind::Arrow, SyntaxKind::Eq]);
            self.bindings.exit();
            return;
        }
        if let Some(loop_) = node.get().cast::<ast::ForLoop>() {
            self.expr(&node, loop_.iterable());
            self.bindings.enter();
            self.bind_pattern(loop_.pattern());
            let body = loop_.body();
            if matches!(
                body.to_untyped().kind(),
                SyntaxKind::CodeBlock | SyntaxKind::ContentBlock
            ) && !std::ptr::eq(body.to_untyped(), loop_.iterable().to_untyped())
            {
                self.expr(&node, body);
            }
            self.bindings.exit();
            return;
        }
        if let Some(assignment) = node.get().cast::<ast::DestructAssignment>() {
            self.expr_after(&node, assignment.value(), &[SyntaxKind::Eq]);
            self.bind_pattern(assignment.pattern());
            return;
        }
        if let Some(binary) = node.get().cast::<ast::Binary>()
            && matches!(
                binary.op(),
                ast::BinOp::Assign
                    | ast::BinOp::AddAssign
                    | ast::BinOp::SubAssign
                    | ast::BinOp::MulAssign
                    | ast::BinOp::DivAssign
            )
            && let ast::Expr::Ident(name) = binary.lhs()
        {
            self.expr_after(
                &node,
                binary.rhs(),
                &[
                    SyntaxKind::Eq,
                    SyntaxKind::PlusEq,
                    SyntaxKind::HyphEq,
                    SyntaxKind::StarEq,
                    SyntaxKind::SlashEq,
                ],
            );
            self.bindings.bind(name.get().as_str(), true);
            return;
        }
        if let Some(rule) = node.get().cast::<ast::SetRule>() {
            if let Some(args) = node.find(rule.args().to_untyped().span()) {
                self.visit(args);
            }
            if let Some(condition) = rule.condition() {
                self.expr(&node, condition);
            }
            if let Some(name) = qualified_name(rule.target()) {
                self.bindings.bind(&name, true);
            }
            return;
        }
        if let Some(rule) = node.get().cast::<ast::ShowRule>() {
            if let Some(selector) = rule.selector() {
                // .where(...) filters a customizable function; its argument
                // values are uses, while the function itself is the rule target.
                let target = if let ast::Expr::FuncCall(call) = selector {
                    if let Some(args) = node.find(call.args().to_untyped().span()) {
                        self.visit(args);
                    }
                    if let ast::Expr::FieldAccess(field) = call.callee()
                        && field.field().get() == "where"
                    {
                        field.target()
                    } else {
                        call.callee()
                    }
                } else {
                    selector
                };
                if let Some(name) = qualified_name(target) {
                    self.bindings.bind(&name, true);
                } else {
                    self.expr(&node, selector);
                }
            }
            self.expr_after(&node, rule.transform(), &[SyntaxKind::Colon]);
            return;
        }
        if let Some(import) = node.get().cast::<ast::ModuleImport>() {
            let source = import.source();
            if node.find(source.to_untyped().span()).is_some_and(|value| {
                node.children()
                    .find(|child| matches!(child.kind(), SyntaxKind::As | SyntaxKind::Colon))
                    .is_none_or(|metadata| value.range().end <= metadata.range().start)
            }) {
                self.expr(&node, source);
            }
            let renamed = import.new_name();
            if let Some(name) = renamed {
                self.bindings.bind(name.get().as_str(), false);
            }
            match import.imports() {
                Some(ast::Imports::Items(items)) => {
                    for item in items.iter() {
                        self.bindings.bind(item.bound_name().get().as_str(), false);
                    }
                }
                None => {
                    if renamed.is_none()
                        && let Ok(name) = import.bare_name()
                    {
                        self.bindings.bind(name.as_str(), false);
                    }
                }
                Some(ast::Imports::Wildcard) => {}
            }
            return;
        }
        if let Some(named) = node.get().cast::<ast::Named>() {
            self.named_value(&node, named);
            return;
        }
        if let Some(field) = node.get().cast::<ast::MathFieldAccess>() {
            if self.bindings.qualified > 0
                && qualified_math_name(ast::MathAccess::MathFieldAccess(field))
                    .is_some_and(|name| self.bindings.contains(&name))
            {
                self.ranges.push(node.range());
            } else if let Some(target) = node.find(field.target().to_untyped().span()) {
                self.visit(target);
            }
            return;
        }
        if let Some(field) = node.get().cast::<ast::FieldAccess>() {
            if self.bindings.qualified > 0
                && qualified_name(ast::Expr::FieldAccess(field))
                    .is_some_and(|name| self.bindings.contains(&name))
            {
                self.ranges.push(node.range());
            } else {
                self.expr(&node, field.target());
            }
            return;
        }
        if matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent) {
            if self.bindings.contains(node.get().leaf_text().as_str()) {
                self.ranges.push(node.range());
            }
            return;
        }
        let scope = matches!(
            node.kind(),
            SyntaxKind::CodeBlock | SyntaxKind::ContentBlock
        );
        if scope {
            self.bindings.enter();
        }
        self.children(node);
        if scope {
            self.bindings.exit();
        }
    }
}
fn qualified_name(mut expr: ast::Expr<'_>) -> Option<String> {
    // Bound work even for pathological chains in incomplete editor buffers.
    let mut parts = Vec::new();
    for _ in 0..32 {
        match expr {
            ast::Expr::Ident(name) => {
                parts.push(name.get().as_str());
                parts.reverse();
                return Some(parts.join("."));
            }
            ast::Expr::FieldAccess(field) => {
                parts.push(field.field().get().as_str());
                expr = field.target();
            }
            _ => return None,
        }
    }
    None
}

fn qualified_math_name(mut access: ast::MathAccess<'_>) -> Option<String> {
    let mut parts = Vec::new();
    for _ in 0..32 {
        match access {
            ast::MathAccess::MathIdent(name) => {
                parts.push(name.as_str());
                parts.reverse();
                return Some(parts.join("."));
            }
            ast::MathAccess::MathFieldAccess(field) => {
                parts.push(field.field().as_str());
                access = field.target();
            }
        }
    }
    None
}

// TeX has dynamic expansion and catcodes, so these are lexical declarations
// and uses only. No expansion, include traversal, or package symbol guessing.
pub(super) fn tex(source: &str) -> Vec<Range<usize>> {
    let mut bindings = Bindings::default();
    bindings.enter();
    let mut ranges = Vec::new();
    let mut i = 0;
    let mut global = false;
    let mut expl = false;
    while i < source.len() {
        match source.as_bytes()[i] {
            b'%' => {
                i = comment_end(source, i);
            }
            b'{' => {
                bindings.enter();
                i += 1;
            }
            b'}' => {
                if bindings.scopes.len() > 1 {
                    bindings.exit();
                }
                i += 1;
            }
            b'\\' => {
                let start = i;
                let end = control_end(source, i, expl);
                let name = &source[start + 1..end];
                i = end;
                if matches!(name, "verb" | "lstinline" | "mintinline")
                    && let Some(literal) =
                        crate::editor_math_guides::tex_inline_literal(source, start)
                {
                    i = literal.end;
                    continue;
                }
                if name == "ExplSyntaxOn" {
                    expl = true;
                }
                if name == "ExplSyntaxOff" {
                    expl = false;
                }
                if name == "global" {
                    global = true;
                    continue;
                }
                if matches!(name, "long" | "outer" | "protected") && global {
                    continue;
                }
                if macro_declaration(name) || environment_declaration(name) {
                    let env = environment_declaration(name);
                    if let Some((target, range, after)) = declaration_target(source, i, env, expl) {
                        let key = if env {
                            format!("env:{target}")
                        } else {
                            target.to_owned()
                        };
                        if global || matches!(name, "gdef" | "xdef") {
                            // Timestamped global bindings override older
                            // locals without scanning unrelated declarations.
                            bindings.bind_global(&key);
                        } else {
                            bindings.bind(&key, true);
                        }
                        // The declared control token is metadata, never a use.
                        debug_assert!(range.start < range.end);
                        i = after;
                    }
                    global = false;
                    continue;
                }
                global = false;
                if matches!(name, "begin" | "end") {
                    if let Some((env, range, after)) = environment_target(source, i) {
                        if name == "begin" && literal_environment(env) {
                            let close = format!("\\end{{{env}}}");
                            i = source[after..]
                                .find(&close)
                                .map_or(source.len(), |at| after + at + close.len());
                            continue;
                        }
                        if bindings.contains(&format!("env:{env}")) {
                            ranges.push(range);
                        }
                        if name == "begin" {
                            bindings.enter();
                        } else if bindings.scopes.len() > 1 {
                            bindings.exit();
                        }
                        i = after;
                    }
                } else if matches!(name, "begingroup" | "bgroup") {
                    bindings.enter();
                } else if matches!(name, "endgroup" | "egroup") {
                    if bindings.scopes.len() > 1 {
                        bindings.exit();
                    }
                } else if bindings.contains(&source[start..end]) {
                    ranges.push(start..end);
                }
            }
            _ => {
                i += source[i..].chars().next().unwrap().len_utf8();
            }
        }
    }
    ranges
}
fn macro_declaration(name: &str) -> bool {
    matches!(
        name,
        "newcommand"
            | "renewcommand"
            | "providecommand"
            | "DeclareRobustCommand"
            | "NewDocumentCommand"
            | "RenewDocumentCommand"
            | "ProvideDocumentCommand"
            | "DeclareDocumentCommand"
            | "DeclareMathOperator"
            | "def"
            | "gdef"
            | "edef"
            | "xdef"
            | "let"
    )
}
fn environment_declaration(name: &str) -> bool {
    matches!(
        name,
        "newenvironment"
            | "renewenvironment"
            | "NewDocumentEnvironment"
            | "RenewDocumentEnvironment"
            | "ProvideDocumentEnvironment"
            | "DeclareDocumentEnvironment"
            | "newtheorem"
    )
}
fn literal_environment(name: &str) -> bool {
    matches!(
        name,
        "verbatim"
            | "verbatim*"
            | "Verbatim"
            | "BVerbatim"
            | "LVerbatim"
            | "lstlisting"
            | "minted"
            | "comment"
    )
}
fn control_end(source: &str, start: usize, expl: bool) -> usize {
    let mut end = start + 1;
    for c in source[end..].chars() {
        if c.is_ascii_alphabetic() || c == '@' || (expl && matches!(c, '_' | ':')) {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    if end == start + 1
        && let Some(c) = source[end..].chars().next()
    {
        end += c.len_utf8();
    }
    end
}
fn comment_end(source: &str, start: usize) -> usize {
    source[start..]
        .find('\n')
        .map_or(source.len(), |at| start + at)
}
fn skip_space(source: &str, mut at: usize) -> usize {
    loop {
        while let Some(c) = source[at..].chars().next().filter(|c| c.is_whitespace()) {
            at += c.len_utf8();
        }
        if source[at..].starts_with('%') {
            at = comment_end(source, at);
        } else {
            return at;
        }
    }
}
fn environment_target(source: &str, start: usize) -> Option<(&str, Range<usize>, usize)> {
    let at = skip_space(source, start);
    let rest = source.get(at..)?.strip_prefix('{')?;
    let end = at + 1 + rest.find(['}', '{', '\\', '%', '\n', '\r'])?;
    if source.as_bytes()[end] != b'}' {
        return None;
    }
    let raw = &source[at + 1..end];
    let target = raw.trim();
    if target.is_empty() || target.contains(['\\', '%', '{']) {
        return None;
    }
    let begin = at + 1 + raw.len() - raw.trim_start().len();
    Some((target, begin..begin + target.len(), end + 1))
}
fn declaration_target(
    source: &str,
    start: usize,
    env: bool,
    expl: bool,
) -> Option<(&str, Range<usize>, usize)> {
    let mut at = skip_space(source, start);
    if source[at..].starts_with('*') {
        at = skip_space(source, at + 1);
    }
    if env {
        return environment_target(source, at);
    }
    let braced = source[at..].starts_with('{');
    if braced {
        at = skip_space(source, at + 1);
    }
    if !source[at..].starts_with('\\') {
        return None;
    }
    let end = control_end(source, at, expl);
    if end <= at + 1 {
        return None;
    }
    let mut after = end;
    if braced {
        after = skip_space(source, after);
        if source[after..].starts_with('}') {
            after += 1;
        }
    }
    Some((&source[at..end], at..end, after))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn typ_ranges(source: &str) -> Vec<Range<usize>> {
        typst(&Source::detached(source), 0, source.len())
    }
    fn typ_names(source: &str) -> Vec<String> {
        typ_ranges(source)
            .into_iter()
            .map(|range| source[range].to_owned())
            .collect()
    }
    fn tex_names(source: &str) -> Vec<String> {
        tex(source)
            .into_iter()
            .map(|range| source[range].to_owned())
            .collect()
    }

    #[test]
    fn typst_uses_follow_lexical_scopes_and_definition_order() {
        let source = "#outside\n#{ let outer = 1; outer; { let inner = 2; inner + outer }; inner }\n#outer\n#let later = 1\n#later";
        assert_eq!(typ_names(source), ["outer", "inner", "outer", "later"]);
    }
    #[test]
    fn typst_parameters_defaults_destructuring_and_loop_bindings_are_not_uses() {
        let source = "#let default = 1\n#let fun((a, b), value: default, ..rest) = a + b + value + rest.len()\n#fun((1, 2))\n#{ for item in (1, 2) { item }; item }\n#let (left, right) = (1, 2)\n#(left + right)";
        assert_eq!(
            typ_names(source),
            [
                "default", "a", "b", "value", "rest", "fun", "item", "left", "right"
            ]
        );
    }
    #[test]
    fn typst_member_names_argument_labels_and_dictionary_keys_are_metadata() {
        let source = "#let custom = (custom: 1)\n#let f(custom: 0) = custom\n#f(custom: custom.custom)\n#(custom.custom + (custom: custom).custom)";
        assert_eq!(
            typ_names(source),
            ["custom", "f", "custom", "custom", "custom"]
        );
    }
    #[test]
    fn typst_reassignment_targets_and_rule_targets_keep_normal_syntax() {
        let source = "#let foo = 1\n#{foo = foo + 1; foo += foo;}\n#set text(fill: red)\n#text[Hi]\n#show heading.where(level: foo): it => it.body\n#heading[Title]\n#set math.equation(numbering: none)\n$ math.equation(x) $";
        assert_eq!(
            typ_names(source),
            [
                "foo",
                "foo",
                "text",
                "foo",
                "it",
                "heading",
                "math.equation"
            ]
        );
    }
    #[test]
    fn typst_imports_shadow_but_do_not_claim_external_definitions() {
        let source = "#let thing = 1\n#thing\n#import \"external.typ\": thing\n#thing\n#{ let thing = 2; thing }\n#thing\n#import \"external.typ\" as thing\n#thing";
        assert_eq!(typ_names(source), ["thing", "thing"]);
    }
    #[test]
    fn typst_plain_math_identifiers_and_unicode_use_exact_byte_ranges() {
        let source =
            "#let α = 2\n#let foo = 3\n#α $foo$ α foo\n#(foo + α)\n\"α foo\" `#α #foo` // #foo";
        assert_eq!(typ_names(source), ["α", "foo", "foo", "α"]);
        assert!(typ_ranges(source).iter().all(
            |range| source.is_char_boundary(range.start) && source.is_char_boundary(range.end)
        ));
    }
    #[test]
    fn typst_code_wrappers_adjust_ranges_without_marking_synthetic_bytes() {
        let source = "let α = 1; α + 2";
        let parsed = Source::detached(format!("#{{{source}}}"));
        let start = source.rfind('α').unwrap();
        let ranges = typst(&parsed, 2, source.len());
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], start..start + 'α'.len_utf8());
        assert!(typst(&Source::detached(source), 0, source.len()).is_empty());
    }
    #[test]
    fn tex_multiline_declarations_comments_and_prefixes_do_not_create_fake_uses() {
        let source = "\\foo\n\\newcommand% target on next line\n  {\\foo}[1]{#1}\n\\foo{ok} \\foobar % \\foo\n\\\\newcommand{\\nope}{} \\nope\n\\% \\foo";
        assert_eq!(tex_names(source), ["\\foo", "\\foo"]);
    }
    #[test]
    fn tex_declared_environments_and_modern_xparse_macros_are_supported() {
        let source = "\\NewDocumentEnvironment{custom}{m}{}{}\n\\DeclareDocumentCommand{\\thing}{m}{#1}\n\\begin{custom}\\thing{x}\\end{custom}\n\\newtheorem{lemma}{Lemma}\n\\begin{lemma}Hi\\end{lemma}";
        assert_eq!(
            tex_names(source),
            ["custom", "\\thing", "custom", "lemma", "lemma"]
        );
    }
    #[test]
    fn tex_definitions_respect_groups_while_global_definitions_survive() {
        let source = "{\\def\\local{x} \\local} \\local\n{\\gdef\\globalmacro{x}} \\globalmacro\n\\begin{center}\\newcommand{\\inside}{x}\\inside\\end{center}\\inside\n{\\global\\long\\def\\another{x}}\\another";
        assert_eq!(
            tex_names(source),
            ["\\local", "\\globalmacro", "\\inside", "\\another"]
        );
    }
    #[test]
    fn tex_literal_regions_and_control_symbols_cannot_become_commands() {
        let source = "\\newcommand{\\foo}{x}\n\\verb|\\foo| \\verb*+\\foo+ \\lstinline[language=TeX]|\\foo|\n\\begin{Verbatim}\\foo\\def\\fake{}\\end{Verbatim}\n\\begin{minted}{tex}\\foo\\end{minted}\n\\foo \\fake";
        assert_eq!(tex_names(source), ["\\foo"]);
    }
    #[test]
    fn tex_let_source_is_a_use_but_target_is_a_definition() {
        let source = "\\newcommand{\\first}{x}\\let\\second=\\first \\second";
        assert_eq!(tex_names(source), ["\\first", "\\second"]);
    }
    #[test]
    fn tex_tolerates_unfinished_unicode_and_declarations() {
        for source in [
            "λ\\",
            "\\newcommand",
            "\\newcommand{",
            "\\def\\",
            "\\begin{",
            "\\verb",
            "\\verb\nλ",
            "\\lstinline[",
        ] {
            let _ = tex(source);
        }
        let source = "\\ExplSyntaxOn\\def\\custom_macro:N{ x }\\custom_macro:N";
        assert_eq!(tex_names(source), ["\\custom_macro:N"]);
    }
    #[test]
    fn tex_unclosed_environment_arguments_do_not_scan_repeatedly_to_eof() {
        let source = "\\begin{".repeat(10_000);
        assert!(tex(&source).is_empty());
    }
    #[test]
    fn typst_math_field_suffixes_are_metadata_and_qualified_rules_follow_root_identity() {
        let source = "#let long = 1\n#let custom = (long: 2)\n$arrow.r.long custom.long$\n#set math.equation(numbering: none)\n$math.equation(x)$\n#{ let math = (equation: x => x); math.equation(1) }\n#import \"external.typ\" as math\n#math.equation(2)";
        let ranges = typ_ranges(source);
        let names: Vec<_> = ranges.iter().map(|range| &source[range.clone()]).collect();
        assert_eq!(names, ["custom", "math.equation", "x", "math"]);
        let expected = source.find("$math.equation(x)$").unwrap() + 1;
        assert_eq!(ranges[1], expected..expected + "math.equation".len());
    }
    #[test]
    fn qualified_rule_is_restored_after_namespace_shadow_scope_exits() {
        let source = "#set math.equation(numbering: none)\n#{ let math = (equation: x => x); math.equation(1) }\n#math.equation(2)";
        assert_eq!(typ_names(source), ["x", "math", "math.equation"]);
    }
    #[test]
    fn tex_global_redefinition_overrides_old_locals_and_future_local_shadowing_is_restored() {
        let source =
            "\\def\\foo{x}{\\def\\foo{y}\\gdef\\foo{z}\\foo{\\def\\foo{q}\\foo}\\foo}\\foo";
        assert_eq!(tex_names(source), ["\\foo", "\\foo", "\\foo", "\\foo"]);
        let mut bindings = Bindings::default();
        bindings.enter();
        bindings.bind("name", false);
        bindings.enter();
        bindings.bind("name", false);
        bindings.bind_global("name");
        assert!(bindings.contains("name"));
        bindings.bind("name", false);
        assert!(!bindings.contains("name"));
        bindings.exit();
        assert!(bindings.contains("name"));
    }
    #[test]
    fn tex_global_definitions_do_not_walk_unrelated_bindings() {
        let mut bindings = Bindings::default();
        bindings.enter();
        for index in 0..10_000 {
            bindings.bind_global(&format!("\\macro{index}"));
        }
        assert!(bindings.names.is_empty());
        assert!(bindings.scopes[0].is_empty());
        assert_eq!(bindings.globals.len(), 10_000);
    }
    #[test]
    fn tex_minted_percent_delimiter_keeps_following_uses_visible() {
        let source = "\\def\\foo{x}\\mintinline{tex}%\\foo% \\foo";
        assert_eq!(tex_names(source), ["\\foo"]);
    }
    #[test]
    fn tex_minted_headers_accept_spacing_and_comments() {
        let source = "\\def\\foo{x}\\mintinline% header\n{tex}|\\foo| \\foo";
        assert_eq!(tex_names(source), ["\\foo"]);
    }
    #[test]
    fn tex_minted_inline_payload_is_literal_in_both_delimiter_forms() {
        let source = "\\def\\foo{x}\\mintinline[bgcolor=white]{tex}|\\foo|\\mintinline{tex}{\\foo {\\foo}}\\foo";
        assert_eq!(tex_names(source), ["\\foo"]);
    }
    #[test]
    fn tex_unclosed_inline_options_consume_once_to_end_of_line() {
        let source = "\\lstinline[".repeat(10_000);
        assert!(tex(&source).is_empty());
    }
    #[test]
    fn tex_verb_bracket_delimiters_do_not_swallow_following_actual_uses() {
        let source = "\\def\\foo{x}\\verb[\\foo[ \\foo \\verb*[\\foo[ \\foo";
        let ranges = tex(source);
        assert_eq!(tex_names(source), ["\\foo", "\\foo"]);
        let first = source.find("[ \\foo").unwrap() + 2;
        let second = source.rfind("[ \\foo").unwrap() + 2;
        assert_eq!(ranges, [first..first + 4, second..second + 4]);
    }
    #[test]
    fn tex_lstinline_star_is_a_payload_delimiter_not_a_verb_modifier() {
        let source = "\\def\\foo{x}\\lstinline*\\foo* \\foo";
        assert_eq!(tex_names(source), ["\\foo"]);
    }
    #[test]
    fn tex_mintinline_star_is_a_payload_delimiter_not_a_verb_modifier() {
        let source = "\\def\\foo{x}\\mintinline{tex}*\\foo* \\foo";
        assert_eq!(tex_names(source), ["\\foo"]);
    }
    #[test]
    fn incomplete_typst_declarations_and_rule_targets_never_become_uses() {
        for source in [
            "#let f(",
            "#let f(x)",
            "#let f=1\n#let f(",
            "#show heading:",
            "#let foo=1\n#show foo:",
            "#let bar=1\n#(bar:)",
            "#let bar=1\n#text(bar:)",
            "#let bar=1\n#let f(bar:)",
            "#let foo=1\n#{ foo = }",
            "#let foo=1\n#{ foo += }",
            "#for item in",
            "#let foo=1\n#let foo",
            "#let foo=1\n#let (foo, bar) =",
        ] {
            assert!(
                typ_ranges(source).is_empty(),
                "{source:?}: {:?}",
                typ_ranges(source)
            );
        }
    }
    #[test]
    fn incomplete_typst_loop_body_does_not_duplicate_its_iterable_use() {
        let source = "#let items=(1,2)\n#for item in items";
        let start = source.rfind("items").unwrap();
        let ranges = typ_ranges(source);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], start..start + 5);
        let source = "#let values=(1,2)\n#for item in { values }";
        let start = source.rfind("values").unwrap();
        let ranges = typ_ranges(source);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], start..start + 6);
        let source = "#let foo=(1,2)\n#for foo in foo";
        let start = source.rfind("foo").unwrap();
        let ranges = typ_ranges(source);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], start..start + 3);
    }
    #[test]
    fn actual_typst_values_remain_uses_while_declaration_is_being_typed() {
        for (source, name) in [
            ("#let foo=1\n#let other=foo +", "foo"),
            ("#let foo=1\n#show heading: foo", "foo"),
            ("#let bar=1\n#text(bar: bar)", "bar"),
        ] {
            let start = source.rfind(name).unwrap();
            let ranges = typ_ranges(source);
            assert_eq!(ranges.len(), 1, "{source}");
            assert_eq!(ranges[0], start..start + name.len(), "{source}");
        }
    }
    #[test]
    fn typst_import_alias_and_items_both_shadow_document_bindings() {
        let source =
            "#let local=1\n#let item=2\n#import \"external.typ\" as local: item\n#local #item";
        assert!(typ_ranges(source).is_empty());
        let source = "#let local=1\n#import as local";
        assert!(typ_ranges(source).is_empty());
        let source = "#let module=(value: 1)\n#import module as external: value";
        let start = source.rfind("module").unwrap();
        let ranges = typ_ranges(source);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], start..start + 6);
    }

    #[test]
    fn typst_assignment_rhs_are_uses_but_writes_are_not_and_markup_interpolation_is_a_use() {
        let source = "#let foo=1\n#{foo = foo + 1; foo += foo;}";
        let first = source.find("= foo").unwrap() + 2;
        let second = source.find("+= foo").unwrap() + 3;
        assert_eq!(typ_ranges(source), [first..first + 3, second..second + 3]);
        let source = "#let foo=1\n#foo =";
        let start = source.rfind("foo").unwrap();
        let ranges = typ_ranges(source);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], start..start + 3);
    }
}
