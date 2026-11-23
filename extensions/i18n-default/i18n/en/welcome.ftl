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
welcome-remote-stage-ssh = SSH
welcome-remote-stage-install = Install
welcome-remote-stage-daemon = Daemon
welcome-remote-stage-handshake = Handshake
welcome-remote-status-idle = Disconnected
welcome-remote-status-by-user = Disconnected by you
welcome-remote-status-connecting = Connecting — { $stage }
welcome-remote-status-connected = Connected
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
