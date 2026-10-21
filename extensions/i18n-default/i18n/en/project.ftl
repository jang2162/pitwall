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
editor-not-utf8 = This text file is not UTF-8 and cannot be shown yet
editor-read-only-large = Opened read-only: the file is larger than 5 MiB (highlighting and wrapping off)
# $kind: image | hex | markdown | text — the viewer kind for the file
editor-no-viewer =
    { $kind ->
        [image] Image preview is not available yet
        [hex] This is a binary file and cannot be shown yet
       *[other] There is no viewer for this file yet
    }
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
