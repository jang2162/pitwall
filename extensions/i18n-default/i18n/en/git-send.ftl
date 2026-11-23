### Commit send and receive (apps/desktop/src/git/send): the popup, confirmation and notices for sending commits to and
### receiving them from another machine's checkout (18-m7-plan M7-17).

action-git-send = Send to Another Checkout…
action-git-receive = Receive from Another Checkout…

## The popup that picks the other checkout.

git-send-pick-send = Checkout to send to
git-send-pick-receive = Checkout to receive from
git-send-pick-placeholder = Find a checkout
git-send-pick-empty = This project has no checkout on another machine
git-send-main = Main checkout
git-send-last = Last target

## Confirmation.

# $branch: branch, $from: source machine, $to: target machine
git-send-confirm-title = { $branch } from { $from } to { $to }
git-send-from = From
git-send-to = To
git-send-loading = Reading the source branch…
git-send-not-branch = The source checkout is not on a branch — there is no branch to send
git-send-not-connected = That machine is not connected
git-send-uncommitted = The source has uncommitted changes — they do not go
# $runs: runs, $terminals: terminals
git-send-target-busy = { $runs } runs and { $terminals } terminals are running in the target checkout — this does not stop them
git-send-no-force = If the target branch has diverged, the send stops instead of overwriting it
git-send-go = Send
git-send-go-receive = Receive

## Progress and result.

# $to: target machine
git-send-progress = Sending to { $to }
git-send-cancel = Cancel the send
# $branch: branch, $to: target machine
git-send-created = Created { $branch } on { $to } and checked it out
# $branch: branch, $to: target machine, $range: "previous hash → new hash"
git-send-moved = Moved { $branch } on { $to } ({ $range }) and checked it out
# $branch: branch, $to: target machine
git-send-same = { $branch } on { $to } was already at that commit — checked it out
git-send-uncommitted-left = The source's uncommitted changes did not go
# $count: files left as LFS pointers
git-send-lfs = { $count } files stayed LFS pointers — run git lfs pull on that machine
git-send-run-there = Run There
git-send-terminal-there = Terminal There
# $branch: branch, $to: target machine
git-send-stopped = Stopped before updating { $branch } on { $to } — the target is unchanged
# $ref: the ref holding the received commits
git-send-incoming = The received commits are in { $ref }
git-send-reason = { $reason ->
    [diverged] The target branch has commits the source does not have (diverged)
    [branchMoved] The target branch moved after it was picked
    [incomingMoved] The ref holding the received commits changed meanwhile
    [inOtherWorktree] Another worktree on the target machine uses that branch
   *[blocked] Git refused
}
git-send-failed = Commit send failed
