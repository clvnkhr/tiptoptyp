#set document(title: "tiptoptyp — quick regression check")
#set page(paper: "a4", margin: 19mm)
#set text(size: 10.5pt)
#set par(leading: 0.55em)
#let check(title, body) = block(breakable: false, above: 0.85em)[
  #box(width: 8pt, height: 8pt, stroke: 0.6pt) #h(3pt) *#title*
  #linebreak() #body
]

= Quick regression check

*Ten checks. Start with 1–6 (roughly 5–10 minutes); 7–10 cover additional
refactor risks. Stop at the first failure.*
Use a disposable copy of a familiar document plus one other file. These target
native interactions automated tests can miss—not the entire application.
Leave untested boxes blank.

*Before starting:* use the freshly built executable directly, not a Dock icon
or app-name lookup that could launch an old bundle. Record the build/commit.
Do not risk unsaved work in real documents.

#check("1. Hover → popup → scroll → link", [
  Hover two different symbols. Move diagonally onto a card and scroll inside it;
  move away. Reopen it and scroll the editor. Finally click a harmless tooltip
  link once.
  *Pass:* hovers appear reliably, entering the card works, moving away/editor
  scrolling dismisses it, scrolling stays smooth, and the link opens promptly
  in exactly one browser tab. No need to keep repeating a flaky failure.
])

#check("2. Preview → source → keyboard", [
  Click preview text to jump to source. Try Option+Left/Right, Cmd+Left/Right,
  then Shift+Option+Right; type a character and undo.
  *Pass:* word/line movement and selection work immediately in the source editor;
  the edit and undo affect the right text. Cmd+A alone is not a pass.
])

#check("3. Tabs and Explorer", [
  Open the second file, drag its tab past the first, and switch back.
  Set a comfortable Explorer width; toggle Cmd+1 off/on and resize the window.
  *Pass:* the tab moves, not the whole window; text/caret and the chosen preview
  stay with their files; Explorer does not become a tiny strip.
])

#check("4. Save and cancel a close", [
  Edit and save the disposable file, then reopen it to confirm the text survived.
  With autosave temporarily off, make another edit, close the tab and choose Cancel.
  *Pass:* the saved edit is present; Cancel keeps the tab and unsaved edit intact.
  Restore your autosave setting afterward.
])

#check("5. Multiple-window independence and Settings", [
  Open different files from the same workspace in two windows. Open and narrow
  Settings, then alternate between editors: type/undo, Find and Save.
  Close one window; edit the survivor and check that its preview updates.
  *Pass:* commands affect only the focused document, Settings stays usable,
  and closing one window does not break the other's editing or preview.
  No noticeable sluggishness or focus stealing.
])

#check("6. Close and reopen", [
  Close the disposable tabs: the last tab should leave an empty workspace.
  Then close all document windows and use File > New Window or File > Open.
  *Pass:* no panic; a usable window returns with a sensible Explorer width,
  working editor and preview. Document-only menu actions are disabled when
  no applicable document exists.
])

#pagebreak()
== Four additional refactor checks

#check("7. Background-tab autosave", [
  With autosave enabled, make distinct edits in saved disposable files A and B,
  switching to B before A's delay expires. Wait beyond the delay, then inspect
  both files on disk or close and reopen them.
  *Pass:* each contains its own latest edit; switching tabs neither loses a save
  nor sends another tab's text to the wrong file. Restore your autosave setting.
])

#check("8. External-edit conflict", [
  Temporarily disable autosave. Make an unsaved edit in a disposable file, then
  change and save that same file in another editor. Return and attempt Save.
  *Pass:* the app reports the conflict and preserves local work; it does not
  silently overwrite the external version or reload over the unsaved edit.
  Cancel conflict resolution and restore your autosave setting.
])

#check("9. Late results must not cross tabs", [
  Request completion or hover in a large document, immediately switch to a
  different tab and type. Wait briefly for the original request to finish.
  *Pass:* no old popup, diagnostic or text replacement appears in the new tab;
  its text stays intact. If the request completed before switching, mark this
  inconclusive rather than spending time trying to win the race.
])

#check("10. Preview ownership across source and asset tabs", [
  Choose Typst file A for preview using its eye. Switch to file B, then open a
  PDF or image tab; scroll/zoom that asset and return to A.
  *Pass:* the chosen preview remains A. The asset occupies the code pane and its
  controls affect only that asset, not A's preview. Returning restores A's editor.
])

== Only when that area changed

*Choose at most one relevant extra—not the whole list:*
pairing: open a fence above an existing fence, then Enter;
miTeX: save one inline and one block expression and inspect canonical source;
Git: stage/unstage one disposable hunk and inspect the index;
PDF: scroll/zoom a long PDF and switch assets;
index: check that a computed-include warning names the actual file/expression.

== If something fails

Send the check number, build, last few actions and what happened (a short recording
helps for hover/focus). *You do not need to diagnose it or finish this sheet.*
The developer should reproduce it, add automated regression coverage and run
the deeper save/concurrency/resource/fault-injection tests—not delegate that
work to you. Passing this sheet is a smoke test, not proof the app is bug-free.
