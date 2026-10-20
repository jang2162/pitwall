### Project window: tool window names and their open actions, status bar, background tasks, notifications, runtime environment, project trust.

## Tool window names. The id is the tool window id (PANELS in src/layout/project-layout.ts) in kebab-case.

tool-window-editor = Editor
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
editor-area-placeholder = Editor area

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
