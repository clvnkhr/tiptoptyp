//! A hunk draft owns its original bytes. Applying is checked and undoable.
use crate::git::repository::{diff::Hunk, hunks};
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub(crate) kind: char,
    pub(crate) text: String,
    pub(crate) reverted: bool,
}
#[derive(Debug, Clone)]
pub(crate) struct Draft {
    pub(crate) text: String,
    original: String,
    pub(crate) rows: Vec<Row>,
    pub(crate) editing: bool,
    pub(crate) manual: bool,
}
impl Draft {
    pub(crate) fn new(hunk: &Hunk) -> Result<Self, String> {
        let mut rows: Vec<Row> = Vec::new();
        for line in hunk.text.split_inclusive('\n').skip(1) {
            let kind = line.chars().next().ok_or("Empty diff line")?;
            if kind == '\\' {
                let row = rows.last_mut().ok_or("Missing diff line")?;
                if row.text.ends_with('\n') {
                    row.text.pop();
                }
            } else if matches!(kind, ' ' | '+' | '-') {
                rows.push(Row {
                    kind,
                    text: line[1..].into(),
                    reverted: false,
                });
            } else {
                return Err("Invalid diff line".into());
            }
        }
        let text: String = rows
            .iter()
            .filter(|r| r.kind != '-')
            .map(|r| r.text.as_str())
            .collect();
        Ok(Self {
            original: text.clone(),
            text,
            rows,
            editing: false,
            manual: false,
        })
    }
    pub(crate) fn changed(&self) -> bool {
        self.text != self.original
    }
    pub(crate) fn toggle_line(&mut self, row: usize) {
        if self.manual {
            return;
        }
        if let Some(row) = self.rows.get_mut(row).filter(|r| r.kind != ' ') {
            row.reverted = !row.reverted;
        }
        self.text = self
            .rows
            .iter()
            .filter(|r| match r.kind {
                '-' => r.reverted,
                '+' => !r.reverted,
                _ => true,
            })
            .map(|r| r.text.as_str())
            .collect();
    }
    pub(crate) fn apply(source: &str, hunk: &Hunk, replacement: &str) -> Result<String, String> {
        hunks::replace(source, hunk, replacement)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn individual_lines_and_manual_edits_are_checked() {
        let hunk = Hunk {
            text: "@@ -1,2 +1,2 @@\n-old\n+new\n context\n".into(),
            changes: vec![],
        };
        let mut draft = Draft::new(&hunk).unwrap();
        draft.toggle_line(0);
        draft.toggle_line(1);
        assert_eq!(draft.text, "old\ncontext\n");
        assert_eq!(
            Draft::apply("new\ncontext\noutside\n", &hunk, &draft.text).unwrap(),
            "old\ncontext\noutside\n"
        );
        assert!(Draft::apply("changed\ncontext\n", &hunk, "manual\n").is_err());
        draft.manual = true;
        draft.text = "my edit\n".into();
        draft.toggle_line(0);
        assert_eq!(draft.text, "my edit\n");
    }
    #[test]
    fn preserves_missing_final_newline() {
        let hunk = Hunk { text: "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n".into(), changes: vec![] };
        let mut draft = Draft::new(&hunk).unwrap();
        draft.toggle_line(0);
        draft.toggle_line(1);
        assert_eq!(Draft::apply("new", &hunk, &draft.text).unwrap(), "old");
    }
}
