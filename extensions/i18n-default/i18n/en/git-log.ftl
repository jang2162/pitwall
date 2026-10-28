### Git log tool window (⌘9, apps/desktop/src/git/log): branch panel, filter bar, graph list, commit details, context menu, confirmations.

git-log-list = Commit log

## Branch panel. A click filters to that branch (⌘ for several).

git-log-branches = Branches
git-log-branches-head = HEAD
git-log-branches-local = Local
git-log-branches-remote = Remote
git-log-branches-tags = Tags
git-log-branches-all = All branches

## Filter bar. The text field searches a hash prefix when it is 7 or more hex digits and a commit starts with them, otherwise the message.

git-log-filter-text = Text or hash
git-log-filter-regex = Regex
git-log-filter-match-case = Match case
git-log-filter-branch = Branch
# $count: number of chosen branches
git-log-filter-branch-count = { $count ->
        [one] 1 branch
       *[other] { $count } branches
    }
git-log-filter-author = Author
git-log-filter-date = Date
git-log-filter-since = From
git-log-filter-until = To
git-log-filter-path = Path
git-log-filter-path-hint = Paths from the repository root, comma separated (quote a path with a comma)
git-log-filter-clear = Clear filters
git-log-filter-no-graph = Filters are on: matching commits only, without the graph

## Status line of the list.

git-log-loading = Loading…
# $count: number of commits
git-log-count = { $count ->
        [one] 1 commit
       *[other] { $count } commits
    }
# $count: number of matching commits
git-log-matched = { $count ->
        [one] 1 matching commit
       *[other] { $count } matching commits
    }
# $count: number of commits found by hash prefix
git-log-matched-hash = { $count ->
        [one] 1 commit by hash prefix
       *[other] { $count } commits by hash prefix
    }
# $message: the Host's error text
git-log-failed = Could not read the log: { $message }
git-log-empty = No commits
git-log-no-project = Only available in a project window

## Commit details on the right.

git-log-detail-none = Select a commit to see its details
# $count: number of changed files
git-log-detail-files = Changed files ({ $count })
git-log-detail-files-loading = Reading changed files…
git-log-detail-parents = Parents
git-log-detail-root = None (root commit)
git-log-detail-author = Author
git-log-detail-committer = Committer
git-log-detail-contained-in = Contained in
git-log-detail-contained-loading = Searching…
git-log-detail-contained-none = None
git-log-detail-generated = generated
# $a, $b: short hashes (older, newer)
git-log-detail-compare = Comparing { $a } ↔ { $b }
git-log-detail-open-compare = Open comparison in a tab
git-log-detail-open-commit = Open commit details in a tab
# $count: number of selected commits
git-log-detail-many = { $count } commits selected
# $message: the Host's error text
git-log-detail-failed = Could not read the details: { $message }

## Context menu and the label drag-and-drop menu.

# $name: branch name
git-log-menu-checkout-branch = Check Out { $name }
git-log-menu-checkout-revision = Check Out This Commit (Detached HEAD)
git-log-menu-new-branch = New Branch…
git-log-menu-new-tag = New Tag…
# $count: number of selected commits
git-log-menu-cherry-pick = { $count ->
        [one] Cherry-Pick
       *[other] Cherry-Pick { $count } Commits
    }
git-log-menu-revert = { $count ->
        [one] Revert Commit
       *[other] Revert { $count } Commits
    }
git-log-menu-reset = Reset to This Commit
# $mode: soft, mixed, hard or keep
git-log-menu-reset-mode = Reset to This Commit --{ $mode }
git-log-menu-copy-hash = Copy Hash
git-log-menu-show-commit = Open Commit Details in a Tab
git-log-menu-compare = Compare the Two Commits in a Tab
# $source: the dragged label, $target: where it was dropped
git-log-menu-merge = Merge { $source } into { $target }
# $target: a drop target that is not the current branch
git-log-menu-merge-needs-checkout = Check out { $target } first to merge into it
git-log-menu-rebase = Rebase { $source } onto { $target }
# $a, $b: the two sides
git-log-menu-compare-refs = Compare { $a } with { $b }

## Confirmations.

git-log-dialog-new-tag = New Tag
# $rev: short hash
git-log-dialog-from = From { $rev }
git-log-dialog-name-empty = Enter a name
git-log-dialog-name-invalid = This name cannot be used

## Results of writes.

# $ref: the moved ref, $from and $to: short hashes
git-log-result-moved = { $ref }: { $from } → { $to }
git-log-result-undo = Undo
git-log-result-dismiss = Dismiss
git-log-result-stopped = Stopped on a conflict
git-log-result-stopped-body = Resolve the conflicts, then continue or abort (Commit tool window)
git-log-result-up-to-date = Nothing changed
# $action: the menu item
git-log-result-failed = { $action } failed
git-log-result-copied = Copied the hash

## When another screen selects a commit in the log (action git.log.reveal — blame annotations, file history).

git-log-reveal-action = Select Commit in Git Log
git-log-reveal-cleared = The commit is not in the current filter, so the filter was cleared
git-log-reveal-cleared-body = Cleared the branch, text, author, date and path filters and selected the commit
git-log-reveal-missing = The commit is not in the Git log
# $rev: short hash
git-log-reveal-missing-body = { $rev } is not reachable from any branch, tag or HEAD
# $message: the Host's error text
git-log-reveal-failed = Could not find the commit: { $message }
