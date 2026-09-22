# Bottom panel (todos 291–293)

Use **Panel** in the toolbar or View menu, or **Command+5** on macOS /
**Control+5** elsewhere. It opens or closes the shared bottom panel and remembers
whether Problems or Terminal was selected. The compact toolbar uses a panel
icon. **View → Terminal** and **Control+`** still toggle Terminal directly.
Both toggles work while the terminal owns keyboard focus and without an open
document. A window retains its shell when the panel is hidden.

Problems shows its diagnostic count in a badge beside the tab. Terminal has
22-point square refresh and folder buttons in a narrow right strip. Refresh
stops the shell and starts a new one in the current workspace. The folder button
opens a copyable starting-directory path and explains that shell `cd` commands
can change the live directory. The popover closes with Escape, outside clicks
or a tab change, and stays within the panel even at its minimum height. Shell
startup/exit status uses spare header space instead of another row.

The focus deadlock was caused by calling the viewport-scoped `terminal_id`
inside an egui memory write closure. Computing the ID reads the same context
lock. IDs are now resolved before taking memory locks; tab, close, toolbar,
menu and shortcut transitions use the same focus/visibility adapter.

Regression coverage includes the original lock failure, root/secondary-window
command routes, remembered tabs, input isolation, count alignment, strip/grid
bounds and the short-panel directory popover. The old lock regression failed
at egui's 10-second debug lock timeout and passes after removing reentrancy.
The idle path adds no worker or repaint timer, still reads one terminal snapshot
per visible frame, and no longer formats the path unless its popover is open.
No material performance impact is expected; this is not a native timing claim.

Fresh light-mode Terminal and Problems viewport captures were inspected under
`.tiptoptyp/screenshots/agent-review/bottom-panel/`. The reduced stable gallery
was regenerated in one app session and all 22 images validated. These are app
framebuffers, not composed desktop evidence. [Terminal](ui-snapshots/latest/main-terminal-panel--catppuccin-latte.png)
and [Problems](ui-snapshots/latest/main-problems-panel--catppuccin-latte.png)
show the compact controls and badge.
