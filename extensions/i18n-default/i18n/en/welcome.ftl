### Welcome window (S1 · S2 · S3, docs/design/10-ui.md §2.1–2.3). The keys are mapped in apps/desktop/src/welcome/strings.ts.

welcome-nav-label = Welcome
welcome-nav-projects = Projects
welcome-nav-remote-hosts = Remote Hosts
welcome-nav-extensions = Extensions
# Tooltip on a tab that does not exist yet.
welcome-nav-not-yet = Not available yet

welcome-list-label = Project list
welcome-list-search = Search projects
welcome-list-empty = No projects yet. Create a new project or open a folder.
welcome-list-no-match = No matching projects
welcome-list-favorite = Favorite
# 행의 현재 브랜치(스크린 리더 이름 · 툴팁). git 을 실행하지 않고 HEAD 를 읽는다(git.projectBranches)
welcome-list-branch = Branch { $name }

welcome-button-new-project = New Project…
welcome-button-open-folder = Open Folder…

welcome-menu-open = Open
# $platform: mac | win | linux
welcome-menu-reveal =
    { $platform ->
        [mac] Reveal in Finder
        [win] Show in Explorer
       *[other] Show in File Manager
    }
welcome-menu-remove = Remove from List

welcome-action-new-project = New Project
welcome-action-open-folder = Open Folder
welcome-action-open = Open Selected Project
# $platform: mac | win | linux
welcome-action-reveal =
    { $platform ->
        [mac] Reveal Selected Project in Finder
        [win] Show Selected Project in Explorer
       *[other] Show Selected Project in File Manager
    }
welcome-action-remove = Remove Selected Project from List
welcome-action-create = Create Project

welcome-drop = Drop a folder to create a new project

# $reason: why the registry file was replaced, $backup: path of the old file
welcome-storage-recovered = The project list file was damaged and has been recreated ({ $reason }). Old file: { $backup }
welcome-storage-read-only = The project list is read-only and cannot be changed ({ $reason })
welcome-storage-memory = Could not open the project list file. The list looks empty and cannot be changed ({ $reason })

welcome-find-label = Find Project
# $path: the file or folder being opened
welcome-find-title = Choose a project for { $path }
welcome-find-search = Folder path
welcome-find-empty = No matching candidates
welcome-find-loading = Looking for candidates…
welcome-find-recommended = recommended
welcome-find-worktree-main = main repository of the worktree
welcome-find-pick-folder = Choose Folder…
welcome-find-file-only = Open the file without a project

# $name: the project name being created
welcome-create-title = New Project: { $name }
welcome-create-name = Name
welcome-create-root = Root
welcome-create-change-root = Change
welcome-create-choose-root = Choose…
welcome-create-root-none = Not chosen
welcome-create-config-mode = Save settings
welcome-create-repository = In .pitwall/ (committed)
welcome-create-personal = Personal only
welcome-create-detected = Detected
welcome-create-detected-empty = Nothing detected
welcome-create-cancel = Cancel
welcome-create-submit = Create

# $path: the file the user tried to open
welcome-notice-file-not-in-project = The new project does not contain { $path }, so only the project window was opened

# $what: one of the welcome-error-* labels below, $message: the error text
welcome-error-action = { $what } failed: { $message }
welcome-error-open = Open project
welcome-error-remove = Remove from list
welcome-error-pick-folder = Choose folder
# $platform: mac | win | linux
welcome-error-reveal =
    { $platform ->
        [mac] Reveal in Finder
        [win] Show in Explorer
       *[other] Show in file manager
    }
welcome-error-create = Create project
welcome-error-find-project = Read the find-project request
welcome-error-open-file = Open the file
welcome-error-candidates = Find root candidates

# Remote hosts tab (S8 — 18-m7-plan M7-08).
welcome-remote-label = Remote hosts
welcome-remote-empty = No remote hosts. Add a Host to ~/.ssh/config or [[remote_hosts]] to the settings (settings.toml).
welcome-remote-add-hint = Remote hosts come from Host lines in ~/.ssh/config and [[remote_hosts]] in the settings — pick one, then Connect.
welcome-remote-source-settings = settings
welcome-remote-source-ssh-config = ssh config
welcome-remote-connect = Connect
welcome-remote-disconnect = Disconnect
welcome-remote-retry = Retry
welcome-remote-show-log = Connection log
welcome-remote-hide-log = Hide log
welcome-remote-trust = Trust this new machine
welcome-remote-stages = Connection stages
welcome-remote-stage-direct = Direct
welcome-remote-stage-ssh = SSH
welcome-remote-stage-install = Install
welcome-remote-stage-daemon = Daemon
welcome-remote-stage-handshake = Handshake
welcome-remote-status-idle = Disconnected
welcome-remote-status-by-user = Disconnected by you
welcome-remote-status-connecting = Connecting — { $stage }
welcome-remote-status-connected = Connected
welcome-remote-status-resuming = Briefly cut off — resuming the link
welcome-remote-status-reconnecting = Reconnecting in { $seconds } s (attempt { $attempt })
welcome-remote-status-machine-changed = Another machine answered — remembered { $expected }, now { $found }. This is like a changed SSH host key — trust it only if it is the right machine.
welcome-remote-failed = Failed at { $stage }
welcome-remote-failure-host-key-changed = The host key changed and ssh refused. Fix known_hosts yourself.
welcome-remote-failure-host-key-not-accepted = The host key was not accepted.
welcome-remote-failure-ssh = ssh could not connect (network or sign-in).
welcome-remote-failure-auth = ssh could not sign in (prompt cancelled, wrong answer or no accepted key) — Retry asks again.
welcome-remote-failure-unsupported = The remote platform is not supported — nothing was uploaded.
welcome-remote-failure-install = Installing or pairing the relay failed.
welcome-remote-failure-version-mismatch = The relay's version differs from this app.
welcome-remote-failure-rejected = The relay did not accept this device.
welcome-remote-failure-link = The link did not come up or dropped.
welcome-remote-failure-already-connected = This machine is already connected under another host name.
welcome-remote-failure-unavailable = Remote hosts are not available.
welcome-remote-last-connected = Last: connected ({ $when })
welcome-remote-last-disconnected = Last: disconnected ({ $when })
welcome-remote-last-failed = Last: failed ({ $when })
welcome-remote-master = SSH master connection open
welcome-remote-log-title = { $name } connection log
welcome-remote-log-empty = Nothing logged yet
# Under the disconnect button while reconnecting (M7-09 review, recommendation 7)
welcome-remote-disconnect-holds = Even if you disconnect, the relay keeps that machine's runs and terminals for its resume window (2 minutes).
welcome-remote-old-title = Old relay versions
# $version: the old version, $runs: runs left, $terminals: terminals left
welcome-remote-old-left = The old relay { $version } still has { $runs } runs and { $terminals } terminals.
# $links: other Desktop connections on that old version
welcome-remote-old-links = { $links } other Desktop connections use this old version — stopping it cuts them too.
welcome-remote-old-stop = Stop the old version
# $links: connections cut with it
welcome-remote-old-stop-links = Stop the old version (cuts { $links } connections)
# $version: the old version that does not answer
welcome-remote-old-silent = The old relay { $version } does not answer — not stopping it.
welcome-remote-old-stopped = Stopped the old relay.
welcome-remote-old-stopping = Sent the stop — it is still ending its runs.
welcome-remote-old-changed = It changed after you looked, so it was not stopped — check the new counts and choose again.
welcome-remote-old-gone = That old relay had already ended.
welcome-remote-old-no-answer = That old relay does not answer, so it was not stopped.
welcome-remote-command = Use the pitwall command on the remote (~/.local/bin/pitwall)
welcome-remote-command-other = ~/.local/bin/pitwall is another file, so it is left alone.

## Device section (18-m7-plan M7-23 ③) — the relay's pairing table
welcome-remote-devices-show = Devices
welcome-remote-devices-hide = Hide devices
welcome-remote-devices-title = Devices paired with this relay
welcome-remote-devices-unreadable = Could not read the device list.
welcome-remote-devices-unnamed = (no name)
welcome-remote-devices-you = This Desktop
welcome-remote-devices-role-owner = owner — everything
welcome-remote-devices-role-mobile = mobile — view and allowed actions
welcome-remote-devices-role-readonly = readonly — view only
# $when: when the device was paired
welcome-remote-devices-paired-at = Paired { $when }
# $when: when the device last connected
welcome-remote-devices-last-seen = Last seen { $when }
welcome-remote-devices-unknown = unknown
welcome-remote-devices-never = never
welcome-remote-devices-unpair = Unpair
welcome-remote-devices-unpair-confirm = Unpair it
welcome-remote-devices-cancel = Cancel
welcome-remote-devices-confirm-title = Confirm unpairing
# $name: the device to unpair
welcome-remote-devices-confirm = Unpairing { $name } cuts its connections and terminals at once, and it cannot connect again.
welcome-remote-devices-confirm-self = Unpairing this Desktop cuts this connection at once. To come back, pair again over SSH.
welcome-remote-devices-confirm-last-owner = This Desktop is the last owner device of this relay — after unpairing, no device manages it, and the only way back is pairing again over SSH.
welcome-remote-devices-code-role = Role of the new device
welcome-remote-devices-show-code = Show a code
# $until: when the code ends
welcome-remote-devices-code-hint = Enter this code on the new device — until { $until }, three tries, one use. Showing a new code ends this one.

## Direct links (18-m7-plan M7-24) — QUIC + Noise, code pairing, search, resident relay
welcome-remote-via-direct = Direct (QUIC)
welcome-remote-via-ssh = SSH
welcome-remote-resident = Keep the relay running on this machine (starts with the login — off, it lives only while started over SSH)
welcome-remote-devices-wide = This relay accepts direct links on every interface, public addresses included — relay.toml [listen] interfaces has "all".
# $addresses: the listening addresses
welcome-remote-devices-listening = Direct links on: { $addresses }
welcome-remote-devices-not-listening = No direct links (SSH only).
welcome-remote-direct-open = Add a relay with a code…
welcome-remote-direct-title = Add a relay with a code
welcome-remote-direct-hint = Enter the 6-digit code shown by `pitwall-relay pair` on the remote machine (or by the device section of a connected Desktop). Pick the address from the search or type it.
welcome-remote-direct-search = Search
welcome-remote-direct-searching = Searching…
welcome-remote-direct-source-tailscale = Tailscale
welcome-remote-direct-source-lan = LAN
welcome-remote-direct-listening = Answers (pairing checks who it is)
welcome-remote-direct-not-listening = No answer
welcome-remote-direct-nothing = Nothing found — type the address.
welcome-remote-direct-address = Address
welcome-remote-direct-port = Port
welcome-remote-direct-code = Code (6 digits)
welcome-remote-direct-nickname = Name for this host
welcome-remote-direct-pair = Pair
welcome-remote-direct-cancel = Close
# $name: the added host name
welcome-remote-direct-paired = Added { $name } — it connects directly to that machine only.
