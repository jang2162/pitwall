### Conflict resolution (apps/desktop/src/git/merge): the operation banner, the conflicts dialog and the 3-way merge document.

## Actions (shown in Find Action).

git-merge-action-resolve-conflicts = Resolve Conflicts…
git-merge-action-open = Open in Merge Tool
git-merge-action-continue = Continue Git Operation
git-merge-action-skip = Skip This Step of the Git Operation
git-merge-action-abort = Abort Git Operation

# The operation's name. $kind: merge · rebase · cherryPick · revert · bisect · am
git-merge-kind = { $kind ->
        [merge] Merge
        [rebase] Rebase
        [cherryPick] Cherry-pick
        [revert] Revert
        [bisect] Bisect
       *[other] Apply patches (am)
    }

## The operation banner (above the editor area · in the commit tool window).

git-merge-banner-label = Git operation in progress
# $kind: the git-merge-kind text
git-merge-banner-title = { $kind } in progress
# $branch: the branch being rebased, $onto: the target commit (short hash)
git-merge-banner-rebase = { $branch } → { $onto }
# $current · $total: steps
git-merge-banner-step = step { $current }/{ $total }
# $count: steps left after this one
git-merge-banner-remaining = { $count } left
# $count: conflicted files
git-merge-banner-conflicts = { $count ->
        [0] no conflicts
        [one] 1 conflicted file
       *[other] { $count } conflicted files
    }
git-merge-banner-resolve = Resolve Conflicts…
git-merge-banner-continue = Continue
git-merge-banner-skip = Skip
git-merge-banner-abort = Abort

## Abort confirmation (decision M2-D6 ① — aborting an operation).

# $kind: the git-merge-kind text
git-merge-abort-title = Abort the { $kind }?
git-merge-abort-body = HEAD and the work tree go back to where they were before the operation. Conflicts resolved during it are lost.
git-merge-abort-confirm = Abort

## Result notifications.

# $kind: the git-merge-kind text
git-merge-result-done = { $kind } finished
git-merge-result-stopped = The next step stopped on conflicts — resolve them and continue
git-merge-result-skipped = Skipped this step
git-merge-result-aborted = { $kind } aborted
# $action: what was being done (the action name)
git-merge-result-failed = { $action } failed

## Conflicts dialog.

git-merge-conflicts-title = Conflicted Files
git-merge-conflicts-loading = Reading conflicts…
git-merge-conflicts-empty = No conflicts
# $message: the error text
git-merge-conflicts-failed = Could not read the conflicts: { $message }
git-merge-conflicts-ours = Accept Yours
git-merge-conflicts-theirs = Accept Theirs
git-merge-conflicts-merge = Merge…
git-merge-conflicts-close = Close
git-merge-conflicts-changed = The file changed in the meantime and was left alone — the list was read again
# The conflict's kind (index stages). $kind: bothModified · bothAdded · addedByUs · addedByThem · deletedByUs · deletedByThem · bothDeleted
git-merge-conflict-kind = { $kind ->
        [bothModified] both modified
        [bothAdded] both added
        [addedByUs] added by us
        [addedByThem] added by them
        [deletedByUs] deleted by us
        [deletedByThem] deleted by them
       *[other] both deleted
    }
# Why Merge… is off. $reason: binary · large · deleted · submodule · symlink · other
git-merge-pick-only = { $reason ->
        [binary] Binary file — pick one side
        [large] Larger than 5 MiB — pick one side
        [deleted] Deleted on one side — pick one side (picking the deleted side deletes it)
        [submodule] Submodule — pick one side
        [symlink] Symbolic link — pick one side
       *[other] No automatic merge — pick one side
    }

## The 3-way merge document (an editor tab).

# $name: the file name
git-merge-title = { $name } (Merge)
git-merge-loading = Reading the conflict…
# $message: the error text
git-merge-failed = Could not read the conflict: { $message }
git-merge-gone = This file is no longer in conflict
git-merge-pane-ours = Yours (ours)
git-merge-pane-result = Result
git-merge-pane-theirs = Theirs
# $count: conflicts not picked yet
git-merge-unresolved = { $count ->
        [0] All conflicts picked
        [one] 1 conflict left
       *[other] { $count } conflicts left
    }
git-merge-clean = No conflict regions to pick — look over the three sides and save to resolve
git-merge-apply-non-conflicting = Apply All Non-Conflicting Changes
git-merge-compare-base = Compare with Base
git-merge-previous = Previous Conflict
git-merge-next = Next Conflict
git-merge-save = Save and Resolve
git-merge-apply-ours = Apply to result (≫)
git-merge-apply-theirs = Apply to result (≪)
git-merge-ignore = Ignore (✕)
git-merge-pick-ours = Resolve with Yours
git-merge-pick-theirs = Resolve with Theirs
git-merge-side-missing = This side has no file (picking it deletes the file)
# $count: conflicts not picked yet
git-merge-save-unresolved-title = { $count ->
        [one] 1 conflict is not picked yet
       *[other] { $count } conflicts are not picked yet
    }
git-merge-save-unresolved-body = Those places are saved as the result shows them now (empty if empty) and marked resolved.
git-merge-save-unresolved-confirm = Save Anyway
# $path: the path in the repository
git-merge-saved = Resolved the conflicts in { $path }
git-merge-save-conflict = The file changed in the meantime and was not saved — it was read again. The result is kept; saving again overwrites the file
git-merge-all-resolved = All conflicts are resolved — the operation can continue
