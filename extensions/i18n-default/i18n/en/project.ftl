### Project window: tool window names and their open actions, editor tabs, status bar, background tasks, notifications, runtime environment, project trust, file watching.

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
# $path: the path that was being opened
editor-open-failed = Could not open { $path }
editor-copy-path-failed = Could not copy the path
editor-tab-pinned = Pinned
editor-tab-pin = Pin Tab
editor-tab-unpin = Unpin Tab
editor-switcher-label = Switcher

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

## File watching. The Host sends these as notifications (notification.show, crates/pw-fs/src/watch).

# $root: the watched root, $seconds: polling interval in seconds
fs-watch-limit = The file watch limit (fs.inotify.max_user_watches) was reached; some folders are checked every { $seconds } seconds
