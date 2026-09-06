#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")

= todo but deferred (big)
- there should be a proper pdf preview option that is just a proper pdf viewer.

= todo

- typst overrides should be in appearances subsection
- when we move off from theme to set in typst overrides it should start from the theme color
- lots of tooltips in typst overrides popup kinda suck. Too big, not useful. If useless just delete it, otherwise tighten the space
- next to the theme name in the selectors, we should be able to see at a glance a colour pallette of the theme.
- allow customising UI fonts
- cmd +/- should adjust UI size. Also put a slider for this in settings
- syntax highlight the markdown in the tooltips. If typc doesnt give colour coding then treat it as typ.

- if we are using a variable width font, add support to set the weight (typst overrides, and UI)
- change so that: allow triangular area from cursor to tooltip / error window to not dismiss. When mouse is over the popup, we can scroll.
- also add a keyboard shortcut to make the popup appear with keyboard focus (for scrolling up/down). Esc to unfocus and dismiss. There should be a subtle outline to indicate focus (as well as mouseover)
- I noticed that when I use a ```typ ...``` code block, it is not syntax highlighted correctly. Fix
- syntax highlight the markdown in the tooltips. If typc doesnt give colour coding then treat it as typ.
- Mitex and cmarker integration: autodetect strings/raw strings inside `#mi(...)` etc to syntax highlight with tex or markdown. Note that you may need to put the text in math mode or text mode depending on the mitex command
- for other colour formats (luma etc) we should give that text the appropriate fill, like we do now for ` #rgb(“hex number”)`
- package manager - inspect available packages, locally installed packages
- detect when file changed outside app. If we dont have any changes queued to be saved, then reload the file
- allow font ligatures in code editor
- image / pdf preview on hover (both in code, when we include images as part of the code, and in file explorer)
- configurable shortcuts everywhere. add a popup for this in settings
- add searcg to all everything - explorer windows, settings,
- view > … needs to be in the title bar as well, and also none of the shortcuts are appearing - add
- allow us to toggle off the title bar dupes of the menu bar items
- once in a while the text everywhere breaks (see pic) some sort of leak?
- after some time, the find/replace UI is not visible but is clickable. I realised this is because it is rendering behind the code editor panel. we shouldnt even make space for the find / replace UI - it should just be on top of the code panel.
- keyboard shortcuts when find and replace is active should work on find and replace text fields. and similarly for other text fields
- cmd+F / the Find button should toggle the find/replace popup, not just turn it on.
- there should be a button to toggle find and find/replace
- there should be options to allow case (in)sensitivity and regex (which we should be able to right click and get a cheat sheet). add single-symbol wide buttons for these
- if we right click and its already ‘use for preview’d then we should be able to toggle it off
- text highlight of file names is not readable, adjust color scheme
- should be able to right click file in explorer -> open in new window
- cmd-/ to toggle comment out current or selected line(s)
- shortcuts should be configurable
- move the problems button to the rightmost slot (matching with default shortcuts)
- remove ‘cmd/ctrl’ everywhere in popups/tooltips its just cmd
- fallback for CJK fonts
- autocomplete
- text is selectable in too many places
- file menu in menu bar does not match file in title bar. It should be single source of truth
- file explorer subsection separators should be draggable
- when closing the file explorer it should first stop showing all the contents before minimising
- there should be another subsection in the explorer, the tags/references
- color code various filetypes (various categories should be: .typ, other text-based files, pdfs and images, folders, and others) make sure you use colours from the theme
- the ttt in settings window is no longer left of file name in main window(s)
- the top right X in settings should not be there - use the Mac OS traffic lights
- selecting a file in file explorer resets the file explorer scroll amount. Why? Opening a file cannot trigger more files to appear. It should not redraw. If it does, at least restore the scroll
- the current line background should be also used for the current file.
- find and replace shortcut just gives find.
- we should (by default) have the current scopes and the current sections/subsections etc be "sticky rows" at the top so that we have context for what we are looking at. each level should give a further row that is persisted to the top. For instance if we are in ```typ
= first section
// <many lines>
== subsection
#let a = {
// <many lines>
// <cursor is here>
```
then overlayed at the top should be 
```typ
= first section
== subsection
#let a = {
```
(together with the line numbers). Basically another code panel floating on top but with those lines. There should also be line numbers, and clicking on the sticky row should jump in main code panel to that row
- sync back from code ->  preview should be one of the right click options
- right click menu if possible should use native right click. If not, at least the text should not be centered, it should be left-aligned like normal
- special detection of typst functions: set text(font: ...) should allow us to right click on font: … and then have a scrollable selector
- allow using fonts in the root of working directory or anywhere in the working directory if it is not a performance hit.
- Table maker / editor on right click (with detection of tables)
- the tooltips on hover should be wider. Maybe more like 80 char wide
- remove unsafe rust as much as possible
- it should format on manual save (cmd+s) (but not auto save)
- if we have a file set to be used for preview, then when we switch to a different file, we dont need to restart the preview window. Also, we should not close it even if we open a non-typ file. thats the point of the used for preview setting - so that we can edit other child files like a .toml or other .typ files
Any part of the UI with this tiny font size like that used in the file path of the file explorer should have a bigger font. Never use this small font. Also don’t use allcaps or smallcaps, not our style.
- in addition to the pdf ready indicator i also want all the greentext on the side e.g. applied theme saved automatically. and everything should be timestamped.
- we should reserve some space for the `*` in the name when a file is not yet saved, sot hat when it does appear, it doesn't shift the UI elements. 
- double clicking on a line in the Problems panel should jump to that line in the code panel
- the logo icon should be redone to match the settings 'ttt'
- on closing the app ran with `cargo compile --release` it should not lose the icon as it disappears?