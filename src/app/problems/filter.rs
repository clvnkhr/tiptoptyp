//! Cached, provider-neutral Problems filtering. Source diagnostics stay untouched.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::diagnostics::{Diagnostic, DiagnosticSeverity, DiagnosticSource};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Filter {
    pub(crate) provider: Option<String>,
    pub(crate) severity: Option<DiagnosticSeverity>,
    pub(crate) category: Option<String>,
    pub(crate) query: String,
}

impl Filter {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_active(&self) -> bool {
        self.provider.is_some()
            || self.severity.is_some()
            || self.category.is_some()
            || !self.query.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Facet<T> {
    pub(crate) value: T,
    pub(crate) count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Group {
    pub(crate) provider: String,
    pub(crate) label: String,
    pub(crate) severity: DiagnosticSeverity,
    /// Positions in compiler.iter().chain(editor), never filtered-row offsets.
    pub(crate) indices: Vec<usize>,
}

struct Entry {
    provider: String,
    provider_slot: usize,
    severity: DiagnosticSeverity,
    category: Option<String>,
    category_slot: Option<usize>,
    search: String,
}

#[derive(Default)]
pub(crate) struct Model {
    generation: Option<u64>,
    main_path: Option<PathBuf>,
    entries: Vec<Entry>,
    providers: Vec<Facet<String>>,
    severities: Vec<Facet<DiagnosticSeverity>>,
    categories: Vec<Facet<String>>,
    applied: Option<Filter>,
    groups: Vec<Group>,
    matched_indices: Vec<usize>,
    visible_count: usize,
    #[cfg(test)]
    rebuilds: usize,
    #[cfg(test)]
    filter_runs: usize,
}

impl Model {
    /// Rebuild only after publication or a main-file rename. Call `refilter`
    /// after editing controls, before reading the resulting groups.
    pub(crate) fn update(
        &mut self,
        generation: u64,
        compiler: &[Diagnostic],
        editor: &[Diagnostic],
        main_path: Option<&Path>,
    ) {
        if self.generation == Some(generation) && self.main_path.as_deref() == main_path {
            return;
        }
        self.generation = Some(generation);
        self.main_path = main_path.map(Path::to_owned);
        self.entries = compiler
            .iter()
            .map(|diagnostic| entry(diagnostic, "Compiler", main_path))
            .chain(
                editor
                    .iter()
                    .map(|diagnostic| entry(diagnostic, "Other", main_path)),
            )
            .collect();
        let mut providers = BTreeMap::<String, usize>::new();
        let mut categories = BTreeMap::<String, usize>::new();
        let mut severities = [0; 5];
        for entry in &self.entries {
            *providers.entry(entry.provider.clone()).or_default() += 1;
            if let Some(category) = &entry.category {
                *categories.entry(category.clone()).or_default() += 1;
            }
            severities[severity_rank(entry.severity)] += 1;
        }
        self.providers = string_facets(providers);
        self.categories = string_facets(categories);
        let provider_slots: BTreeMap<_, _> = self
            .providers
            .iter()
            .enumerate()
            .map(|(index, facet)| (facet.value.as_str(), index))
            .collect();
        let category_slots: BTreeMap<_, _> = self
            .categories
            .iter()
            .enumerate()
            .map(|(index, facet)| (facet.value.as_str(), index))
            .collect();
        for entry in &mut self.entries {
            entry.provider_slot = provider_slots[entry.provider.as_str()];
            entry.category_slot = entry
                .category
                .as_deref()
                .and_then(|category| category_slots.get(category).copied());
        }
        self.severities = [
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Warning,
            DiagnosticSeverity::Note,
            DiagnosticSeverity::Help,
            DiagnosticSeverity::Unknown,
        ]
        .into_iter()
        .filter_map(|value| {
            let count = severities[severity_rank(value)];
            (count > 0).then_some(Facet { value, count })
        })
        .collect();
        self.applied = None;
        #[cfg(test)]
        {
            self.rebuilds += 1;
        }
    }

    /// Returns whether the visible view was rebuilt.
    pub(crate) fn refilter(&mut self, filter: &Filter) -> bool {
        if self.applied.as_ref() == Some(filter) {
            return false;
        }
        let terms: Vec<_> = filter
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let mut groups = BTreeMap::<(String, usize), Group>::new();
        let mut provider_counts = vec![0; self.providers.len()];
        let mut category_counts = vec![0; self.categories.len()];
        let mut severity_counts = [0; 5];
        self.visible_count = 0;
        self.matched_indices.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            let provider_matches = filter
                .provider
                .as_ref()
                .is_none_or(|provider| *provider == entry.provider);
            let severity_matches = filter
                .severity
                .is_none_or(|severity| severity == entry.severity);
            let category_matches = filter
                .category
                .as_ref()
                .is_none_or(|category| entry.category.as_ref() == Some(category));
            let search_matches = terms.iter().all(|term| entry.search.contains(term));
            if search_matches && severity_matches && category_matches {
                provider_counts[entry.provider_slot] += 1;
            }
            if search_matches && provider_matches && category_matches {
                severity_counts[severity_rank(entry.severity)] += 1;
            }
            if search_matches
                && provider_matches
                && severity_matches
                && let Some(slot) = entry.category_slot
            {
                category_counts[slot] += 1;
            }
            if !(provider_matches && severity_matches && category_matches && search_matches) {
                continue;
            }
            groups
                .entry((entry.provider.clone(), severity_rank(entry.severity)))
                .or_insert_with(|| Group {
                    provider: entry.provider.clone(),
                    label: format!("{} · {}", entry.provider, type_label(entry.severity)),
                    severity: entry.severity,
                    indices: Vec::new(),
                })
                .indices
                .push(index);
            self.matched_indices.push(index);
            self.visible_count += 1;
        }
        for (facet, count) in self.providers.iter_mut().zip(provider_counts) {
            facet.count = count;
        }
        for (facet, count) in self.categories.iter_mut().zip(category_counts) {
            facet.count = count;
        }
        for facet in &mut self.severities {
            facet.count = severity_counts[severity_rank(facet.value)];
        }
        self.groups = groups.into_values().collect();
        self.groups.sort_by(|a, b| {
            a.provider
                .to_lowercase()
                .cmp(&b.provider.to_lowercase())
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| severity_rank(a.severity).cmp(&severity_rank(b.severity)))
        });
        self.applied = Some(filter.clone());
        #[cfg(test)]
        {
            self.filter_runs += 1;
        }
        true
    }

    /// Original compiler-then-editor order, cached alongside the grouped view.
    pub(crate) fn indices(&self) -> &[usize] {
        &self.matched_indices
    }

    pub(crate) fn groups(&self) -> &[Group] {
        &self.groups
    }
    pub(crate) fn providers(&self) -> &[Facet<String>] {
        &self.providers
    }
    pub(crate) fn severities(&self) -> &[Facet<DiagnosticSeverity>] {
        &self.severities
    }
    pub(crate) fn categories(&self) -> &[Facet<String>] {
        &self.categories
    }
    pub(crate) fn total_count(&self) -> usize {
        self.entries.len()
    }
    pub(crate) fn visible_count(&self) -> usize {
        self.visible_count
    }
}

pub(crate) fn type_label(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Error => "Error",
        DiagnosticSeverity::Warning => "Warning",
        DiagnosticSeverity::Help => "Help",
        DiagnosticSeverity::Note => "Note",
        DiagnosticSeverity::Unknown => "Other",
    }
}

fn severity_rank(severity: DiagnosticSeverity) -> usize {
    match severity {
        DiagnosticSeverity::Error => 0,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Note => 2,
        DiagnosticSeverity::Help => 3,
        DiagnosticSeverity::Unknown => 4,
    }
}

fn string_facets(counts: BTreeMap<String, usize>) -> Vec<Facet<String>> {
    let mut values: Vec<_> = counts
        .into_iter()
        .map(|(value, count)| Facet { value, count })
        .collect();
    values.sort_by(|a, b| {
        a.value
            .to_lowercase()
            .cmp(&b.value.to_lowercase())
            .then_with(|| a.value.cmp(&b.value))
    });
    values
}

fn entry(diagnostic: &Diagnostic, fallback: &str, main_path: Option<&Path>) -> Entry {
    let provider = diagnostic
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .unwrap_or(fallback)
        .to_owned();
    let category = diagnostic
        .correction
        .as_ref()
        .and_then(|correction| correction.category.as_deref())
        .map(str::trim)
        .filter(|category| !category.is_empty())
        .map(str::to_owned);
    let mut search = String::new();
    for field in std::iter::once(diagnostic.message.as_str())
        .chain(diagnostic.details.iter().map(String::as_str))
        .chain([
            provider.as_str(),
            diagnostic.severity.label(),
            type_label(diagnostic.severity),
        ])
        .chain(
            diagnostic
                .correction
                .as_ref()
                .and_then(|correction| correction.rule.as_deref()),
        )
        .chain(category.as_deref())
    {
        search.push_str(field);
        search.push('\n');
    }
    let path = match &diagnostic.source {
        DiagnosticSource::Main => {
            search.push_str("main\n");
            main_path
        }
        DiagnosticSource::File(path) => Some(path.as_path()),
        DiagnosticSource::Global => {
            search.push_str("global\n");
            None
        }
    };
    if let Some(path) = path {
        search.push_str(&path.to_string_lossy());
        search.push('\n');
    }
    if let Some(location) = diagnostic.location {
        use std::fmt::Write as _;
        let _ = write!(
            search,
            "{}:{} line {} column {}",
            location.line, location.column, location.line, location.column
        );
    }
    Entry {
        provider,
        provider_slot: 0,
        severity: diagnostic.severity,
        category,
        category_slot: None,
        search: search.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{DiagnosticCorrection, DiagnosticLocation};

    fn diagnostic(
        provider: Option<&str>,
        severity: DiagnosticSeverity,
        message: &str,
    ) -> Diagnostic {
        Diagnostic {
            correction: None,
            provider: provider.map(str::to_owned),
            severity,
            source: DiagnosticSource::Main,
            location: Some(DiagnosticLocation {
                line: 42,
                column: 7,
            }),
            message: message.into(),
            details: Vec::new(),
        }
    }

    fn sample() -> Vec<Diagnostic> {
        vec![
            diagnostic(Some("Harper"), DiagnosticSeverity::Help, "Repeated word"),
            diagnostic(
                Some("Badness"),
                DiagnosticSeverity::Warning,
                "Redundant braces",
            ),
            diagnostic(
                Some("Harper"),
                DiagnosticSeverity::Help,
                "Apostrophe missing",
            ),
            diagnostic(Some("LTeX+"), DiagnosticSeverity::Warning, "Grammar issue"),
        ]
    }

    fn indices(model: &Model) -> Vec<usize> {
        model
            .groups()
            .iter()
            .flat_map(|group| group.indices.iter().copied())
            .collect()
    }

    #[test]
    fn search_matches_the_visible_other_type_for_unknown_severity() {
        let unknown = diagnostic(Some("Compiler"), DiagnosticSeverity::Unknown, "Tool output");
        let mut model = Model::default();
        model.update(1, &[unknown], &[], None);
        model.refilter(&Filter {
            query: "Other".into(),
            ..Default::default()
        });
        assert_eq!(
            model.indices(),
            [0],
            "the visible Other type must be searchable"
        );
        model.refilter(&Filter {
            query: "diagnostic".into(),
            ..Default::default()
        });
        assert_eq!(
            model.indices(),
            [0],
            "the underlying severity spelling remains searchable"
        );
    }

    #[test]
    fn source_type_and_search_are_combined_without_renumbering_diagnostics() {
        let mut model = Model::default();
        let diagnostics = sample();
        model.update(1, &diagnostics[..1], &diagnostics[1..], None);
        let filter = Filter {
            provider: Some("Harper".into()),
            severity: Some(DiagnosticSeverity::Help),
            query: "MISSING apostrophe".into(),
            ..Default::default()
        };
        model.refilter(&filter);
        assert_eq!(indices(&model), [2]);
        assert_eq!(model.total_count(), 4);
        assert_eq!(model.visible_count(), 1);
        assert_eq!(
            model
                .providers()
                .iter()
                .find(|facet| facet.value == "Harper")
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            model
                .severities()
                .iter()
                .map(|facet| facet.count)
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn search_covers_collapsed_details_rule_category_file_location_and_unicode() {
        let mut diagnostic = diagnostic(Some("Harper"), DiagnosticSeverity::Help, "ÜBER spelling");
        diagnostic.details = vec!["Hint: concealed explanation".into()];
        diagnostic.source = DiagnosticSource::File(PathBuf::from("/papers/Schrödinger.tex"));
        diagnostic.correction = Some(DiagnosticCorrection::new(
            0..4,
            "Über".into(),
            Some("ProperName".into()),
            Some("Spelling".into()),
            Vec::new(),
        ));
        let mut model = Model::default();
        model.update(0, &[diagnostic], &[], None);
        for term in [
            "über",
            "CONCEALED",
            "propername",
            "spelling",
            "SCHRÖDINGER.TEX",
            "42:7",
            "harper",
            "line 42",
            "column 7",
        ] {
            model.refilter(&Filter {
                query: term.into(),
                ..Default::default()
            });
            assert_eq!(model.visible_count(), 1, "missing {term}");
        }
        model.refilter(&Filter {
            query: "concealed nonexistent".into(),
            ..Default::default()
        });
        assert_eq!(model.visible_count(), 0);
        assert_eq!(
            model.categories(),
            [Facet {
                value: "Spelling".into(),
                count: 0
            }]
        );
    }

    #[test]
    fn facet_counts_respect_other_filters_and_preserve_zero_count_choices() {
        let mut diagnostics = sample();
        diagnostics[0].correction = Some(DiagnosticCorrection::new(
            0..0,
            String::new(),
            None,
            Some("Spelling".into()),
            Vec::new(),
        ));
        diagnostics[3].correction = Some(DiagnosticCorrection::new(
            0..0,
            String::new(),
            None,
            Some("Grammar".into()),
            Vec::new(),
        ));
        let mut model = Model::default();
        model.update(0, &[], &diagnostics, None);
        model.refilter(&Filter {
            provider: Some("Harper".into()),
            ..Default::default()
        });
        assert_eq!(
            model.severities(),
            [
                Facet {
                    value: DiagnosticSeverity::Warning,
                    count: 0
                },
                Facet {
                    value: DiagnosticSeverity::Help,
                    count: 2
                },
            ]
        );
        assert_eq!(
            model.categories(),
            [
                Facet {
                    value: "Grammar".into(),
                    count: 0
                },
                Facet {
                    value: "Spelling".into(),
                    count: 1
                },
            ]
        );
        model.refilter(&Filter {
            provider: Some("Harper".into()),
            severity: Some(DiagnosticSeverity::Warning),
            ..Default::default()
        });
        assert_eq!(model.visible_count(), 0);
        assert_eq!(
            model.providers(),
            [
                Facet {
                    value: "Badness".into(),
                    count: 1
                },
                Facet {
                    value: "Harper".into(),
                    count: 0
                },
                Facet {
                    value: "LTeX+".into(),
                    count: 1
                },
            ]
        );
        assert_eq!(
            model.severities()[1].count,
            2,
            "severity ignores only its own filter"
        );
        model.refilter(&Filter {
            provider: Some("Harper".into()),
            category: Some("Spelling".into()),
            query: "word".into(),
            ..Default::default()
        });
        assert_eq!(model.providers()[1].count, 1);
        assert_eq!(model.severities()[1].count, 1);
        assert_eq!(model.categories()[1].count, 1);
    }

    #[test]
    fn category_filter_excludes_uncategorized_rows_without_inventing_metadata() {
        let mut diagnostics = sample();
        diagnostics[0].correction = Some(DiagnosticCorrection::new(
            0..0,
            String::new(),
            None,
            Some("Spelling".into()),
            Vec::new(),
        ));
        let mut model = Model::default();
        model.update(0, &[], &diagnostics, None);
        model.refilter(&Filter {
            category: Some("Spelling".into()),
            ..Default::default()
        });
        assert_eq!(indices(&model), [0]);
        model.refilter(&Filter {
            category: Some("Grammar".into()),
            ..Default::default()
        });
        assert_eq!(model.visible_count(), 0);
    }

    #[test]
    fn missing_provider_filter_survives_refresh_and_clear_restores_every_row() {
        let diagnostics = sample();
        let mut model = Model::default();
        let mut filter = Filter {
            provider: Some("Harper".into()),
            ..Default::default()
        };
        model.update(0, &[], &diagnostics, None);
        model.refilter(&filter);
        assert_eq!(model.visible_count(), 2);
        model.update(1, &[], &diagnostics[1..2], None);
        model.refilter(&filter);
        assert_eq!(filter.provider.as_deref(), Some("Harper"));
        assert_eq!(model.total_count(), 1);
        assert_eq!(model.visible_count(), 0);
        assert!(filter.is_active());
        filter.clear();
        model.refilter(&filter);
        assert!(!filter.is_active());
        assert_eq!(model.visible_count(), 1);
    }

    #[test]
    fn idle_frames_reuse_index_facets_and_filter_matches() {
        let diagnostics = sample();
        let mut model = Model::default();
        let filter = Filter::default();
        for _ in 0..40 {
            model.update(7, &[], &diagnostics, Some(Path::new("/paper/main.tex")));
            model.refilter(&filter);
        }
        assert_eq!((model.rebuilds, model.filter_runs), (1, 1));
        model.refilter(&Filter {
            query: "word".into(),
            ..filter.clone()
        });
        assert_eq!((model.rebuilds, model.filter_runs), (1, 2));
        model.update(8, &[], &diagnostics, Some(Path::new("/paper/main.tex")));
        model.refilter(&filter);
        assert_eq!((model.rebuilds, model.filter_runs), (2, 3));
    }

    #[test]
    fn rename_invalidates_main_file_search_even_without_diagnostic_publication() {
        let diagnostics = sample();
        let mut model = Model::default();
        let filter = Filter {
            query: "new.tex".into(),
            ..Default::default()
        };
        model.update(1, &diagnostics, &[], Some(Path::new("old.tex")));
        model.refilter(&filter);
        assert_eq!(model.visible_count(), 0);
        model.update(1, &diagnostics, &[], Some(Path::new("new.tex")));
        model.refilter(&filter);
        assert_eq!(model.visible_count(), 4);
        assert_eq!(model.rebuilds, 2);
    }

    #[test]
    fn provider_fallback_is_not_confused_with_the_source_file() {
        let mut compiler = diagnostic(None, DiagnosticSeverity::Error, "Build failed");
        compiler.source = DiagnosticSource::File(PathBuf::from("some-file.tex"));
        let editor = diagnostic(None, DiagnosticSeverity::Unknown, "Other issue");
        let mut model = Model::default();
        model.update(0, &[compiler], &[editor], None);
        model.refilter(&Filter::default());
        assert_eq!(
            model
                .providers()
                .iter()
                .map(|facet| facet.value.as_str())
                .collect::<Vec<_>>(),
            ["Compiler", "Other"]
        );
        assert_eq!(indices(&model), [0, 1]);
        model.refilter(&Filter {
            provider: Some("some-file.tex".into()),
            ..Default::default()
        });
        assert_eq!(model.visible_count(), 0);
    }

    #[test]
    fn flat_indices_preserve_original_order_across_filters_and_reuse_cached_view() {
        let diagnostics = sample();
        let mut model = Model::default();
        model.update(7, &diagnostics[..2], &diagnostics[2..], None);
        let all = Filter::default();
        assert!(model.refilter(&all));
        assert_eq!(model.indices(), [0, 1, 2, 3]);
        assert_ne!(indices(&model), model.indices());

        let pointer = model.indices().as_ptr();
        for _ in 0..20 {
            model.update(7, &diagnostics[..2], &diagnostics[2..], None);
            assert!(!model.refilter(&all));
            assert_eq!(model.indices().as_ptr(), pointer);
            assert_eq!(model.indices(), [0, 1, 2, 3]);
        }
        assert_eq!((model.rebuilds, model.filter_runs), (1, 1));

        assert!(model.refilter(&Filter {
            provider: Some("Harper".into()),
            ..Default::default()
        }));
        assert_eq!(model.indices(), [0, 2]);
        assert!(model.refilter(&Filter {
            severity: Some(DiagnosticSeverity::Warning),
            ..Default::default()
        }));
        assert_eq!(model.indices(), [1, 3]);
        assert!(model.refilter(&Filter {
            provider: Some("Harper".into()),
            severity: Some(DiagnosticSeverity::Warning),
            ..Default::default()
        }));
        assert!(model.indices().is_empty());
        assert_eq!(model.visible_count(), 0);

        assert!(model.refilter(&all));
        assert_eq!(model.indices(), [0, 1, 2, 3]);
        assert_eq!(model.visible_count(), model.indices().len());
        assert_eq!((model.rebuilds, model.filter_runs), (1, 5));
    }

    #[test]
    fn groups_have_deterministic_provider_then_severity_order_and_keep_source_order() {
        let diagnostics = vec![
            diagnostic(Some("zeta"), DiagnosticSeverity::Help, "one"),
            diagnostic(Some("Alpha"), DiagnosticSeverity::Warning, "two"),
            diagnostic(Some("Alpha"), DiagnosticSeverity::Error, "three"),
            diagnostic(Some("Alpha"), DiagnosticSeverity::Warning, "four"),
        ];
        let mut model = Model::default();
        model.update(0, &diagnostics, &[], None);
        model.refilter(&Filter::default());
        assert_eq!(indices(&model), [2, 1, 3, 0]);
        assert_eq!(model.groups()[0].label, "Alpha · Error");
        assert_eq!(model.groups()[1].indices, [1, 3]);
    }
}
