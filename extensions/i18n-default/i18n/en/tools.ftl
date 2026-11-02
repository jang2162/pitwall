### Language server status (status bar `lsp` item, status popup, banner above the tab) and tool management (Settings > Tools, the license and source confirmation dialog, update notices) — 15-m4-plan M4-17.

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

# $tool: JDK, Node or Python, $needed: version range (>=21), $install: the mise command
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
