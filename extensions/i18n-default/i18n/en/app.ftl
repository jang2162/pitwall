### Strings shared by every window: actions, Find Action, theme and language switchers, keymap file errors.

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
