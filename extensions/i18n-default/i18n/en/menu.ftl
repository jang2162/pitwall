### Native menu (apps/desktop/src-tauri/src/menu.rs). Rust draws these. $app is the app name.

menu-app = Pitwall
menu-file = File
menu-edit = Edit
menu-window = Window
menu-help = Help

menu-about = About { $app }
menu-settings = Settings…
menu-install-shell-command = Install Shell Command
menu-services = Services
menu-hide = Hide { $app }
menu-hide-others = Hide Others
menu-show-all = Show All
menu-quit = Quit { $app }

menu-welcome-window = Welcome Window
# ⌘W(키맵 액션 window.close): 에디터 탭이 있으면 탭, 없으면 창을 닫는다.
menu-close = Close
# ⌘S(키맵 액션 editor.save): 활성 탭의 문서를 저장한다.
menu-save = Save

menu-undo = Undo
menu-redo = Redo
menu-cut = Cut
menu-copy = Copy
menu-paste = Paste
menu-select-all = Select All

menu-minimize = Minimize
menu-maximize = Zoom
menu-fullscreen = Toggle Full Screen

menu-find-action = Find Action…
menu-copy-diagnostics = Copy Diagnostic Info
menu-check-updates = Check for Updates…

## "Install Shell Command" result dialog (apps/desktop/src-tauri/src/shell_command.rs). $target: where the command was installed.

shell-command-title = Install Shell Command
shell-command-installed = Installed the 'pitwall' command at { $target }.

    Run `pitwall <path>` in a terminal to open it in Pitwall.
shell-command-already = The 'pitwall' command at { $target } already opens this app.
# $previous: what the old link pointed to
shell-command-replaced = Installed the 'pitwall' command at { $target }.

    It replaced a link to { $previous }.
# $error: why it failed
shell-command-failed = Could not install the 'pitwall' command.

    { $error }

## Update dialogs (apps/desktop/src-tauri/src/update.rs). $app: the app name, $current: the running version, $version: the new version.

update-title = Software Update
update-available = { $app } { $version } is available. You have { $current }.

    Install it and restart { $app } now?
update-install = Install and Restart
update-later = Later
update-none = { $app } { $current } is the latest version.
# $error: why it failed
update-check-failed = Could not check for updates.

    { $error }
# $error: why it failed
update-install-failed = Could not install the update.

    { $error }
# The deb/rpm install found a newer release but cannot update itself.
update-package-manager = { $app } { $current } was installed with a system package (deb or rpm), so it does not update itself.

    Update it with your package manager, or download a new package from the release page.
# After installing an update, a window chose Cancel in its unsaved-edits prompt while every window was closing.
update-restart-cancelled = The update is installed, but { $app } did not restart because a window kept its unsaved edits.

    The new version starts the next time you open { $app }.
update-dev-build = Development builds do not check for updates.
