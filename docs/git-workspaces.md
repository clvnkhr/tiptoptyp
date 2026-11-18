# Git backups for local and Overleaf workspaces

Open the workspace and expand **Git** in Explorer. Use **View → Git** if the
section is hidden. Git uses the workspace's files on disk.

## Set up the repository

1. Choose **Set up Git…**.
2. Review the folder shown in the form. For an Overleaf project this is its
   actual local working copy. If that folder already belongs to a parent
   repository, the form shows and configures that repository.
3. Enter the author name and email to record in commits. They are saved for
   this repository; other repositories and global Git settings are unchanged.
4. Choose an initial branch, normally `main`, for a new repository.
5. Choose **Existing remote / local only** to enter an `origin` URL, or leave it
   empty for local history. HTTPS, SSH and absolute local repository paths are supported.
   Authentication uses Git's configured credentials or SSH keys.
6. Choose **Create repository**, or follow the GitHub steps below to create a
   hosted repository as part of setup.

Existing-remote setup creates configuration only. It does not stage files,
create a commit or contact the remote. Existing repositories have **Configure Git…**
for reviewing the author and origin; their current branch is left unchanged.

## Create a GitHub repository

Select **GitHub** in the same setup page and choose **Sign in to GitHub**.
The app prepares a verified copy of the official GitHub CLI on first use.
A valid saved sign-in is reused. Otherwise, copy the displayed one-time code
and choose **Open GitHub** to approve sign-in in your browser. GitHub identifies
the authorization application as GitHub CLI. The app shows the confirmed account
before you create anything; **Sign in again** starts a fresh browser sign-in.

Review the repository name and visibility. **Private repository** is selected
by default.
Choose **Create on GitHub** to configure the local repository, create an empty
repository in that account and connect it as `origin`. This action does not
commit or push your files. Close setup with **Done**, then review and commit
your files before choosing **Push**.

An existing `origin` must be cleared and saved in **Existing remote / local only**
before creating another hosted repository. Account changes and configuration
changes require reviewing setup again.

Sign-in credentials use the system credential store. Helper configuration and
creation recovery records live in the app's private data folder outside your
workspace. The Git credential helper is configured only for this repository's
GitHub address and account. Other repositories and global Git settings are
unchanged. Sign-in requires a working secure credential store; the app removes
the helper's plaintext fallback if secure storage is unavailable.

If creation or connection is interrupted, reopen the workspace's Git setup.
The app retains the requested repository and offers **Retry connection**. It
checks the original account and repository before attaching; retry does not
send another creation request. Closing setup cannot undo a repository already
created on GitHub. A missing repository must be checked before reviewing a new
creation request.

## Save and back up an Overleaf project

Wait for the initial **Syncing files…** phase to finish so the local copy has
downloaded its project files. Save your editor changes, then use **Stage all**,
review the staged files, enter a commit message and choose **Commit staged changes**.
A commit records the saved local files. **Push** sends those commits to the configured
remote; the first push sets up branch tracking automatically.

This also works after **Work offline**: save the downloaded text locally, then
commit and push that copy. A Git backup can include local edits that have not
yet been published to Overleaf. Git records downloaded, locally saved files;
files unavailable in the offline cache require reconnecting to download them.

The app waits for pending local-copy writes before staging or committing.
Overleaf's own synchronization and conflict review continue to handle updates
to the linked project. **Fetch** remains available for inspecting remote Git
updates; **Pull** and Git **Revert** are disabled in a linked working copy so
they cannot replace files behind Overleaf's document and sync state. Ordinary
local workspaces retain those operations.

## What is included

Project source, bibliography files and downloaded assets, including PDF assets,
are eligible for Git. Setup excludes `.tiptoptyp/` through the repository's
private `info/exclude` file, preserving the project's `.gitignore`. Session
state, sync journals and app caches stay local. The app also refuses commits
that contain staged app metadata.

Existing history and already-tracked files are preserved. Adding an exclusion
does not remove files from old commits. Setup never rewrites history or
automatically removes tracked files.

## Review changes

The Git section shows working and staged changes, recent commits, branch and
tracking status. Double-click a changed text file for the editable comparison;
hover or click a gutter marker for the corresponding hunk. See
[Git diff editor](git-diff-editor.md).

Git work runs in the background. A failed operation leaves an error in the
panel and Activity. Setup checks that its workspace and reviewed configuration
still match before writing; if another window changes them, reopen the form to
review the current state.
