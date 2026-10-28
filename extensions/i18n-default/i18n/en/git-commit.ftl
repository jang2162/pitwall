### Commit tool window (⌘0, apps/desktop/src/git/commit): changed file groups, checkboxes, the commit message box, hook output,
### stashes, and hunk and line checkboxes in the diff preview (partial staging).

git-commit-title = Commit

## Toolbar

git-commit-refresh = Refresh
git-commit-rollback = Rollback
git-commit-show-diff = Show Diff

## Groups. A file checkbox stages the file (files in Staged are checked and unstage when clicked).

git-commit-group-conflicts = Conflicts
git-commit-group-staged = Staged
git-commit-group-changes = Changes
git-commit-group-untracked = Unversioned Files
git-commit-group-stash = Stashes
# $name: group name, $count: number of files
git-commit-group-count = { $name } ({ $count })
git-commit-partial = partly staged
git-commit-stage = Stage
git-commit-unstage = Unstage
git-commit-stage-all = Stage All
git-commit-unstage-all = Unstage All
git-commit-conflict-hint = Resolve conflicts in the merge tool
git-commit-no-changes = No changed files
git-commit-no-repository = No Git repository
git-commit-unsupported = Unsupported repository
git-commit-no-project = Only available in a project window

## Message box

git-commit-message = Commit Message
git-commit-history = Message History
git-commit-history-empty = No messages yet
git-commit-amend = Amend
git-commit-signoff = Sign-off
git-commit-run = Commit
git-commit-run-and-push = Commit and Push
git-commit-running = Committing…
git-commit-nothing-staged = Nothing is staged
git-commit-empty-message = The commit message is empty
git-commit-template-unedited = The commit template was not edited; write a message first

## Commit result and hook output

git-commit-output = Hook Output
git-commit-output-failed = Not committed — hook and git output
git-commit-output-close = Close
# $hash: the new commit (short hash)
git-commit-done = Committed { $hash }
# $previous: the commit before, $hash: the new commit (short hashes)
git-commit-amended = Amended { $previous } → { $hash }
git-commit-push-unavailable = The push dialog is not available yet; the commit was made
# $message: the error text
git-commit-failed = Failed: { $message }

## Rollback (discarding work tree changes cannot be undone)

git-commit-rollback-title = Rollback Changes
# $count: number of files
git-commit-rollback-body = { $count ->
        [one] Discard the changes to 1 file? Staged changes are unstaged and reset to HEAD. This cannot be undone.
       *[other] Discard the changes to { $count } files? Staged changes are unstaged and reset to HEAD. This cannot be undone.
    }
git-commit-rollback-confirm = Rollback
# $files: one file per line
git-commit-rollback-untracked = Unversioned files were not deleted: { $files }
git-commit-rollback-none = No changes are selected
# $count: number of further lines
git-commit-more = and { $count } more

## Hunk and line checkboxes in the diff preview

git-commit-hunk-stage = Stage this change
git-commit-hunk-unstage = Unstage this change
git-commit-line-stage = Stage this line
git-commit-line-unstage = Unstage this line
git-commit-hunk-revert = Revert this change
git-commit-hunk-revert-title = Revert Change
git-commit-hunk-revert-body = Discard this change from the work tree? This cannot be undone.
git-commit-hunk-changed = The file changed in the meantime; the diff was read again

## Stashes

git-commit-stash-create = Stash Changes…
git-commit-stash-include-untracked = Include untracked
git-commit-stash-name = Stash Message
git-commit-stash-hint = Leave empty for git's default message
git-commit-stash-apply = Apply
git-commit-stash-pop = Pop
git-commit-stash-drop = Drop
git-commit-stash-drop-title = Drop Stash
# $name: the stash (stash@{"{"}0{"}"} and its message)
git-commit-stash-drop-body = Drop { $name }? This cannot be undone.
git-commit-stash-nothing = There were no changes to stash
git-commit-stash-conflicted = Applied with conflicts; resolve them in the Conflicts group
git-commit-stash-changed = The stash list changed in the meantime; it was read again
git-commit-stash-empty = No stashes

## Actions (Find Action, keymap)

git-commit-action-open = Commit…
git-commit-action-run = Run Commit
git-commit-action-run-and-push = Commit and Push
git-commit-action-rollback = Rollback Changes
