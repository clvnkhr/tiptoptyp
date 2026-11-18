//! A workbench owns one finite sample request, bound to its locked source buffer.
use super::*;
use crate::table_sampling::{Engine, Request, View};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(super) struct Job {
    phase: Phase,
    locked: BTreeMap<crate::editor_features::CellPosition, String>,
}
#[derive(Default)]
enum Phase {
    #[default]
    Unrequested,
    NotNeeded,
    Sampling(Sampling),
    Ready(crate::table_sampling::Grid),
    Failed(String),
}
struct Sampling {
    cancel: Arc<AtomicBool>,
    worker: crate::worker::LatestJob<crate::table_sampling::Grid>,
}
impl Drop for Sampling {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Job {
    pub(super) fn activity(&self) -> crate::activity::Activity {
        use crate::activity::Activity;
        match &self.phase {
            Phase::Unrequested => Activity::Inactive("Not requested"),
            Phase::NotNeeded => Activity::Inactive("No computed cells"),
            Phase::Sampling(_) => Activity::Running,
            Phase::Ready(_) => Activity::Idle,
            Phase::Failed(error) => Activity::Failed(error.clone()),
        }
    }
    fn view(&self) -> View {
        View {
            locked: self.locked.clone(),
            grid: match &self.phase {
                Phase::Ready(grid) => Some(grid.clone()),
                _ => None,
            },
            pending: matches!(self.phase, Phase::Sampling(_)),
        }
    }
    fn publish(&self, context: &egui::Context, viewport: egui::ViewportId) {
        crate::table_sampling::publish(context, viewport, Arc::new(self.view()));
    }
    fn start(&mut self, request: Request, context: &egui::Context) {
        assert!(
            matches!(self.phase, Phase::NotNeeded),
            "samples are requested once per workbench"
        );
        let cancel = Arc::<AtomicBool>::default();
        let token = cancel.clone();
        let mut worker = crate::worker::LatestJob::default();
        self.phase = match worker.start_and_repaint("table-values", context, move || {
            crate::table_sampling::sample(request, &token)
        }) {
            Ok(()) => Phase::Sampling(Sampling { worker, cancel }),
            Err(error) => Phase::Failed(error),
        };
    }
    fn poll(&mut self) -> bool {
        let Phase::Sampling(sampling) = &mut self.phase else {
            return false;
        };
        self.phase = match sampling.worker.poll() {
            crate::worker::LatestJobPoll::Ready(grid) => Phase::Ready(grid),
            crate::worker::LatestJobPoll::Failed(error) => Phase::Failed(error),
            crate::worker::LatestJobPoll::Pending | crate::worker::LatestJobPoll::Idle => {
                return false;
            }
        };
        true
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspect(&self) -> serde_json::Value {
        let view = self.view();
        let phase = match self.phase {
            Phase::Unrequested => "unrequested",
            Phase::NotNeeded => "not_needed",
            Phase::Sampling(_) => "sampling",
            Phase::Ready(_) => "ready",
            Phase::Failed(_) => "failed",
        };
        serde_json::json!({"phase":phase,"pending":view.pending,"failed":matches!(self.phase, Phase::Failed(_)),
        "locked":view.locked.keys().collect::<Vec<_>>(),
        "grid":view.grid.as_ref().map(|grid| serde_json::json!({
            "columns":grid.columns,"rows":grid.rows,
            "values":grid.cells.iter().map(|cell| serde_json::json!({"position":cell.position,"fingerprint":crate::desktop_test::fingerprint(&cell.value)})).collect::<Vec<_>>()
        }))})
    }
    pub(super) fn ready_for_snapshot(&self) -> bool {
        !matches!(self.phase, Phase::Unrequested | Phase::Sampling(_))
    }
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}
impl EditorApp {
    pub(super) fn prepare_table_samples(&mut self, context: &egui::Context) {
        let viewport = scoped_child_viewport_id(context, "tiptoptyp-table-editor");
        if matches!(self.table_samples.phase, Phase::Unrequested) {
            self.table_samples.phase = Phase::NotNeeded;
            let mut locked = BTreeMap::new();
            let (range, call, engine, program) = match self.table_editor.as_ref() {
                Some(TableEditorDialog::Typst(dialog)) if dialog.insertion_prefix.is_none() => {
                    for (r, row) in dialog.table.cells.iter().enumerate() {
                        for (c, cell) in row.iter().enumerate() {
                            if crate::table_sampling::computed_markup(cell) {
                                locked.insert((r, c), cell.clone());
                            }
                        }
                    }
                    if locked.is_empty() {
                        return;
                    }
                    (
                        dialog.table.source_range.clone(),
                        dialog.original_call.clone(),
                        Engine::Typst {
                            command: self.settings.typst.command.clone(),
                            fonts: self.font_catalog.workspace_directories().to_vec(),
                            kind: dialog.table.kind.callee().into(),
                        },
                        self.typst_tool.program.clone(),
                    )
                }
                Some(TableEditorDialog::Tex(dialog)) => {
                    if let Some(rows) = &dialog.table.cells {
                        for (r, row) in rows.iter().enumerate() {
                            for (c, cell) in row.iter().enumerate() {
                                if tex_computed_markup(cell) {
                                    locked.insert((r, c), cell.clone());
                                }
                            }
                        }
                    }
                    if locked.is_empty() {
                        return;
                    }
                    let call = match crate::table_sampling::tex_instrument(&dialog.table, &locked) {
                        Ok(call) => call,
                        Err(error) => {
                            self.table_samples.locked = locked;
                            self.table_samples.phase = Phase::Failed(error);
                            self.table_samples.publish(context, viewport);
                            return;
                        }
                    };
                    let standard = self.settings.tex.build_engine
                        != crate::tex::settings::BuildEngine::Tectonic;
                    let (program, command) = if standard {
                        (
                            self.tex_tools
                                .distribution(self.settings.tex.build_engine)
                                .map(Path::to_owned)
                                .unwrap_or_default(),
                            crate::tool_command::CommandCustomization::default(),
                        )
                    } else {
                        (
                            self.tex_tools.tectonic.program.clone(),
                            self.settings.tex.tectonic.command.clone(),
                        )
                    };
                    (
                        dialog.table.source_range.clone(),
                        call,
                        Engine::Tex {
                            command,
                            standard,
                            only_cached: self.settings.tex.only_cached,
                            cells: locked.clone(),
                            columns: dialog.table.columns().unwrap_or(1),
                            rows: dialog.table.cells.as_ref().map_or(0, Vec::len),
                        },
                        program,
                    )
                }
                _ => return,
            };
            let root = self.project_root();
            let path = self.document().path();
            let source_dir = path
                .as_deref()
                .and_then(Path::parent)
                .unwrap_or(&root)
                .to_owned();
            let name = path
                .as_deref()
                .and_then(Path::file_name)
                .and_then(|x| x.to_str())
                .unwrap_or(if self.document().kind() == DocumentKind::Tex {
                    "table-sample.tex"
                } else {
                    "table-sample.typ"
                })
                .to_owned();
            let Some(range) = char_range_to_byte_range(self.document().source(), range) else {
                return;
            };
            self.table_samples.locked = locked;
            let request = Request {
                root,
                source_dir,
                name,
                source: self.document().source().into(),
                range,
                call,
                program,
                engine,
            };
            self.table_samples.start(request, context);
            self.table_samples.publish(context, viewport);
        }
        if !self.table_samples.poll() {
            return;
        }
        self.table_samples.publish(context, viewport);
        context.request_repaint_of(viewport);
    }
    pub(super) fn validate_table_sample_locks(&self) -> Result<(), String> {
        if let Some(TableEditorDialog::Typst(dialog)) = &self.table_editor {
            for (&(r, c), source) in &self.table_samples.locked {
                if dialog.table.cells.get(r).and_then(|row| row.get(c)) != Some(source) {
                    return Err(
                        "A sampled cell is locked. Change its variable in the code editor instead."
                            .into(),
                    );
                }
            }
        }
        if let Some(TableEditorDialog::Tex(dialog)) = &self.table_editor {
            for (&(r, c), source) in &self.table_samples.locked {
                if dialog
                    .table
                    .cells
                    .as_ref()
                    .and_then(|rows| rows.get(r))
                    .and_then(|row| row.get(c))
                    != Some(source)
                {
                    return Err(
                        "A sampled cell is locked. Change its macro in the code editor instead."
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
}
// Formatting controls retain editable source; imported user macros remain sampled.
fn tex_computed_markup(cell: &str) -> bool {
    tex_computed_markup_depth(cell, 0)
}

fn tex_computed_markup_depth(cell: &str, depth: usize) -> bool {
    if depth > 16 {
        return true;
    }
    let bytes = cell.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i] == b'\\' {
            if let Some(literal) = crate::editor_math_guides::tex_inline_literal(cell, i) {
                i = literal.end;
                continue;
            }
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            if start == i {
                i = (i + 1).min(bytes.len());
                continue;
            }
            let command = &cell[start..i];
            if matches!(
                command,
                "cite"
                    | "citep"
                    | "citet"
                    | "citealp"
                    | "citealt"
                    | "citeauthor"
                    | "citeyear"
                    | "citeyearpar"
                    | "citenum"
                    | "Citep"
                    | "Citet"
                    | "Citealt"
                    | "Citealp"
                    | "Citeauthor"
                    | "autocite"
                    | "Autocite"
                    | "parencite"
                    | "Parencite"
                    | "textcite"
                    | "Textcite"
                    | "footcite"
                    | "footcitetext"
                    | "supercite"
                    | "nocite"
                    | "ref"
                    | "pageref"
                    | "eqref"
                    | "autoref"
                    | "Autoref"
                    | "nameref"
                    | "Nameref"
                    | "cref"
                    | "Cref"
                    | "cpageref"
                    | "Cpageref"
                    | "label"
                    | "url"
                    | "href"
            ) {
                let Some(end) = tex_key_argument_end(cell, i, depth) else {
                    return true;
                };
                i = end;
            } else if !tex_literal_control(command) {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

fn tex_key_argument_end(cell: &str, mut i: usize, depth: usize) -> Option<usize> {
    let bytes = cell.as_bytes();
    if bytes.get(i) == Some(&b'*') {
        i += 1;
    }
    loop {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if bytes.get(i) != Some(&b'[') {
            break;
        }
        let Some(end) = tex_group_end(bytes, i, b'[', b']') else {
            return Some(i);
        };
        if tex_computed_markup_depth(&cell[i + 1..end - 1], depth + 1) {
            return None;
        }
        i = end;
    }
    Some(if bytes.get(i) == Some(&b'{') {
        tex_group_end(bytes, i, b'{', b'}').unwrap_or(i)
    } else {
        i
    })
}

fn tex_group_end(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 1usize;
    let mut braces = 0usize;
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'%' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' if open != b'{' => braces += 1,
            b'}' if open != b'{' => braces = braces.saturating_sub(1),
            value if value == open && braces == 0 => depth += 1,
            value if value == close && braces == 0 => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn tex_literal_control(command: &str) -> bool {
    matches!(
        command,
        "textbf"
            | "textit"
            | "emph"
            | "textrm"
            | "textsf"
            | "texttt"
            | "textnormal"
            | "textsc"
            | "textsl"
            | "textup"
            | "textmd"
            | "textsuperscript"
            | "textsubscript"
            | "bfseries"
            | "mdseries"
            | "itshape"
            | "slshape"
            | "scshape"
            | "upshape"
            | "rmfamily"
            | "sffamily"
            | "ttfamily"
            | "normalfont"
            | "tiny"
            | "scriptsize"
            | "footnotesize"
            | "small"
            | "normalsize"
            | "large"
            | "Large"
            | "LARGE"
            | "huge"
            | "Huge"
            | "underline"
            | "overline"
            | "uline"
            | "sout"
            | "text"
            | "mbox"
            | "makebox"
            | "parbox"
            | "shortstack"
            | "makecell"
            | "thead"
            | "multirow"
            | "multicolumn"
            | "rotatebox"
            | "scalebox"
            | "resizebox"
            | "raisebox"
            | "color"
            | "textcolor"
            | "colorbox"
            | "fcolorbox"
            | "cellcolor"
            | "hspace"
            | "vspace"
            | "hfill"
            | "vfill"
            | "quad"
            | "qquad"
            | "enspace"
            | "thinspace"
            | "medspace"
            | "thickspace"
            | "smallskip"
            | "medskip"
            | "bigskip"
            | "noindent"
            | "newline"
            | "linebreak"
            | "strut"
            | "rule"
            | "phantom"
            | "hphantom"
            | "vphantom"
            | "smash"
            | "centering"
            | "raggedleft"
            | "raggedright"
            | "arraybackslash"
            | "protect"
            | "relax"
            | "footnote"
            | "footnotemark"
            | "footnotetext"
            | "begin"
            | "end"
            | "TeX"
            | "LaTeX"
            | "ldots"
            | "cdots"
            | "dots"
            | "vdots"
            | "ddots"
            | "ldotp"
            | "cdotp"
            | "textbackslash"
            | "textasciitilde"
            | "textasciicircum"
            | "textdegree"
            | "textregistered"
            | "textcopyright"
            | "texttrademark"
            | "textendash"
            | "textemdash"
            | "ding"
            | "textwidth"
            | "linewidth"
            | "columnwidth"
            | "baselineskip"
            | "dimexpr"
            | "mathit"
            | "mathbf"
            | "mathrm"
            | "mathsf"
            | "mathtt"
            | "mathnormal"
            | "mathcal"
            | "mathbb"
            | "mathfrak"
            | "boldsymbol"
            | "bm"
            | "operatorname"
            | "mathop"
            | "mathbin"
            | "mathrel"
            | "mathord"
            | "mathopen"
            | "mathclose"
            | "mathpunct"
            | "ensuremath"
            | "displaystyle"
            | "textstyle"
            | "scriptstyle"
            | "scriptscriptstyle"
            | "frac"
            | "dfrac"
            | "tfrac"
            | "sqrt"
            | "binom"
            | "dbinom"
            | "tbinom"
            | "overset"
            | "underset"
            | "stackrel"
            | "substack"
            | "hat"
            | "widehat"
            | "bar"
            | "vec"
            | "dot"
            | "ddot"
            | "tilde"
            | "widetilde"
            | "check"
            | "breve"
            | "acute"
            | "grave"
            | "overbrace"
            | "underbrace"
            | "left"
            | "right"
            | "middle"
            | "big"
            | "Big"
            | "bigg"
            | "Bigg"
            | "bigl"
            | "bigr"
            | "Bigl"
            | "Bigr"
            | "biggl"
            | "biggr"
            | "Biggl"
            | "Biggr"
            | "sum"
            | "prod"
            | "coprod"
            | "int"
            | "iint"
            | "iiint"
            | "oint"
            | "lim"
            | "limsup"
            | "liminf"
            | "log"
            | "ln"
            | "exp"
            | "sin"
            | "cos"
            | "tan"
            | "arcsin"
            | "arccos"
            | "arctan"
            | "sinh"
            | "cosh"
            | "tanh"
            | "min"
            | "max"
            | "sup"
            | "inf"
            | "det"
            | "dim"
            | "ker"
            | "gcd"
            | "Pr"
            | "alpha"
            | "beta"
            | "gamma"
            | "delta"
            | "epsilon"
            | "varepsilon"
            | "zeta"
            | "eta"
            | "theta"
            | "vartheta"
            | "iota"
            | "kappa"
            | "varkappa"
            | "lambda"
            | "mu"
            | "nu"
            | "xi"
            | "pi"
            | "varpi"
            | "rho"
            | "varrho"
            | "sigma"
            | "varsigma"
            | "tau"
            | "upsilon"
            | "phi"
            | "varphi"
            | "chi"
            | "psi"
            | "omega"
            | "Gamma"
            | "Delta"
            | "Theta"
            | "Lambda"
            | "Xi"
            | "Pi"
            | "Sigma"
            | "Upsilon"
            | "Phi"
            | "Psi"
            | "Omega"
            | "partial"
            | "nabla"
            | "infty"
            | "emptyset"
            | "varnothing"
            | "forall"
            | "exists"
            | "neg"
            | "in"
            | "notin"
            | "subset"
            | "subseteq"
            | "supset"
            | "supseteq"
            | "cup"
            | "cap"
            | "setminus"
            | "times"
            | "cdot"
            | "pm"
            | "mp"
            | "div"
            | "ast"
            | "star"
            | "circ"
            | "bullet"
            | "oplus"
            | "otimes"
            | "le"
            | "leq"
            | "ge"
            | "geq"
            | "ne"
            | "neq"
            | "approx"
            | "sim"
            | "simeq"
            | "equiv"
            | "propto"
            | "to"
            | "rightarrow"
            | "leftarrow"
            | "leftrightarrow"
            | "Rightarrow"
            | "Leftarrow"
            | "Leftrightarrow"
            | "mapsto"
            | "langle"
            | "rangle"
            | "lvert"
            | "rvert"
            | "lVert"
            | "rVert"
            | "vert"
            | "Vert"
            | "lceil"
            | "rceil"
            | "lfloor"
            | "rfloor"
            | "limits"
            | "nolimits"
    )
}

pub(super) fn locked_cell(
    ui: &mut egui::Ui,
    position: crate::editor_features::CellPosition,
    _source: &str,
) -> Option<egui::Response> {
    let view = crate::table_sampling::view(ui)?;
    if !view.locked.contains_key(&position) {
        return None;
    }
    let text = view.locked_value(position).unwrap_or(if view.pending {
        "…"
    } else {
        "Sample unavailable"
    });
    let response = ui
        .push_id(("table-locked-value", position), |ui| {
            ui.add(egui::Label::new(text).selectable(true).wrap())
        })
        .inner;
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(
        &format!("table.locked.{}.{}", position.0, position.1),
        &response,
    );
    if response.clicked() {
        ui.memory_mut(|memory| {
            if let Some(focused) = memory.focused() {
                memory.surrender_focus(focused);
            }
        });
    }
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_and_view_are_projections_of_one_sample_phase() {
        use crate::activity::Activity;
        let mut job = Job::default();
        assert_eq!(job.activity(), Activity::Inactive("Not requested"));
        assert!(!job.ready_for_snapshot());
        job.phase = Phase::NotNeeded;
        assert_eq!(job.activity(), Activity::Inactive("No computed cells"));
        job.locked.insert((0, 0), "#price".into());
        job.phase = Phase::Failed("sample failed".into());
        assert_eq!(job.activity(), Activity::Failed("sample failed".into()));
        assert!(!job.view().pending);
        assert!(job.view().grid.is_none());
        job.phase = Phase::Ready(crate::table_sampling::Grid {
            columns: 1,
            rows: 1,
            cells: Vec::new(),
        });
        assert_eq!(job.activity(), Activity::Idle);
        assert!(!job.view().pending);
        assert!(job.ready_for_snapshot());
        job.reset();
        assert_eq!(job.activity(), Activity::Inactive("Not requested"));
        assert!(job.locked.is_empty());
    }

    #[test]
    fn sampling_reset_cancels_one_owned_worker_and_cannot_publish_its_late_reply() {
        let mut job = Job::default();
        let cancel = Arc::<AtomicBool>::default();
        let token = cancel.clone();
        let (release, released) = std::sync::mpsc::channel();
        let mut worker = crate::worker::LatestJob::default();
        worker
            .start("controlled sample", move || {
                released.recv().unwrap();
                Ok(crate::table_sampling::Grid::default())
            })
            .unwrap();
        job.phase = Phase::Sampling(Sampling { cancel, worker });
        assert_eq!(job.activity(), crate::activity::Activity::Running);
        assert!(job.view().pending);
        assert!(!job.ready_for_snapshot());
        job.reset();
        assert!(token.load(Ordering::Acquire));
        release.send(()).unwrap();
        assert!(!job.poll());
        assert!(!job.view().pending);
        assert!(job.view().grid.is_none());
    }
    #[test]
    fn table_samples_apply_refuses_programmatic_overwrite_of_computed_cells() {
        let dir = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, dir.path().into());
        let source = "#let price = 60\n#table(columns: 2, [Revenue], [#price])";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            dir.path().join("sample.typ"),
            DocumentKind::Typst,
            None,
        );
        app.begin_table_target(
            TableTarget::at(source, source.find("table(").unwrap(), DocumentKind::Typst).unwrap(),
        );
        app.table_samples.locked.insert((0, 1), "#price".into());
        assert!(app.validate_table_sample_locks().is_ok());
        let Some(TableEditorDialog::Typst(dialog)) = &mut app.table_editor else {
            unreachable!()
        };
        dialog.table.cells[0][1] = "60".into();
        assert!(app.validate_table_sample_locks().is_err());
        assert_eq!(app.document().source(), source);
    }
    #[test]
    fn table_samples_literal_arxiv_descriptions_are_editable_but_macro_symbols_are_locked() {
        let source = include_str!("../../tests/fixtures/arxiv-tables/platonic-metrics.tex");
        let table =
            crate::editor_features::tex_table_at(source, source.find("\\begin{tabular}").unwrap())
                .unwrap();
        let rows = table.cells.as_ref().unwrap();
        assert!(!tex_computed_markup(&rows[3][5]));
        assert!(rows[3][5].contains("Unbiased estimator of CKA"));
        assert!(tex_computed_markup(&rows[3][1]));
        assert!(rows[3][1].contains("\\cmark"));
    }

    #[test]
    fn table_samples_apply_refuses_overwrite_of_unknown_tex_macro_inside_markup() {
        let dir = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, dir.path().into());
        let source = r"\begin{tabular}{ll}Revenue & \textbf{\revenue}\\\end{tabular}";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            dir.path().join("sample.tex"),
            DocumentKind::Tex,
            None,
        );
        app.begin_table_target(TableTarget::at(source, 2, DocumentKind::Tex).unwrap());
        app.table_samples
            .locked
            .insert((0, 1), r"\textbf{\revenue}".into());
        assert!(app.validate_table_sample_locks().is_ok());
        let Some(TableEditorDialog::Tex(dialog)) = &mut app.table_editor else {
            unreachable!()
        };
        dialog.table.cells.as_mut().unwrap()[0][1] = "60".into();
        assert!(app.validate_table_sample_locks().is_err());
        assert_eq!(app.document().source(), source);
    }

    #[test]
    fn table_samples_standard_tex_markup_remains_editable() {
        for cell in [
            r"\textbf{Header}",
            r"\small symmetric",
            r"\multirow{2}{*}{\textbf{Metric}}",
            r"\multicolumn{4}{c}{\textbf{Property}}",
            r"\thead{\multirow{2}{*}{\textbf{Description}}}",
            r"{\vspace{-0.5em}Unbiased estimator of CKA that corrects for sample bias in HSIC~\cite{song2012feature}.}",
            r"\rotatebox{90}{\makecell{first\\second}}",
            r"$\frac{\alpha + \beta}{\sqrt{n}} \leq \infty$",
            r"See \citet[\emph{especially}][p.~3]{a,b}, \eqref{eq:label} and \label{sec:label}",
            r"\href{https://example.test/\notavariable}{\emph{Link}}",
        ] {
            assert!(
                !tex_computed_markup(cell),
                "literal markup must be editable: {cell}"
            );
        }
    }

    #[test]
    fn table_samples_unknown_tex_macros_stay_locked_inside_standard_markup() {
        for cell in [
            r"\revenue",
            r"\cmark",
            r"\textbf{\revenue}",
            r"\thead{\multirow{2}{*}{\cmark}}",
            r"\href{https://example.test}{\revenue}",
            r"\cite{key} \revenue",
        ] {
            assert!(
                tex_computed_markup(cell),
                "computed macro must remain locked: {cell}"
            );
        }
        for cell in [
            r"\lstinline|\revenue|",
            r"\mintinline{tex}{\revenue}",
            r"\cite{\citationkey}",
            r"\cite[{	extbf{See ]}}]{\citationkey}",
        ] {
            assert!(
                !tex_computed_markup(cell),
                "literal keys/code are not variables: {cell}"
            );
        }
    }

    #[test]
    fn table_samples_nested_reference_notes_are_bounded() {
        let mut cell = String::from("literal");
        for _ in 0..32 {
            cell = format!("\\cite[{cell}]{{key}}");
        }
        assert!(tex_computed_markup(&cell));
    }

    #[test]
    fn table_samples_macro_detection_handles_imports_comments_literals_and_escapes() {
        assert!(tex_computed_markup(r"Price: \price"));
        assert!(tex_computed_markup(r"\macrofromanotherfile"));
        assert!(tex_computed_markup(r"\textbf{\price}"));
        assert!(!tex_computed_markup(r"\\price"));
        assert!(!tex_computed_markup(r"\% \$ \&"));
        assert!(!tex_computed_markup("Plain % \\price\ntext"));
        assert!(!tex_computed_markup(r"\verb|\price|"));
        assert!(!tex_computed_markup(r"\verb*+\price+"));
        assert!(tex_computed_markup(r"\verb|literal| \price"));
    }
}
