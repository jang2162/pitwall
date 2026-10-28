### Git dialogs (apps/desktop/src/git/dialogs): risky-operation confirmations, push, pull, fetch, branch names, [Undo], and the
### settings window's Git page.

action-git-push = Push…
action-git-pull = Update Project…
action-git-fetch = Fetch

## Shared parts.

git-dialog-cancel = Cancel
git-dialog-danger = This cannot be undone from Pitwall
git-dialog-no-project = Git needs a project window
git-dialog-no-repository = This project is not in a Git repository
# $what: the operation that failed
git-dialog-failed = { $what } failed

## What a reset or undo would lose (git.resetPreview).

# $count: number of changes lost
git-lost-count = { $count ->
        [one] This change will be lost:
       *[other] These { $count } changes will be lost:
    }
# $count: lines not shown
git-lost-more = …and { $count } more
git-lost-nothing = Nothing will be lost
git-lost-unverifiable = Too much would be lost to list it all, so this cannot be done
git-lost-changed = The working tree changed — check the new list

## Branch create, rename, delete.

git-branch-name = Branch name
git-branch-create-title = New Branch
# $from: where the branch starts
git-branch-create-from = From { $from }
git-branch-create-checkout = Check out the branch
git-branch-create = Create
# $name: current branch name
git-branch-rename-title = Rename { $name }
git-branch-rename = Rename
git-branch-name-empty = Enter a branch name
git-branch-name-invalid = Git does not allow this branch name
git-branch-name-taken = A branch with this name already exists
# $name: branch
git-branch-force-title = Delete { $name } even though it is not merged?
# $name: branch, $commit: short hash of its tip
git-branch-force-body = { $name } (at { $commit }) has commits that are not merged. Git refused to delete it:
git-branch-force-confirm = Force Delete
# $name: branch, $commit: short hash it pointed at
git-branch-deleted = Deleted { $name } (was { $commit })

## [Undo] of an operation that moved a ref.

git-undo = Undo
# $ref: branch, $from / $to: short hashes (∅ when there was none)
git-undo-moved = { $ref }: { $from } → { $to }
# $commit: short hash to go back to
git-undo-title = Undo back to { $commit }
git-undo-confirm = Undo
# $ref: branch
git-undo-done = Undid the change to { $ref }
# $name: branch
git-undo-restored = Restored { $name }
# $name: branch
git-undo-removed = Removed { $name }
git-undo-failed = Undo failed
# $name: branch, $commit: short hash it is at now (∅ when gone)
git-undo-moved-since = { $name } moved to { $commit } since — not undoing

## Reset confirmation.

# $mode: soft · mixed · keep · hard, $commit: short hash
git-reset-title = Reset --{ $mode } to { $commit }
git-reset-confirm = Reset

## Push dialog (⌘⇧K).

git-push-title = Push
git-push-loading = Working out where this push goes…
# $branch: local branch, $remote: remote name, $remoteRef: ref on the remote
git-push-target = { $branch } → { $remote } : { $remoteRef }
# $source: what decided the target
git-push-source = { $source ->
        [input] Target chosen here
        [pushRefspec] Target from the remote's push setting
       *[pushDefault] Target from push.default
    }
git-push-terminal = git push in a terminal would do something else:
# $value: push.default
git-push-terminal-default = push.default = { $value }
# $refspecs: configured push refspecs
git-push-terminal-configured = Configured push: { $refspecs }
# $sends: what git would send
git-push-terminal-sends = Git would send: { $sends }
git-push-terminal-nothing = Git would send nothing
# $message: git's reason
git-push-terminal-refuses = Git would refuse: { $message }
# $reason: why git would refuse the push
git-push-refused = { $reason ->
        [nothing] push.default is "nothing", so git pushes nowhere.
        [matching] push.default is "matching" — Pitwall does not push every matching branch.
        [noUpstream] This branch has no upstream.
        [upstreamNameDiffers] The upstream branch has a different name.
        [notUpstreamRemote] This remote is not the branch's upstream remote.
        [multipleUpstreams] The branch has more than one upstream.
        [notPushedByConfig] The remote's push setting does not push this branch.
        [mirror] The remote is a mirror.
        [invalidConfig] Git does not accept the push settings.
       *[unqualifiedDestination] The configured target is a short name, so git would pick it from the remote's refs.
    }
git-push-candidates = Choose where to push:
# $count: commits to push
git-push-commits = { $count ->
        [one] 1 commit to push
       *[other] { $count } commits to push
    }
git-push-commits-more = …more not shown
git-push-no-commits = No new commits — only the ref is sent
git-push-set-upstream = Set as upstream
git-push-force = Force push (with lease)
git-push-force-unavailable = The remote has no such branch yet — there is nothing to force over
git-push-force-title = Force push?
# $remoteRef: ref on the remote, $commit: short hash last seen there
git-push-force-body = This replaces { $remoteRef } on the remote, last seen at { $commit }. Commits that only the remote has will be lost. If the remote moved since, git refuses and nothing changes (--force-with-lease).
git-push-force-confirm = Force Push
git-push-send = Push
git-push-sending = Pushing…
git-push-changed = The push target changed since it was shown — check the new target
git-push-rejected = Git rejected the push:
git-push-pull = Pull…
# $branch, $remote, $remoteRef: where it actually went
git-push-done = Pushed { $branch } → { $remote } : { $remoteRef }
# $refspec: the refspec sent
git-push-done-refspec = Sent { $refspec }

## Pull dialog (⌘T).

git-pull-title = Update Project
git-pull-mode = Update with
git-pull-merge = Merge
git-pull-rebase = Rebase
git-pull-remote = Remote
git-pull-branch = Branch
# $name: upstream branch
git-pull-upstream = Upstream: { $name }
git-pull-no-upstream = This branch has no upstream — choose a remote branch
git-pull-send = Update
git-pull-done = Updated
git-pull-stopped = Stopped on conflicts — resolve them, then continue or abort in the commit tool window

## Fetch.

git-fetch-done = Fetched all remotes

## Settings window › Git (10-ui S7).

settings-git-title = Git
settings-git-path = Executable
# $value: the [git] path setting
settings-git-configured = Set in settings: { $value }
settings-git-not-configured = Found as "git" on the login shell's PATH
settings-git-version = Version
# $minimum: lowest version that can write
settings-git-minimum = Writing needs { $minimum } or newer
settings-git-writes = Writes
settings-git-writes-on = On
# $program: the name searched for
settings-git-not-found = Off — { $program } was not found
# $version: found version, $minimum: lowest version that can write
settings-git-too-old = Off — { $version } is older than { $minimum }
# $reason: why git --version could not be read
settings-git-unreadable = Off — the version could not be read: { $reason }
settings-git-shell-path = Searched the login shell's PATH. A trusted project's runtime (mise) can put another git first.
# $reason: why the login shell could not be read
settings-git-process-path = The login shell could not be read ({ $reason }), so the app's own PATH was searched.
settings-git-checking = Checking…
# $error: the error
settings-git-failed = Could not check: { $error }
settings-git-recheck = Check Again
