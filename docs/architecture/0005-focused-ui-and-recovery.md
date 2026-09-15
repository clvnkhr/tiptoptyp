# Focused UI ownership and bounded service recovery

## Boundaries

- `core::recovery` owns the retry budget, generation admission, and deadline.
  Time is supplied by the caller. Five failed attempts allow four one-second
  delayed retries; duplicate failure/stop events cannot consume another attempt.
  A recovered service or an explicit restart resets the budget.
- `PreviewController` classifies Tinymist protocol failures and owns the
  waiting/fallback presentation. Preview-start errors count even when the LSP
  remains alive. Formatting and navigation errors do not restart the service.
  The application adapter stops/starts the process and schedules repaints.
- `ExplorerPanelState` owns visibility, remembered width, restoration intent,
  and search text. Both menu and shortcut actions call the same transitions.
  Widths from the blank closing frame cannot replace the remembered width.
- `settings_panel` borrows settings and read-only presentation inputs, owns
  transient interaction through `SettingsUiState`, and emits `SettingsAction`s.
  It cannot mutate a document or start a worker. The window adapter dispatches
  the actions after rendering, including file pickers and server restarts.
- `tooltips` owns tooltip geometry/timing rules, rendering, and viewport-scoped
  caches, without access to `EditorApp`. Native child-window composition remains
  in the window adapter.

The large application adapter still has further extraction opportunities.
These are ownership boundaries, not a claim that the entire application has
been decomposed. Architecture tests protect the focused renderers from broad
application imports and service calls.

## QA fixtures

`app::qa::QaSession` is the privileged fixture adapter. It owns the original
fixture document and temporary font, prepares scenes, and resets state between
batch steps. Normal document sessions do not initialize these fixtures.
Rendering still accepts deterministic presentation flags where needed to open
a picker, select a scroll position, or freeze a status.

The `git-panel` scene captures the real Explorer Git subpanel in `main`.
The screenshot-only Git child window, its viewport identity, and its old scene
name are removed. The fixture restores a visible Explorer width, including
after child-window captures. Repository polling belongs to the app update loop,
not the panel renderer, so rendering cannot overwrite an injected fixture with
a live scan. A semantic regression checks the restored width and real Git
controls. Scene parsing, routing, and gallery-manifest tests check the capture
contract.

## Settings recovery

Missing saved settings are a normal default configuration. Malformed or
incomplete settings return an error to startup, which shows a notice and records
the reason in the status log. Before valid preferences replace rejected input,
`AppSettings::save` copies the exact rejected value into
`tiptoptyp.settings.rejected` in the same storage. Ordinary later saves leave
that recovery record untouched. This is diagnostic preservation, not a schema
migration or compatibility alias.

## Automated checks

`.github/workflows/checks.yml` runs formatting, strict Clippy, application/core
tests, and xtask tests on Linux, macOS, and Windows. A separate macOS job installs
Poppler, fetches the hash-verified sidecars from the existing manifest, and runs
the normally ignored real-tool tests. A missing Tinymist executable now fails
an explicitly requested integration test instead of silently skipping it.

The workflow uses read-only repository permissions and does not retain checkout
credentials. Runner labels and checkout usage follow the
[GitHub-hosted runner documentation](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job).
Native screenshots remain the proportionate local QA step described in
`AGENTS.md`; the headless jobs do not claim composed-desktop visual coverage.
