### Strings shared by every window: actions, Find Action, theme and language switchers, keymap file errors, Open In.

# The language's own name. The language switcher lists every language by this message of that language.
language-name = English

window-title-project = Project
window-title-settings = Settings

action-find = Find Action
action-settings = Settings
action-cli-install = Install Shell Command
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

settings-appearance = Appearance
settings-theme = Theme
settings-language = Language
settings-language-system = System
# $error: why the setting could not be saved
settings-language-failed = Could not change the language: { $error }

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
# $program: the program path; $message: the OS error
open-with-problem-spawn-failed = { $program } could not start ({ $message })
# $code: the exit code, or "signal" when a signal ended it
open-with-problem-exited =
    { $code ->
        [signal] the command was ended by a signal
       *[other] the command exited with code { $code }
    }
