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
welcome-list-remote = remote

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
