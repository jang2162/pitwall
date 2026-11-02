### Project window: tool window names and their open actions, editor tabs, file tree, status bar, background tasks, notifications, runtime environment, project trust, file watching.

## Tool window names. The id is the tool window id (PANELS in src/layout/project-layout.ts) in kebab-case.

tool-window-project = Project
tool-window-commit = Commit
tool-window-database = Database
tool-window-search = Search
tool-window-structure = Structure
tool-window-run = Run
tool-window-debug = Debug
tool-window-problems = Problems
tool-window-terminal = Terminal
tool-window-git = Git
tool-window-query-results = Query Results
tool-window-search-results = Search Results
tool-window-notifications = Notifications
tool-window-agent = Agent Activity

# $name: tool window name (tool-window-*)
action-tool-window = { $name } Tool Window
action-tool-window-hide-all = Hide All Tool Windows
action-tool-window-last = Jump to Last Tool Window
action-focus-editor = Focus Editor
action-layout-preset-default = Layout: Default
action-layout-preset-git-review = Layout: Git Review
action-layout-preset-db = Layout: Database
action-tool-window-stretch-left = Move Tool Window Edge Left
action-tool-window-stretch-right = Move Tool Window Edge Right
action-tool-window-stretch-up = Move Tool Window Edge Up
action-tool-window-stretch-down = Move Tool Window Edge Down

tool-window-bar-left = Left tool windows
tool-window-bar-right = Right tool windows
# $name: tool window name (tool-window-*)
tool-window-placeholder = { $name } — tool window placeholder

## Editor tabs (src/editor). One tab = one file; the centre groups are editor groups.

editor-empty = No open files
editor-loading = Loading…
# $message: the error as reported
editor-read-failed = Could not read the file: { $message }
# $size: the file size with its unit (e.g. 72.4 MiB)
editor-too-large = Not opened: the file is larger than 50 MiB ({ $size })
# Not a regular file (FIFO · device · socket): opening it blocks or never ends
editor-special-file = Not opened: this is not a regular file (FIFO, device or socket)
# $size: the size to show, with unit (e.g. 1.0 MiB)
editor-show-head = Show the first { $size }
editor-show-hex = View as Hex
# $shown: the size shown, $size: the file size (both with unit)
editor-head-notice = Showing only the first { $shown } (of { $size }) · read-only
editor-binary-as-text = Showing a binary file as text (bytes that are not UTF-8 show as �) · read-only
editor-read-only-large = Opened read-only: the file is larger than 5 MiB (highlighting and wrapping off)
# Actions row of an editor popup (M4-07) — the name screen readers read.
lsp-popup-actions = Popup actions
# Navigation (M4-08): declaration, implementation, usages, quick documentation, the location popup and outside sources.
action-lsp-goto-declaration = Go to Declaration or Usages
action-lsp-goto-implementation = Go to Implementation
action-lsp-find-usages = Find Usages
action-lsp-quick-documentation = Quick Documentation
# $count: number of locations
lsp-locations-definitions = Choose Declaration · { $count }
lsp-locations-implementations = Choose Implementation · { $count }
# $name: symbol name, $count: number of usages
lsp-locations-usages = Usages of “{ $name }” · { $count }
lsp-locations-none = Nothing found
# $key: the key that pins the list
lsp-locations-pin = { $key } to pin in Search Results
# $server: the language server that returned the location
lsp-outside-bar = Outside source · read-only · { $server }
# $message: the error text
lsp-outside-failed = Could not open the outside source: { $message }
# $encoding: the guessed encoding name (e.g. EUC-KR, UTF-16LE)
editor-read-only-encoding = Read as { $encoding } · opened read-only because it is not UTF-8
editor-read-only-lossy = Read as UTF-8 · opened read-only because some bytes are invalid (�)
# Hex viewer (M1-23)
editor-hex-label = Hex · read-only · 16 bytes per row
editor-hex-goto-title = Go to Offset
editor-hex-goto-input = Offset (4096 or 0x1000)
# $current: the current offset, $last: the offset of the last byte (both 0x hex)
editor-hex-goto-hint = Now { $current } · last { $last }
editor-hex-goto-invalid = Use decimal (4096) or hex (0x1000 or 1000h)
# $message: the original error
editor-hex-read-failed = Could not read the bytes: { $message }
# Image viewer (M1-22)
editor-image-fit = Fit
editor-image-actual = Actual Size
editor-image-zoom = Zoom
# $width $height: pixels
editor-image-dimensions = { $width } × { $height } px
# $message: the error
editor-image-read-failed = Could not read the image: { $message }
# Markdown viewer (M1-22)
editor-markdown-mode = Markdown View
editor-markdown-editor = Editor
editor-markdown-preview = Preview
editor-markdown-split = Editor and Preview
editor-markdown-image-too-large = Image not shown, larger than 50 MiB: { $size }
editor-markdown-image-limit = Image not shown, this document is over its 100 MiB / 50 image limit
# $path: the path that was being opened
editor-open-failed = Could not open { $path }
editor-copy-path-failed = Could not copy the path
editor-tab-pinned = Pinned
editor-tab-pin = Pin Tab
editor-tab-unpin = Unpin Tab
editor-switcher-label = Switcher
# Name of the unsaved mark (●) on a tab header.
editor-tab-unsaved = Unsaved changes
# $path: the file being saved
editor-save-failed = Could not save { $path }
# Body of the save failure notice: while you edited, the file on disk stopped being UTF-8 or lost its write permission.
editor-save-read-only = The file is now read-only, so it was not written. Your changes are still in the tab.

## External change banner (src/editor/disk-banner.tsx). The file on disk no longer matches this tab.

editor-disk-changed = The file changed on disk. It was not loaded because you have unsaved changes.
editor-disk-take = Load Disk Version
editor-disk-keep = Keep Mine
editor-disk-compare = Compare
editor-disk-compare-close = Close Comparison
editor-disk-compare-mine = Mine (unsaved)
editor-disk-compare-disk = Disk
editor-disk-deleted = The file was deleted on disk. Saving creates it again.
# $path: the new file with the same content
editor-disk-renamed = The file seems to have moved to { $path }
editor-disk-open-renamed = Open New Name
editor-disk-dismiss = Dismiss

## Unsaved changes prompt (src/editor/unsaved-dialog.tsx). Closing a tab or window, or quitting.

editor-unsaved-title = Unsaved changes
# $count: number of files, $files: file names, one per line
editor-unsaved-body =
    { $count ->
        [one] This file has unsaved changes:
       *[other] These { $count } files have unsaved changes:
    }
    { $files }
editor-unsaved-save = Save
editor-unsaved-discard = Don't Save
editor-unsaved-cancel = Cancel
# "Save" did not finish (conflict or failure). See the tab's banner or the notification.
editor-unsaved-save-incomplete = Not closed: some files could not be saved

action-editor-close-tab = Close Tab
# ⌘W · 메뉴 「닫기」: 에디터 탭이 있으면 탭, 없으면 창(10-ui §5.2).
action-editor-close-tab-or-window = Close Tab or Window
action-editor-close-other-tabs = Close Other Tabs
action-editor-split-right = Split Right
action-editor-toggle-pin = Pin / Unpin Tab
action-editor-toggle-maximized = Maximize / Restore Editor
action-editor-copy-path = Copy Path
# $platform: mac | win | linux
action-editor-reveal =
    { $platform ->
        [mac] Reveal in Finder
        [win] Show in Explorer
       *[other] Show in File Manager
    }
action-editor-switcher = Switcher
# ⌘S: 활성 탭의 문서를 저장한다(자동 저장 없음, 결정 M1-D13).
action-editor-save = Save
action-editor-save-all = Save All

## Editor operations (M1-18): find and replace bar (src/editor/find-bar.tsx), go to line (goto-line.tsx), editing keys
## (editor-ops.ts), editor items in the status bar (editor-status.tsx).

action-editor-find = Find
action-editor-replace = Replace
action-editor-goto-line = Go to Line
action-editor-undo = Undo
action-editor-redo = Redo
action-editor-duplicate-line = Duplicate Line
action-editor-delete-line = Delete Line
action-editor-move-line-up = Move Line Up
action-editor-move-line-down = Move Line Down
action-editor-comment-line = Comment with Line Comment
action-editor-select-next-occurrence = Add Selection for Next Occurrence
action-editor-unselect-occurrence = Unselect Occurrence
action-editor-select-all-occurrences = Select All Occurrences
action-editor-add-cursor-above = Clone Caret Above
action-editor-add-cursor-below = Clone Caret Below
action-editor-toggle-column-mode = Column Selection Mode
action-editor-single-cursor = Remove Secondary Carets
# $cap: the limit of Select All Occurrences (multiple carets, M4-14)
editor-occurrences-capped = More than { $cap } occurrences: selected the { $cap } around the caret
action-editor-open-as-text = Open as Text
action-editor-open-as-hex = Open as Hex
action-editor-open-as-default = Open with Default Viewer
action-hex-goto-offset = Go to Offset

editor-find-label = Find in file
editor-find-search = Search
editor-find-replace = Replace with
editor-find-case = Match Case
editor-find-word = Words
editor-find-regexp = Regex
editor-find-previous = Previous Occurrence (⇧↩)
editor-find-next = Next Occurrence (↩)
editor-find-replace-one = Replace
editor-find-replace-all = Replace All
editor-find-close = Close (Esc)
editor-find-invalid = Bad regex
editor-find-none = 0 results
# Still counting a large document and no match found yet
editor-find-counting = Counting…
# $total: number of matches (text like "9999+" when counting stopped at the cap)
editor-find-count =
    { $total ->
        [one] 1 result
       *[other] { $total } results
    }
# $current: position of the selected match, $total: number of matches
editor-find-count-current = { $current }/{ $total }

## Navigation history (M4-09, src/editor/navigation-ui.tsx): back and forward (⌘[ and ⌘]) and the Recent Locations popup (⌘⇧E).

nav-action-back = Back
nav-action-forward = Forward
nav-action-recent-locations = Recent Locations
nav-recent-empty = No location recorded yet

editor-goto-title = Go to Line
editor-goto-input = Line[:column]
# $cursor: the cursor now (line:column), $lines: lines in the document
editor-goto-hint = Now at { $cursor } · { $lines } lines
editor-goto-invalid = Type a line or line:column (for example 120:5)
editor-goto-go = Go
editor-goto-cancel = Cancel

# $line · $column: the cursor
status-editor-cursor = { $line }:{ $column }
# $chars: selected characters, $breaks: line breaks in the selection
status-editor-selection =
    { $breaks ->
        [0] { $chars ->
            [one] 1 char
           *[other] { $chars } chars
        }
       *[other] { $chars } chars, { $breaks ->
            [one] 1 line break
           *[other] { $breaks } line breaks
        }
    }
status-editor-cursor-hint = Go to Line
# $count: number of carets (two or more — multiple carets, M4-14)
status-editor-cursors = { $count } carets
# Column selection mode is on
status-editor-column-mode = Column
status-editor-column-mode-hint = Column selection mode: Shift+arrows and dragging select a rectangle
# $ending: lf | crlf | cr | mixed
status-editor-line-ending =
    { $ending ->
        [lf] LF
        [crlf] CRLF
        [cr] CR
       *[other] Mixed
    }
status-editor-line-ending-hint = Line separators (kept as they are on save)
status-editor-encoding-hint = File encoding
# $encoding: the encoding name (UTF-8)
status-editor-encoding-lossy = { $encoding } (invalid bytes)
status-editor-language-hint = Change the language of this file
language-plain-text = Plain Text
language-popup-label = Choose a language
language-popup-placeholder = Language name
language-popup-empty = No matching language
# $language: the detected language name
language-popup-detect = Back to detection ({ $language })
language-popup-popular = Popular
language-popup-all = All languages

## File tree (src/file-tree). The content of the Project tool window.

file-tree-label = Project files
file-tree-no-root = Reading the project root…
file-tree-loading = Loading…
# $message: the Host's error message
file-tree-read-failed = Could not read: { $message }
file-tree-outside = Points outside the root
file-tree-ignored-hide = Hide Ignored Files
file-tree-ignored-show = Show Ignored Files
file-tree-search-label = Search in tree
file-tree-search-none = No matches
file-tree-copy-relative-path = Copy Relative Path
action-file-tree-select-in = Select in Project View
action-file-tree-toggle-ignored = Hide / Show Ignored Files
action-file-tree-new-file = New File
action-file-tree-new-folder = New Folder
action-file-tree-rename = Rename (Project View)
action-file-tree-delete = Delete (Project View)
action-file-tree-open-in-terminal = Open in Terminal
# The first item of the "Select In" (⌥F1) popup.
open-with-project-view = Project View
open-with-project-view-no-file = No file is open
open-with-project-view-outside = This file is outside the project tree

## File operations (src/file-tree/file-ops.ts · file-dialogs.tsx, M1-21). The tree's context menu · ⇧F6 · ⌫ · drag · Finder drop.

file-ops-menu-new-file = New File…
file-ops-menu-new-folder = New Folder…
file-ops-menu-rename = Rename…
file-ops-menu-delete = Delete…
file-ops-menu-open-in-terminal = Open in Terminal
file-ops-name-new-file = New File
file-ops-name-new-folder = New Folder
file-ops-name-rename = Rename
# $folder: the name of the folder it goes into
file-ops-name-in = In “{ $folder }”
file-ops-name-label = Name
file-ops-name-empty = Enter a name
file-ops-name-separator = A name cannot contain / or \
file-ops-name-dots = This name cannot be used
file-ops-ok = OK
file-ops-cancel = Cancel
file-ops-delete-title = Delete
# $count: number of items, $name: the item's name when there is one
file-ops-delete-body =
    { $count ->
        [1] Move “{ $name }” to the Trash.
       *[other] Move { $count } items to the Trash.
    }
# $files: file names (one per line). Those tabs stay open with the “deleted” banner — saving creates the file again.
file-ops-delete-unsaved =
    Some files have unsaved changes. Their tabs stay open:
    { $files }
file-ops-delete-confirm = Move to Trash
file-ops-permanent-title = Trash Not Available
# $files: item names (one per line), $reason: the trash's error message
file-ops-permanent-body =
    These items could not be moved to the Trash ({ $reason }). Deleting them permanently cannot be undone:
    { $files }
file-ops-permanent-confirm = Delete Permanently
# $name: the name that is taken
file-ops-exists = “{ $name }” already exists — nothing was changed
# $message: the original error
file-ops-create-failed = Could not create: { $message }
file-ops-rename-failed = Could not rename: { $message }
file-ops-move-failed = Could not move: { $message }
file-ops-copy-failed = Could not copy: { $message }
file-ops-delete-failed = Could not delete: { $message }
# $files: item names (one per line)
file-ops-delete-permission =
    No permission to delete:
    { $files }
file-ops-drop-task = Copying dropped items
# $files: source names (one per line)
file-ops-drop-conflicts =
    Not copied because the name is taken:
    { $files }
file-ops-drop-skipped =
    Skipped special files (FIFO · socket · device):
    { $files }
file-ops-drop-cancelled = Stopped copying the dropped items — finished ones stay
file-ops-drop-failed = Could not copy the dropped items: { $message }

## Status bar

status-bar-label = Status bar
status-host-connecting = Connecting…
status-host-failed = Connection failed
# $appearance: light | dark — the theme currently drawn
status-appearance =
    { $appearance ->
        [light] Light
       *[dark] Dark
    }

## Background tasks

task-list-label = Background tasks
# $title: task name
task-cancel = Cancel “{ $title }”
# $count: other tasks not shown in the status bar
task-more = +{ $count }
# $title: task name
task-failed = Task failed: { $title }
task-start-failed = Could not start the task
task-cancel-failed = Could not cancel the task
# Background task titles sent by the host (`task-title-*` ids).
task-title-indexing-files = Indexing files
task-title-indexing-symbols = Indexing symbols
task-title-git-continue = Continuing the git operation
task-title-git-abort = Aborting the git operation
task-title-git-skip = Skipping a step of the git operation
task-title-git-commit = Committing
task-title-git-stage = Staging changes
task-title-git-unstage = Unstaging changes
task-title-git-discard = Discarding changes
task-title-git-stash-push = Stashing changes
task-title-git-stash-apply = Applying a stash
task-title-git-stash-pop = Popping a stash
task-title-git-stash-drop = Dropping a stash
task-title-git-status = Reading git status
task-title-git-resolve = Resolving a git conflict
task-title-git-merge = Merging
task-title-git-cherry-pick = Cherry-picking
task-title-git-revert = Reverting
task-title-git-rebase = Rebasing
task-title-git-reset = Resetting
task-title-git-checkout = Checking out
task-title-git-branch-create = Creating a branch
task-title-git-branch-rename = Renaming a branch
task-title-git-branch-delete = Deleting a branch
task-title-git-tag-create = Creating a tag
task-title-git-fetch = Fetching
task-title-git-pull = Pulling
task-title-git-push = Pushing
action-task-demo = Run Demo Task
action-task-demo-invalid = Run Demo Task (invalid arguments — to check error notifications)

## Notifications

notifications-empty = No notifications
notifications-clear = Clear All
# $count: error notifications not yet seen
status-notifications =
    { $count ->
        [0] Notifications
        [one] Notifications — { $count } unseen error
       *[other] Notifications — { $count } unseen errors
    }
# $level: info | warning | error
notification-level =
    { $level ->
        [error] Error
        [warning] Warning
       *[info] Info
    }

# $title: notification title, $count: how many were merged (2 or more)
notification-repeated = { $title } (×{ $count })

## Runtime environment

action-runtime-reload = Reload Runtime Environment
runtime-reload-failed = Could not reload the runtime environment
# Status bar item (src/runtime/runtime-status.tsx): which provider built the environment, and a warning when it fell back.
status-runtime-mise = mise
# $files: the mise config files, one per line
status-runtime-mise-hint =
    The runtime environment comes from mise. Click to reload it.
    { $files }
# $problem: loginShell | notInstalled | notTrusted | failed
status-runtime-fallback =
    { $problem ->
        [notInstalled] mise not found
        [notTrusted] mise trust required
        [failed] mise failed
       *[loginShell] Login shell not read
    }
# $problem: as above, $reason: the original error
status-runtime-fallback-hint =
    { $problem ->
        [notInstalled] The project has mise settings, but mise is not on the login shell PATH. The login shell environment is used.
        [notTrusted] mise does not trust this project's settings. Run `mise trust` in the project, then reload. The login shell environment is used.
        [failed] mise failed, so the login shell environment is used.
       *[loginShell] The login shell environment could not be read, so the app's own environment is used.
    }
    { $reason }
    Click to reload the runtime environment.

## Project trust. Untrusted projects do not run runtime providers (mise and so on) and use only the login shell environment.

status-trust-needed = Runtime environment not applied (trust required)
status-trust-needed-hint = Until you trust this project, only the login shell environment is used. Click to trust it
status-trust-unsavable = Runtime environment not applied (trust cannot be saved)
status-trust-unsavable-hint = The project registry cannot be written (read-only or in memory), so this project cannot be trusted. Only the login shell environment is used
# $name: project name
trust-dialog-title = Trust the project “{ $name }”?
trust-dialog-body =
    Trusting it builds the environment from the project's runtime settings (mise.toml and so on). Those settings can run the project's scripts.
    If you do not trust it, only the login shell environment is used. You can change this any time from the status bar or an action.
trust-dialog-trust = Trust
trust-dialog-distrust = Don't Trust
action-project-trust = Trust This Project…
action-project-untrust = Stop Trusting This Project
trust-save-failed = Could not save the project trust

## File watching. The Host sends the limit warning as a notification (notification.show, crates/pw-fs/src/watch). The status bar item is src/fs/file-watch.tsx.

# $root: the watched root, $seconds: polling interval in seconds
fs-watch-limit = The file watch limit (fs.inotify.max_user_watches) was reached; some folders are checked every { $seconds } seconds

status-file-watch-polling = File watch: polling
# $roots: the roots watched by polling, one per line
status-file-watch-polling-hint =
    The file watch limit (fs.inotify.max_user_watches) was reached, so some folders are checked by polling. Changes in them show up late.
    { $roots }

## Terminal tool window

terminal-tabs-label = Terminal tabs
# $index: tab number among tabs of the same name (from 1). No number when 1.
terminal-tab-name =
    { $index ->
        [1] Local
       *[other] Local ({ $index })
    }
terminal-tab-exited = exited
terminal-new-tab = New Terminal Tab
# $name: tab name
terminal-close-tab = Close “{ $name }”
terminal-rename-input = Tab name
# $code: exit code, or none when ended by a signal.
terminal-exited =
    { $code ->
        [none] [Process exited]
       *[other] [Process exited with code { $code }]
    }
# $message: the reason the Host reported
terminal-start-failed = [Could not open the terminal: { $message }]
terminal-no-host = Cannot open a terminal: not connected to the Host
terminal-copy-failed = Could not copy the terminal selection
terminal-paste-failed = Could not paste the clipboard into the terminal
# A path has a control character, or the shell is unknown (its quoting rules are unknown)
terminal-drop-refused = Did not paste the dropped paths: the terminal’s shell cannot take one of them safely
action-terminal-new-tab = Terminal: New Tab
action-terminal-close-tab = Terminal: Close Tab
action-terminal-rename-tab = Terminal: Rename Tab
action-terminal-copy = Terminal: Copy
action-terminal-paste = Terminal: Paste

## Find in files (src/search). The ⌘⇧F popup and the Search Results tool window.

action-find-in-files = Find in Files
find-placeholder = Text to find
find-match-case = Match Case
find-whole-word = Words
find-regex = Regex
find-exclude-vendored = Exclude Vendored and Generated Files
find-scope-project = In Project
find-scope-folder = Directory
find-folder-label = Folder to search (relative to the project root)
find-mask-label = File mask
find-mask-placeholder = e.g. *.rs, !*_test.rs
find-searching = Searching…
# $lines: matched lines, $files: files
find-summary =
    { $lines ->
        [one] 1 line
       *[other] { $lines } lines
    } in { $files ->
        [one] 1 file
       *[other] { $files } files
    }
# Past the result limit (10,000 lines)
find-more = More results not shown
find-cancelled = Stopped
# $message: the Host's reason (bad regex, glob, …)
find-failed = Search failed: { $message }
# The Host's limit on concurrent searches was hit
find-busy = Other searches are still running, so this one could not start — try again in a moment
find-empty = Nothing found
find-stop = Stop
find-pin = Open in Tool Window
find-preview-label = Preview
find-preview-loading = Loading…
# $message: the Host's reason
find-preview-failed = Could not read the preview: { $message }
find-results-empty = No pinned search results. Open in Tool Window in the Find in Files popup puts them here.
# $query: the text searched for
find-results-title = "{ $query }"
find-in-folder = Find in Folder

## Git. The Host sends the writes-off warning once as a notification (notification.show, crates/pw-git). The body is the path of that git executable.

# $version: the version of that git (or why it could not be read), $required: the lowest version that can write
git-too-old = Git { $version } is older than { $required }, so commits, branches and other changes are turned off. Viewing still works

## Databases. The Host sends these as notifications (notification.show, crates/pw-db/src/session.rs) when it rolls back an open
## transaction nobody came back to. The same rollbacks are listed by db.session.unattendedRollbacks for the next time the window opens.

# $connection: the connection name, $minutes: how long the Host kept the transaction for the window to come back
db-unattended-transaction-rolled-back = The open transaction on { $connection } was rolled back: no window came back to it within { $minutes } min
# $connection: the connection name
db-closed-transaction-rolled-back = The open transaction on { $connection } was rolled back because its window's connection closed

## Git authentication (src/git/git-prompts.tsx). git or ssh asks for a password, token, key passphrase or host key confirmation.
## The question shown in the dialog is git's or ssh's own text and is not translated.

git-askpass-title = Git asks for your credentials
git-askpass-confirm-title = Git asks for your confirmation
git-askpass-notice-title = Git is waiting for your security key
git-askpass-input = Answer
git-askpass-submit = OK
git-askpass-cancel = Cancel
git-askpass-yes = Yes
git-askpass-no = No
git-askpass-dismiss = Close

## Git status (src/git/git-status-mark.tsx). The name a screen reader reads for the letter (M · A · D …) on file tree rows and editor tabs, and the tooltip.

git-status-added = Added
git-status-modified = Modified
git-status-deleted = Deleted
git-status-renamed = Renamed
git-status-copied = Copied
git-status-type-changed = Type changed
git-status-untracked = Untracked
git-status-conflicted = Conflict
git-status-conflict-both-modified = Conflict: both modified
git-status-conflict-both-added = Conflict: both added
git-status-conflict-both-deleted = Conflict: both deleted
git-status-conflict-added-by-us = Conflict: added by us
git-status-conflict-added-by-them = Conflict: added by them
git-status-conflict-deleted-by-us = Conflict: deleted by us
git-status-conflict-deleted-by-them = Conflict: deleted by them
# $change: the change name (git-status-*)
git-status-staged = Staged: { $change }
# $change: the change name (git-status-*)
git-status-unstaged = Working tree: { $change }
# A folder's color: something below it changed
git-status-folder = Contains changed files

## Branches (src/git/branch-*.tsx, M2-13): the status bar branch item, the branch popup (⌃⇧B) and the VCS operations popup (⌃V).
action-git-branches = Branches…
action-git-vcs-operations = VCS Operations…
# Tooltip of the status bar item: the repository it found (work tree top)
branch-status-repository = Repository: { $path }
# Screen reader name. $name: "main ↑1↓2 MERGING"
branch-status-label = Branch { $name }
branch-status-unsupported = Unsupported repository
branch-status-unsupported-detail = Pitwall cannot read this repository's ref format (reftable) yet, so git features are off. git in the terminal still works
branch-status-detached = Detached HEAD
# Marker of an operation in progress (same words as git's prompt). $kind: merge · rebase · cherryPick · revert · bisect · am
branch-status-operation = { $kind ->
        [merge] MERGING
        [rebase] REBASING
        [cherryPick] CHERRY-PICKING
        [revert] REVERTING
        [bisect] BISECTING
       *[other] AM
    }
branch-popup-label = Branches
branch-popup-placeholder = Find a branch, tag or worktree
branch-popup-empty = Nothing matches
branch-popup-loading = Reading branches…
branch-popup-no-repository = This project is not a git repository
branch-popup-failed = Could not read the branches: { $message }
branch-group-favorites = Favorites
branch-group-local = Local
branch-group-remote = Remote
branch-group-tags = Tags
branch-group-worktrees = Worktrees
branch-new = New Branch…
branch-current = current
branch-favorite = Add to Favorites
branch-unfavorite = Remove from Favorites
branch-worktree-main = main
branch-worktree-current = this window
branch-worktree-missing = folder missing
branch-worktree-locked = locked
branch-worktree-other = cannot open here
branch-back = back
branch-checkout = Checkout
# Asks for the local branch that tracks a remote branch (when that name is taken). $local: the remote branch (origin/x)
branch-checkout-tracking = Local branch tracking { $local }
branch-new-from = New Branch from { $name }…
branch-merge = Merge { $name } into { $current }
branch-rebase = Rebase { $current } onto { $name }
branch-compare = Compare with { $current }
branch-rename = Rename…
branch-delete = Delete
branch-new-hint = Created from { $start } and checked out
branch-name-empty = Enter a name
branch-name-bad = Not a valid name, or the branch exists
branch-merge-title = Merge { $name } into { $current }
branch-rebase-title = Rebase { $current } onto { $name }
branch-history-body = On a conflict it stops and stays in progress (the status bar shows a marker)
branch-proceed = Proceed
branch-result-checked-out = Checked out { $name }
branch-result-created = Created branch { $name }
branch-result-renamed = Renamed the branch to { $name }
# $commit: short hash the deleted branch pointed at — a branch can be made there again
branch-result-deleted = Deleted branch { $name } ({ $commit })
branch-result-merged = Merged { $name }
branch-result-rebased = Rebased { $name }
branch-result-up-to-date = Already up to date
branch-result-stopped = Stopped on a conflict — resolve it and continue
branch-result-failed = { $what } failed
branch-compare-missing = This window has no compare view yet
vcs-popup-label = VCS Operations
vcs-popup-placeholder = Find an operation
vcs-popup-empty = No git operations in this window

## Git documents (src/git/documents · src/git/diff, M2-15): diff view · commit details · compare two commits · file at a revision. Hashes are 7 characters.

action-git-show-commit = Show Commit Details
action-git-compare-commits = Compare Two Commits
action-git-open-revision-file = Open File at Revision
action-diff-next-change = Next Difference in Diff
action-diff-previous-change = Previous Difference in Diff
git-doc-commit-title = Commit { $hash }
git-doc-compare-title = { $a } ↔ { $b }
git-doc-revision-title = { $name } ({ $hash })
# A revision blame document (M2-17). $name: file name, $hash: short hash
git-doc-blame-title = { $name } (blame { $hash })
git-doc-loading = Loading…
git-doc-failed = Could not open: { $message }
git-doc-bad-args = Invalid arguments for { $action }
git-doc-author = Author
git-doc-committer = Committer
git-doc-parents = Parents
git-doc-no-parents = None (root commit)
git-doc-shallow = Shallow clone boundary — the parents are not in this repository
git-doc-files = { $count } changed files
git-doc-no-files = No changed files
git-doc-compare-heading = From { $a } to { $b }
git-doc-revision-bar = Revision { $hash } · read-only
git-doc-pick-file = Select a file on the left
git-diff-mode = Diff view
git-diff-split = Side by side
git-diff-unified = Unified
git-diff-ignore-whitespace = Ignore whitespace
git-diff-next = Next difference
git-diff-previous = Previous difference
git-diff-changes = { $count } changes
git-diff-unchanged-lines = $ unchanged lines
git-diff-generated = Generated file — the diff is collapsed (+{ $added } −{ $removed })
git-diff-expand = Expand
git-diff-too-large = The file is too large to show a diff
git-diff-binary = Binary file — no content diff
git-diff-submodule = Submodule commit { $old } → { $new }
git-diff-submodule-none = (none)
git-diff-error = Could not read this file: { $message }
git-diff-no-content-change = The content is unchanged
git-diff-mode-change = Mode { $old } → { $new }
git-diff-renamed = Renamed from { $old }
git-diff-side-unreadable = One side cannot be read as text

## Rename · code actions · batch edits (15-m4-plan M4-12)
lsp-edit-action-rename = Rename Symbol
lsp-edit-action-code-actions = Show Code Actions
lsp-edit-action-undo-last = Undo Last Batch Edit
lsp-edit-quick-fix = Quick Fix
lsp-edit-rename-input = New name
lsp-edit-rename-hint = ↩ to rename · Esc to cancel
lsp-edit-rename-invalid = This cannot be renamed
lsp-edit-rename-invalid-reason = This cannot be renamed: { $message }
lsp-edit-rename-not-ready = The language server is still getting ready — try again in a moment
lsp-edit-rename-failed = Could not rename: { $message }
lsp-edit-code-actions = Code actions
lsp-edit-code-actions-none = No code actions here
lsp-edit-code-action-failed = Could not apply the code action: { $message }
lsp-edit-stale = Files changed after the language server read them — try again
lsp-edit-foreign = A file is open in another window — save it there and try again: { $files }
lsp-edit-ended = The command ended, so its edit was not applied
lsp-edit-refused = This edit is not applied: { $message }
lsp-edit-failed = Could not apply the edit: { $message }
lsp-edit-applied =
    { $count ->
        [one] Changed 1 file
       *[other] Changed { $count } files
    }
lsp-edit-editor-only = Put in the editor only because of unsaved edits: { $files }
lsp-edit-not-written = Not written because they changed meanwhile: { $files }
lsp-edit-undo = Undo
lsp-edit-reverted =
    { $count ->
        [one] Reverted 1 file
       *[other] Reverted { $count } files
    }
lsp-edit-not-reverted = Not reverted because they changed since: { $files }
lsp-edit-nothing-to-undo = No batch edit to undo
lsp-edit-preview-title =
    { $count ->
        [one] 1 file will change
       *[other] { $count } files will change
    }
lsp-edit-preview-apply = Apply
lsp-edit-preview-cancel = Cancel
lsp-edit-preview-editor-only = Editor only (unsaved edits)
lsp-edit-preview-lines =
    { $lines ->
        [one] 1 line
       *[other] { $lines } lines
    }
lsp-edit-preview-create = New file
lsp-edit-preview-rename = Renamed from { $from }
lsp-edit-preview-delete = Deleted

## Structure tool window and file structure popup (src/structure, M4-10). Names from the server are shown as plain text.
# Source label. $server: language server id (rust-analyzer)
structure-source-lsp = LSP · { $server }
structure-source-index = Index
structure-source-label = Symbol source
structure-tree = Structure
structure-follow-cursor = Follow cursor
structure-sort-name = Sort by name
structure-empty-none = No open file
structure-empty-loading = Reading symbols…
structure-empty-symbols = No symbols
structure-failed = Could not read the symbols
structure-truncated = Too many symbols — only some are shown
structure-too-large = The file is too large for the symbol index
structure-server-failed = The language server did not answer — showing the symbols read before
structure-action-file-structure = File Structure
structure-popup-placeholder = Filter by symbol name
structure-popup-empty = No matching symbols
# $count: rows not shown
structure-popup-more = { $count } more rows — type more to narrow
# Screen reader row name. $name: symbol name, $kind: kind
structure-row = { $name }, { $kind }
structure-kind-module = Module
structure-kind-class = Class
structure-kind-method = Method
structure-kind-property = Property
structure-kind-field = Field
structure-kind-constructor = Constructor
structure-kind-enum = Enum
structure-kind-interface = Interface
structure-kind-function = Function
structure-kind-variable = Variable
structure-kind-constant = Constant
structure-kind-value = Value
structure-kind-enum-member = Enum member
structure-kind-struct = Struct
structure-kind-event = Event
structure-kind-operator = Operator
structure-kind-type-parameter = Type parameter
structure-kind-trait = Trait
structure-kind-type-alias = Type alias
structure-kind-macro = Macro
structure-kind-impl = Implementation
structure-kind-symbol = Symbol
