### File history (⌃⌥H, apps/desktop/src/git/history): the commits of a file across renames + the file's diff in the chosen commit.

git-history-action-show = Show File History
# Item in the project tree and editor tab context menus
git-history-menu = Show History
# Tab title. $name: file name
git-history-title = History of { $name }
git-history-list = File history
git-history-loading = Loading…
git-history-empty = No commit changed this file
# $message: the Host's error text
git-history-failed = Could not read the history: { $message }
# $count: number of commits
git-history-count = { $count ->
        [one] { $count } commit
       *[other] { $count } commits
    }
git-history-pick = Choose a commit to see this file's diff in it

## Rename points. The commit row shows old name → new name.

# $from: old path, $to: new path
git-history-renamed = Renamed { $from } → { $to }
# $from: source path, $to: new path
git-history-copied = Copied { $from } → { $to }
# $path: the path in that commit (from the repository top)
git-history-path-at = Path in this commit: { $path }

## Change kinds (screen readers read these instead of the letter).

git-history-change-added = Added
git-history-change-modified = Modified
git-history-change-deleted = Deleted
git-history-change-renamed = Renamed
git-history-change-copied = Copied
git-history-change-type-changed = Type changed

## Context menu of a commit row.

git-history-menu-reveal = Select in Git Log
git-history-menu-show-commit = Show Commit Details
git-history-menu-open-diff = Open This Diff in a Tab
git-history-menu-open-revision = Open File at This Revision
git-history-menu-copy-hash = Copy Hash
git-history-copied-hash = Copied the hash

## When the file history cannot open (notifications).

git-history-no-file = No file to show the history of
git-history-no-file-body = Open a file in the editor or choose one in the project tree
git-history-outside = The file is outside the repository
git-history-folder = Folders have no history view — choose a file
# $submodule: the submodule's path (from the repository top)
git-history-submodule = The file is inside the submodule { $submodule } — its history is in that submodule's repository
# $repo: that repository's folder (from the tree root)
git-history-nested = { $repo } is a separate repository (submodule or nested repository) — the history of files inside it is in that repository
# $message: the error text
git-history-open-failed = Could not open the file history: { $message }
