use super::{Activity, Indicator};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Group {
    Preview,
    Code,
    Checks,
    Files,
    Git,
    Tools,
}

impl Group {
    pub(super) const ALL: [Self; 6] = [
        Self::Preview,
        Self::Code,
        Self::Checks,
        Self::Files,
        Self::Git,
        Self::Tools,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Preview => "Build & preview",
            Self::Code => "Code",
            Self::Checks => "Checks",
            Self::Files => "Files",
            Self::Git => "Git",
            Self::Tools => "Tools",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Status {
    Active,
    Working,
    Waiting,
    Failed,
    Idle,
    Inactive,
}

impl Status {
    pub(super) const ALL: [Self; 6] = [
        Self::Active,
        Self::Working,
        Self::Waiting,
        Self::Failed,
        Self::Idle,
        Self::Inactive,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Working => "Working",
            Self::Waiting => "Waiting",
            Self::Failed => "Failed",
            Self::Idle => "Idle",
            Self::Inactive => "Inactive",
        }
    }

    pub(super) fn of(state: &Activity) -> Self {
        match state {
            Activity::Running => Self::Working,
            Activity::Pending(_) => Self::Waiting,
            Activity::Failed(_) => Self::Failed,
            Activity::Idle => Self::Idle,
            Activity::Inactive(_) => Self::Inactive,
        }
    }

    pub(super) fn matches(self, state: &Activity) -> bool {
        match self {
            Self::Active => matches!(
                state,
                Activity::Running | Activity::Pending(_) | Activity::Failed(_)
            ),
            Self::Working => matches!(state, Activity::Running),
            Self::Waiting => matches!(state, Activity::Pending(_)),
            Self::Failed => matches!(state, Activity::Failed(_)),
            Self::Idle => matches!(state, Activity::Idle),
            Self::Inactive => matches!(state, Activity::Inactive(_)),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Filter {
    pub(super) query: String,
    pub(super) group: Option<Group>,
    pub(super) status: Option<Status>,
}

impl Filter {
    pub(super) fn is_active(&self) -> bool {
        !self.query.trim().is_empty() || self.group.is_some() || self.status.is_some()
    }

    pub(super) fn filter_indices(&self, rows: &[Indicator]) -> Vec<usize> {
        let needle = self.query.trim().to_lowercase();
        rows.iter()
            .enumerate()
            .filter_map(|(index, row)| self.matches(row, &needle).then_some(index))
            .collect()
    }

    fn matches(&self, row: &Indicator, needle: &str) -> bool {
        if self.group.is_some_and(|group| row.group != group)
            || self
                .status
                .is_some_and(|status| !status.matches(&row.state))
        {
            return false;
        }
        if needle.is_empty() {
            return true;
        }

        let contains = |value: &str| value.to_lowercase().contains(needle);
        contains(row.name)
            || contains(row.group.label())
            || contains(Status::of(&row.state).label())
            || contains(row.state.label())
            || matches!(&row.state, Activity::Failed(error) if contains(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Indicator> {
        vec![
            Indicator {
                name: "Tinymist",
                group: Group::Preview,
                role: "Descriptions must not silently affect search results.",
                state: Activity::Running,
            },
            Indicator {
                name: "TexLab",
                group: Group::Checks,
                role: "Checks source files.",
                state: Activity::Pending("Waiting for source"),
            },
            Indicator {
                name: "Harper",
                group: Group::Checks,
                role: "Checks spelling.",
                state: Activity::Failed("Could not load DICTIONARY".into()),
            },
            Indicator {
                name: "Save",
                group: Group::Files,
                role: "Writes the source file.",
                state: Activity::Idle,
            },
            Indicator {
                name: "Git",
                group: Group::Git,
                role: "Tracks source changes.",
                state: Activity::Inactive("No repository"),
            },
            Indicator {
                name: "Översikt",
                group: Group::Tools,
                role: "Lists symbols.",
                state: Activity::Failed("ÉCHEC du modèle".into()),
            },
        ]
    }

    #[test]
    fn facets_and_text_search_are_combined() {
        let rows = rows();
        let mut filter = Filter {
            query: "dictionary".into(),
            group: Some(Group::Checks),
            status: Some(Status::Failed),
        };
        assert_eq!(filter.filter_indices(&rows), vec![2]);
        filter.status = Some(Status::Waiting);
        assert!(filter.filter_indices(&rows).is_empty());
        filter.query = "source".into();
        assert_eq!(filter.filter_indices(&rows), vec![1]);
        filter.group = Some(Group::Files);
        assert!(filter.filter_indices(&rows).is_empty());
    }

    #[test]
    fn search_matches_visible_fields_including_reasons_but_not_role_prose() {
        let rows = rows();
        for (query, expected) in [
            ("TINYMIST", vec![0]),
            ("build & PREVIEW", vec![0]),
            ("working", vec![0]),
            ("waiting for source", vec![1]),
            ("failed", vec![2, 5]),
            ("no REPOSITORY", vec![4]),
            ("Up to date", vec![3]),
            ("descriptions", vec![]),
            ("spelling", vec![]),
        ] {
            let filter = Filter {
                query: query.into(),
                ..Default::default()
            };
            assert_eq!(filter.filter_indices(&rows), expected, "{query}");
        }
    }

    #[test]
    fn state_kind_labels_are_searchable_alongside_their_reasons() {
        let mut rows = rows();
        rows[1].state = Activity::Pending("Queued");
        rows[4].state = Activity::Inactive("Not in use");
        for (query, expected) in [
            ("waiting", vec![1]),
            ("queued", vec![1]),
            ("working", vec![0]),
            ("inactive", vec![4]),
            ("not in use", vec![4]),
            ("idle", vec![3]),
        ] {
            let filter = Filter {
                query: query.into(),
                ..Default::default()
            };
            assert_eq!(filter.filter_indices(&rows), expected, "{query}");
        }
    }

    #[test]
    fn unicode_names_and_errors_are_case_insensitive() {
        let rows = rows();
        for query in ["öVERSIKT", "échec", "MODÈLE"] {
            let filter = Filter {
                query: query.into(),
                ..Default::default()
            };
            assert_eq!(filter.filter_indices(&rows), vec![5], "{query}");
        }
    }

    #[test]
    fn active_includes_attention_and_work_without_idle_or_inactive_rows() {
        let states = [
            Activity::Running,
            Activity::Pending("Queued"),
            Activity::Failed("Failed".into()),
            Activity::Idle,
            Activity::Inactive("Disabled"),
        ];
        for (state, expected) in states.iter().zip([true, true, true, false, false]) {
            assert_eq!(Status::Active.matches(state), expected, "{state:?}");
        }
        for (status, expected) in [
            (Status::Working, 0),
            (Status::Waiting, 1),
            (Status::Failed, 2),
            (Status::Idle, 3),
            (Status::Inactive, 4),
        ] {
            let matches: Vec<_> = states
                .iter()
                .enumerate()
                .filter_map(|(index, state)| status.matches(state).then_some(index))
                .collect();
            assert_eq!(matches, vec![expected], "{status:?}");
            assert_eq!(Status::of(&states[expected]), status);
        }
    }

    #[test]
    fn default_and_whitespace_filters_preserve_order_and_do_not_mutate_rows() {
        let rows = rows();
        let original = rows.clone();
        let mut filter = Filter::default();
        assert!(!filter.is_active());
        assert_eq!(
            filter.filter_indices(&rows),
            (0..rows.len()).collect::<Vec<_>>()
        );
        filter.query = " \n\t ".into();
        assert!(!filter.is_active());
        assert_eq!(
            filter.filter_indices(&rows),
            (0..rows.len()).collect::<Vec<_>>()
        );
        filter.status = Some(Status::Active);
        assert!(filter.is_active());
        assert_eq!(filter.filter_indices(&rows), vec![0, 1, 2, 5]);
        filter.status = None;
        filter.group = Some(Group::Checks);
        assert!(filter.is_active());
        assert_eq!(filter.filter_indices(&rows), vec![1, 2]);
        assert_eq!(rows, original);
    }

    #[test]
    fn facets_have_unique_labels_and_a_stable_display_order() {
        assert_eq!(
            Group::ALL.map(Group::label),
            ["Build & preview", "Code", "Checks", "Files", "Git", "Tools",]
        );
        assert_eq!(
            Status::ALL.map(Status::label),
            ["Active", "Working", "Waiting", "Failed", "Idle", "Inactive",]
        );
    }
}
