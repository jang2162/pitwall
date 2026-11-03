### Strings shared by every window: actions, Find Action and Search Everywhere, theme and language switchers, keymap file errors, Open In.

# The language's own name. The language switcher lists every language by this message of that language.
language-name = English

window-title-project = Project
window-title-settings = Settings

action-find = Find Action
action-settings = Settings
action-cli-install = Install Shell Command
action-window-close = Close Window
# $theme: light | dark | system
action-theme =
    { $theme ->
        [light] Theme: Light
        [dark] Theme: Dark
       *[system] Theme: System
    }
# $name: the language's own name (language-name of that language)
action-language = Language: { $name }
action-language-system = Language: System

find-action-label = Find Action
find-action-placeholder = Action name
find-action-empty = No matching actions
# $reason: why the OS takes the key
find-action-reserved = Reserved by the OS ({ $reason }), the app never receives it

theme-switcher-label = Theme
theme-system = System Theme
theme-light = Light Theme
theme-dark = Dark Theme

# $error: what was wrong with keymap.toml
keymap-file-error = Keymap file error, using the previous keymap: { $error }
# $warnings: the warnings joined with "; "
keymap-file-warning = Keymap file warning: { $warnings }


## Search Everywhere (Shift Shift) and its tabs: Go to File, Recent Files, Project Switcher. src/search-everywhere.

action-search-everywhere = Search Everywhere
action-goto-file = Go to File
action-recent-files = Recent Files
action-project-switcher = Switch Project
search-everywhere-label = Search Everywhere
search-recent-files-label = Recent Files
search-tab-all = All
search-tab-files = Files
search-tab-actions = Actions
search-tab-projects = Projects
search-tab-branches = Branches
search-tab-db-objects = DB Objects
search-placeholder-all = Files, symbols, actions, projects, branches, DB objects
search-placeholder-files = File name, or name:line to go to the line
search-placeholder-projects = Project name
search-placeholder-branches = Branch or tag name
search-placeholder-db-objects = Table or view name (only those the explorer has read)
search-placeholder-recent-files = File name
search-empty = Nothing found
search-more = More
search-no-recent-files = No files opened in this window yet
search-files-searching = Searching…
search-files-busy = File search is busy, searching again shortly
# $message: why it failed
search-failed = Search failed: { $message }
# $shown: how many are shown, $matched: how many matched
search-files-truncated = { $shown } of { $matched }
# $line: the line to go to (from 1)
search-files-at-line = line { $line }
search-recent-badge = recent
search-current-project = this window
search-db-table = table
search-db-view = view
# $key: ⌘↩ or Ctrl+Enter
search-db-open-data = { $key } data
search-db-none = No cached names yet. Expand a connection in the Database Explorer and its names show up here
search-db-untrusted = DB objects are hidden in a project you have not trusted
search-db-truncated = Too many cached names, searching only the first part

## Go to Symbol (Cmd+Alt+O), Go to Class (Cmd+O) and the Symbols tab of Search Everywhere (M4-16). src/search-everywhere/symbol-tab.tsx.

action-goto-symbol = Go to Symbol
action-goto-class = Go to Class
search-tab-symbols = Symbols
search-classes-label = Go to Class
search-placeholder-symbols = Function, type or method name
search-placeholder-classes = Class, struct, interface or enum name
search-symbols-busy = The symbol index is busy, searching again shortly
search-symbols-waiting-server = Waiting for the language server…
search-symbols-suspended = The file list was cancelled, so no symbols were found
search-symbols-rebuild = Rebuild
search-symbols-outside = external
# $line: the line (from 1)
search-symbols-at = line { $line }
search-symbol-kind-module = module
search-symbol-kind-class = class
search-symbol-kind-struct = struct
search-symbol-kind-interface = interface
search-symbol-kind-trait = trait
search-symbol-kind-enum = enum
search-symbol-kind-enumMember = enum member
search-symbol-kind-typeAlias = type alias
search-symbol-kind-function = function
search-symbol-kind-method = method
search-symbol-kind-constructor = constructor
search-symbol-kind-field = field
search-symbol-kind-property = property
search-symbol-kind-constant = constant
search-symbol-kind-variable = variable
search-symbol-kind-macro = macro
search-symbol-kind-impl = impl

## Settings window: Appearance, and "External Tools > Open Pitwall from Orca" (docs/design/04-features.md F8-6). The keys are mapped in apps/desktop/src/settings/strings.ts.

settings-appearance = Appearance
settings-theme = Theme
settings-language = Language
settings-language-system = System
# $error: why the setting could not be saved
settings-language-failed = Could not change the language: { $error }

settings-external-tools = External Tools
settings-orca-title = Open Pitwall from Orca
settings-orca-intro = In Orca, add an app under Settings → General → Open in menu with the values below. Orca runs the command with the absolute path of a worktree folder or a file appended.
settings-orca-label = Label
settings-orca-command = Command
settings-orca-copy = Copy
# $value: the value to copy
settings-orca-copy-value = Copy { $value }
settings-orca-copied = Copied
# $error: why the clipboard could not be written
settings-orca-copy-failed = Could not copy: { $error }
settings-orca-checking = Checking…
# $error: why the check failed
settings-orca-check-failed = Could not check: { $error }
settings-orca-recheck = Check Again
settings-orca-installed = Installed
settings-orca-not-installed = Not installed
settings-orca-other-link = Points elsewhere
settings-orca-not-a-link = Cannot install
settings-orca-on-path = On PATH
settings-orca-not-on-path = Not on PATH
settings-orca-other-on-path = A different pitwall on PATH
# Not installed at the install location, but the pitwall on PATH is already this app
settings-orca-works-from-path = On PATH (outside the install location)
# $target: the install location (/usr/local/bin/pitwall)
settings-orca-detail-installed = { $target } points to this app.
settings-orca-detail-missing = There is no shell command at { $target }.
# $points: where the link points
settings-orca-detail-other-link = { $target } points elsewhere: { $points }
settings-orca-detail-not-a-link = { $target } is a file Pitwall did not create. Install Shell Command does not replace it — remove it yourself, then check again.
# $path: the pitwall found on PATH
settings-orca-detail-works-from-path = { $path } runs this app, and that is what Orca runs, so the shell command at { $target } is not needed.
# $path: the pitwall found on PATH
settings-orca-detail-other-on-path = The first one on PATH is { $path }. Orca runs that one.
# $resolved: that path with its symlinks resolved
settings-orca-detail-resolved = Actual file: { $resolved }
settings-orca-install = Install Shell Command
settings-orca-installing = Installing…
# $app: this app's binary, $target: the install location
settings-orca-source-development-build = This is a development build ({ $app }), so it is not installed at { $target }: your pitwall command would point at this build and break once it is deleted. To try installing, start the app with PITWALL_SHELL_COMMAND_TARGET set to an absolute path in a temporary folder.
# $app: this app's binary
settings-orca-source-transient = Pitwall is running from { $app }, which goes away when the disk image is ejected or the Mac restarts. Move Pitwall to the Applications folder, open it from there, and install.
# $target: the install location
settings-orca-absolute = Orca cannot find this app on PATH. Enter the absolute path { $target } as the command.
# $reason: why the login shell environment could not be read
settings-orca-path-fallback = Could not read the login shell environment, so this used the app's PATH — it may differ from the PATH Orca sees ({ $reason }).
settings-orca-worktree = Worktrees Orca creates do not have the uncommitted .pitwall/*.local.toml files; Pitwall then reads the ones in the main repository. To copy them into worktrees as well, list literal paths such as .pitwall/project.local.toml in .worktreeinclude at the repository root (Orca does not accept globs).

## Settings window, DB page (M3-20). The host judges the ranges.
settings-db-title = Database
settings-db-intro = The [db] table of the global settings.toml. A changed value applies from the next run and the next connection.
settings-db-loading = Loading…
settings-db-failed = Could not change the setting: { $error }
settings-db-grid-max-rows = Grid row limit
settings-db-grid-max-bytes = Grid byte limit
settings-db-cell-max-bytes = Cell limit
settings-db-redis-response-timeout = Redis response timeout
settings-db-redis-key-separator = Redis key separator (initial)
settings-db-connect-timeout = Default connect timeout
settings-db-unit-rows = rows
settings-db-unit-bytes = bytes
settings-db-unit-seconds = seconds
settings-db-apply = Apply
settings-db-reset = Reset to Default
settings-db-default = default { $value }
settings-db-range = range { $min } to { $max } { $unit }
settings-db-now = now { $value }
settings-db-not-integer = Must be an integer
settings-db-out-of-range = Must be an integer from { $min } to { $max }
settings-db-not-text = Must be text
settings-db-bad-separator = Must be 1 to { $max } characters without spaces or control characters
settings-db-file-refused = Not using { $key } from settings.toml, using the default: { $reason }

## Open In (⌥F1) and Open in Default External Tool (⌘⌥E).

action-open-with = Open In…
action-open-with-default = Open in Default External Tool

open-with-label = Open In
# $project: the project name
open-with-title = Open { $project } in
open-with-placeholder = Tool name
open-with-empty = No matching tools
open-with-loading = Loading tools…
open-with-default-badge = default
# $reason: one of the open-with-problem-* messages
open-with-unavailable = Not usable: { $reason }
open-with-no-default = No default tool is set. Pick one here, or set [open_with] default in settings.toml.
# $tool: the tool id the settings name
open-with-unknown-default = The default tool “{ $tool }” is not available on this system. Pick one here.
# $tool: the tool's label; $reason: one of the open-with-problem-* messages
open-with-failed = Could not open with { $tool }: { $reason }
# $command: the command line that was run, for trying it in a terminal
open-with-command = Command: { $command }
open-with-dismiss = Dismiss
open-with-default-tool = the default external tool
open-with-list-failed = Could not read the external tools: { $error }
open-with-problem-unknown-tool = the tool is not available on this system
# $message: what is wrong in settings.toml
open-with-problem-invalid-tool = its settings are invalid ({ $message })
# $variable: the template variable without braces, e.g. branch
open-with-problem-missing-variable = the command uses {"{"}{ $variable }{"}"}, which has no value here
# $path: the project folder
open-with-problem-project-root-missing = the project folder { $path } does not exist
# $program: the program name; $source: login | process (which PATH was searched)
open-with-problem-not-found =
    { $source ->
        [login] “{ $program }” is not on the login shell PATH
       *[process] “{ $program }” is not on the app's PATH (the login shell PATH could not be read)
    }
# $path: the bundle folder
open-with-problem-bundle-path = { $path } is an app bundle, which “open” would run instead of showing — not run
# $program: the program path; $message: the OS error
open-with-problem-spawn-failed = { $program } could not start ({ $message })
# $code: the exit code, or "signal" when a signal ended it
open-with-problem-exited =
    { $code ->
        [signal] the command was ended by a signal
       *[other] the command exited with code { $code }
    }

## Language server status (status bar `lsp` item, status popup, banner above the tab) and tool management (Settings > Tools, the license and source confirmation dialog, update notices) — 15-m4-plan M4-17.

## Status bar item. Each state has its own words (never colour alone — 10-ui §6).

lsp-status-starting = Starting
lsp-status-ready = Ready
# $title: the server's progress title, $percent: 0 to 100
lsp-status-progress = { $title } { $percent }%
lsp-status-progress-unknown = { $title }…
lsp-status-error = Error
lsp-status-crashed = Error — stopped
lsp-status-restart-needed = Restart needed
lsp-status-restart-suggested = Restart suggested
lsp-status-not-installed = Not installed
lsp-status-runtime-missing = Runtime missing
lsp-status-untrusted = Untrusted
lsp-status-disabled = Off
lsp-status-stopped = Stopped
# $count: other servers serving the same document
lsp-status-more = +{ $count }
# $server: server id
lsp-status-hint = Language server { $server } — click for its status

## Status popup

lsp-status-popup-title = Language servers
lsp-status-root = Root
lsp-status-version = Version
lsp-status-found = Found in
lsp-status-found-settings = Configured path
lsp-status-found-project-local = Inside the project
lsp-status-found-path = PATH of the runtime environment
lsp-status-found-installed = Installed by Pitwall
lsp-status-unknown = —
lsp-status-restart = Restart
lsp-status-stop = Stop
lsp-status-log = Open Log
lsp-status-disable = Turn Off in This Project
lsp-status-enable = Turn On in This Project
lsp-status-ask-again = Offer the Install Again
lsp-status-install = Install…
lsp-status-close = Close
# $file: the personal settings file written
lsp-status-disabled-note = Written to { $file } — personal settings that are not committed
lsp-status-action-failed = Language server action failed
# $server: server id
lsp-status-log-title = { $server } log
lsp-status-log-empty = No log yet
lsp-status-log-truncated = The beginning is left out (last 256 KiB)
# $message: why it could not start
lsp-status-failed-detail = Could not start: { $message }
lsp-status-crashed-detail = It died five times within three minutes — Restart starts it again

## Why a server cannot start (banner and popup)

# $tool: JDK, Node or Python, $needed: version range (>=25), $install: the mise command
lsp-runtime-missing = The server needs { $tool } { $needed } — `{ $install }` (the project's pinned version is not changed)
lsp-untrusted = Trust the project to turn on language servers
lsp-trust = Trust…

## Banner above the tab

# $language: language name, $server: server name, $license: license (SPDX)
lsp-banner-not-installed = The { $language } language server { $server } ({ $license }) is not installed
lsp-banner-not-installed-plain = The { $language } language server { $server } is not installed
lsp-banner-install = Install
lsp-banner-settings = Settings
lsp-banner-dont-ask = Don't Ask in This Project
# $server: server name
lsp-banner-installing = Installing { $server }…
lsp-banner-failed = Could not install: { $message }

## License and source confirmation (first time only — decision M4-D2)

# $name: package, $version: version
tools-confirm-title = Install { $name } { $version }
tools-confirm-update-title = Update { $name } to { $version }
tools-confirm-intro = This package is downloaded for the first time. Check where it comes from and its license.
tools-confirm-changed = The version, license or source changed while you were confirming. Check the new values.
tools-confirm-description = Description
tools-confirm-version = Version
tools-confirm-license = License
tools-confirm-source = Source
tools-confirm-integrity = Integrity
tools-confirm-integrity-pinned = Checked against a SHA-256 pinned by Pitwall
tools-confirm-integrity-none = No checksum — there is no pinned hash to check the download against
tools-confirm-size = Download size
tools-confirm-size-unknown = Unknown
tools-confirm-note = Note
tools-confirm-homepage = Homepage
tools-confirm-cancel = Cancel
tools-confirm-install = Install
tools-confirm-update = Update

## Settings > Tools

tools-title = Tools
tools-intro = Pitwall installs language servers and formatters into its data folder (tools) from the Mason registry. The first download of a package asks you to confirm its license and source.
tools-loading = Loading…
# $error: the error text
tools-load-failed = Could not load: { $error }
tools-installed = Installed tools
tools-installed-empty = No tools installed
tools-col-name = Name
tools-col-version = Version
tools-col-language = Language
tools-col-category = Category
tools-col-license = License
tools-col-size = Size
tools-col-checksum = Checksum
tools-col-servers = Used by
tools-col-actions = Actions
tools-checksum-pinned = Pinned hash
tools-checksum-none = No checksum
# $version: previous version
tools-previous = Previous { $version }
# $problem: why it cannot be used
tools-problem = Reinstall needed: { $problem }
# $version: version to get
tools-update-to = Update to { $version }
tools-rollback = Roll Back
tools-uninstall = Uninstall
tools-log = Install Log
tools-log-empty = No install log
tools-log-close = Close
tools-search = Search the registry
tools-search-placeholder = Name or description
tools-filter-category = Category
tools-filter-language = Language
tools-filter-all = All
tools-install = Install
tools-installed-badge = Installed
tools-bundled = Default bundle
tools-deprecated = No longer maintained
# $reason: why it is not supported
tools-unsupported = Cannot be installed on this machine ({ $reason })
# $count: results shown
tools-truncated = Too many results; showing { $count }
tools-search-empty = No matching packages
tools-registry = Registry
# $version: current version, $pinned: pinned version
tools-registry-version = Version { $version } · pinned { $pinned }
# $time: when it was fetched
tools-registry-checked = Last checked { $time }
tools-registry-snapshot = Built-in snapshot (never fetched)
tools-registry-latest = Use the latest registry (checked once a day)
tools-registry-fetching = Fetching…
# $error: the error text
tools-registry-error = Last fetch failed: { $error }
# $name: package
tools-busy = Working on { $name }…
# $name: package, $message: the error text
tools-failed = { $name }: { $message }
tools-open-failed = Could not open { $url }: { $message }
# $name: package, $version: version
tools-done = { $name } { $version }
# $runtime: node or python
tools-runtime-missing = { $runtime } is needed — install it with mise (mise use -g { $runtime })

## Update notice and action

action-tools-manage = Open Tool Management
# $count: tools with an update
tools-updates-title =
    { $count ->
        [one] 1 tool update
       *[other] { $count } tool updates
    }
