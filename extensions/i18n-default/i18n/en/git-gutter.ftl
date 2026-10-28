### Git change markers in the editor gutter, the inline diff popup, and blame (apps/desktop/src/git/gutter).

## Actions (shown in Find Action).

git-gutter-action-next-change = Next Change
git-gutter-action-previous-change = Previous Change
git-gutter-action-toggle-blame = Toggle Git Blame Annotations
git-gutter-action-blame-revision = Open a Revision of a File with Blame Annotations

## Change markers (tooltips of the gutter bars and triangles).

git-gutter-marker-added = Lines added since HEAD — click to show the change
git-gutter-marker-modified = Lines changed since HEAD — click to show the change
git-gutter-marker-deleted = Lines deleted since HEAD — click to show the change

## Inline diff popup.

git-gutter-popup-label = Change preview
git-gutter-popup-revert = Rollback
git-gutter-popup-stage = Stage
git-gutter-popup-previous = Previous Change
git-gutter-popup-next = Next Change
git-gutter-popup-close = Close
# Which change this is. $index: from 1, $count: all
git-gutter-popup-position = { $index } / { $count }
git-gutter-stage-dirty = The document has unsaved edits — save it to stage this change
git-gutter-stage-read-only = This document cannot be edited, so the change is not rolled back
git-gutter-staged-already = This change is already staged
git-gutter-stage-inexact = This change does not line up with git's diff, so it cannot be staged on its own — pick the lines in the Commit tool window
git-gutter-stage-disk-changed = The file on disk differs from this document, so nothing was staged
git-gutter-stage-failed = Could not stage: { $message }

## Why the gutter is off (bar above the editor).

# $driver: the driver the .gitattributes filter attribute picks (lfs, …)
git-gutter-off-filter = Git change markers are off: this file goes through the filter driver `{ $driver }` (drivers are never run automatically)
git-gutter-off-dismiss = Dismiss

## Blame.

git-blame-uncommitted = Not Committed Yet
# Faded text at the end of the current line. $author: author, $when: relative time such as "3 days ago", $subject: commit subject
git-blame-inline = { $author }, { $when } · { $subject }
# Author line of an annotation tooltip (below the hash, the subject below it). $date: author time
git-blame-tooltip-author = { $author } <{ $email }> · { $date }
git-blame-boundary = Boundary commit (the first commit or the end of a shallow clone) — this line may not have started here
git-blame-click = Click to select this commit in the Git log
git-blame-no-moves = Line moves are not tracked — lines moved within the file show the commit that moved them
git-blame-loading = Reading blame…
git-blame-failed = Could not read blame: { $message }
git-blame-menu-previous = Annotate Previous Revision
git-blame-menu-close = Close Annotations
git-blame-menu-history = Show History
