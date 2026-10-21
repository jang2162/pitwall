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
search-placeholder-all = Files, actions, projects
search-placeholder-files = File name, or name:line to go to the line
search-placeholder-projects = Project name
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
