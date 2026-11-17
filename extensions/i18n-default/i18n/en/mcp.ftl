### MCP — status bar item and connection count in the welcome window (16-m5-plan M5-10), Agent Activity tool window (M5-13).
### Keys are wired to the screen by apps/desktop/src/mcp/strings.ts.

## Status bar item

# $count: MCP sessions whose session project is this project (no item at 0)
mcp-status-label = MCP { $count }
# $count: same as above
mcp-status-hint =
    { $count ->
        [one] 1 agent connected to this project — click to open Agent Activity
       *[other] { $count } agents connected to this project — click to open Agent Activity
    }

## Connected agents in the Agent Activity tool window (M5-13 — moved from the M5-10 session list popup)

mcp-sessions-title = Connected agents
mcp-sessions-empty = No agents are connected to this project
mcp-sessions-unknown-client = Unknown client
# $time: start time (local)
mcp-sessions-started = Started { $time }
# $count: tool calls the session received
mcp-sessions-calls =
    { $count ->
        [one] 1 call
       *[other] { $count } calls
    }

## Welcome window bottom line

# $count: all MCP sessions open in this app
mcp-welcome-connections = MCP: { $count } connected

## Agent Activity tool window (16-m5-plan M5-13)

mcp-activity-approvals-title = Pending approvals
mcp-activity-approvals-empty = No approval requests are waiting
# $count: pending requests not drawn in the list (only the first rows are drawn)
mcp-activity-approvals-more =
    { $count ->
        [one] and 1 more
       *[other] and { $count } more
    }
# $agent: requesting agent, $tool: tool name — clicking the row reopens the approval dialog
mcp-activity-approvals-open = Open the approval dialog for { $tool } from { $agent }
mcp-activity-log-title = Call log
# The audit log shows this project's rows together with rows without a project (the per-tool recent call count in Settings > MCP counts this project's rows only)
mcp-activity-log-scope = Calls on this project and calls without a project
mcp-activity-column-time = Time
mcp-activity-column-client = Client
mcp-activity-column-tool = Tool
mcp-activity-column-args = Arguments
mcp-activity-column-outcome = Result
mcp-activity-column-duration = Took
mcp-activity-column-approval = Approval
mcp-activity-outcome-ok = ok
mcp-activity-outcome-error = error
mcp-activity-outcome-denied = denied
# The row was written before the call ran and has no end yet — still running, or the process died before writing it
mcp-activity-outcome-started = end unknown
mcp-activity-approval-auto = auto
mcp-activity-approval-once = once
mcp-activity-approval-session = session
mcp-activity-approval-window = while window open
# Calls that did not pass approval (read, refused)
mcp-activity-approval-none = none
# Marks a call without a project
mcp-activity-no-project = no project
# $ms: time taken (milliseconds)
mcp-activity-duration = { $ms } ms
mcp-activity-filter-outcome = Filter by result
mcp-activity-filter-tool = Filter by tool
mcp-activity-filter-client = Filter by client
mcp-activity-filter-all-outcomes = All results
mcp-activity-filter-all-tools = All tools
mcp-activity-filter-all-clients = All clients
mcp-activity-loading = Loading…
mcp-activity-empty = No calls recorded
mcp-activity-empty-filtered = No calls match the filters
# $message: error message
mcp-activity-load-failed = Cannot read the audit log: { $message }
mcp-activity-clear = Clear log
mcp-activity-clear-title = Clear this project's call log?
mcp-activity-clear-body = Deletes every audit log row of this project. Rows of calls without a project stay. This cannot be undone.
mcp-activity-clear-confirm = Clear
mcp-activity-clear-cancel = Cancel
mcp-activity-clear-failed = Cannot clear the call log

## Approval dialog and OS notification (16-m5-plan M5-14)

mcp-approval-title = An agent is waiting for approval
mcp-approval-not-this-project = This request is not for this project — it is a write without a project, so it opened in the most recently focused window
mcp-approval-agent = Agent
mcp-approval-unknown-agent = An agent that did not give its name
mcp-approval-cli = Called from the CLI (pitwall tool)
mcp-approval-tool = Tool
mcp-approval-tier = Tier
mcp-approval-tier-read = Read
mcp-approval-tier-write = Write
mcp-approval-tier-exec = Exec
mcp-approval-tier-dangerous = Dangerous
mcp-approval-summary = Summary
mcp-approval-agent-text = The text in the boxes below was sent by the agent. Invisible characters are shown as ⟨U+…⟩
# $count: number of changes
mcp-approval-changes =
    { $count ->
        [one] 1 change
       *[other] { $count } changes
    }
mcp-approval-no-changes = No list of changes — see the summary
mcp-approval-file-added = New file
mcp-approval-file-deleted = File to delete
mcp-approval-file-modified = File to change
mcp-approval-unchanged-lines = $ unchanged lines
# $count: number of characters shown
mcp-approval-text-cut = The text is long; only the first { $count } characters are shown — the approval covers all of it
mcp-approval-invisible = Contains invisible characters
mcp-approval-sql-connection = Connection
mcp-approval-sql-file = File
mcp-approval-sql-irreversible = Cannot be undone — some statements commit outside the transaction
# $count: number of statements
mcp-approval-sql-statements =
    { $count ->
        [one] 1 statement
       *[other] { $count } statements
    }
# $n: statement number (from 1), $line: first line
mcp-approval-sql-statement = Statement { $n } · line { $line }
# $count: read statements folded away (statements that are not reads are always all shown)
mcp-approval-sql-folded-reads =
    { $count ->
        [one] and 1 more read statement, folded — every statement that is not a read is shown
       *[other] and { $count } more read statements, folded — every statement that is not a read is shown
    }
# $time: time left (minutes:seconds)
mcp-approval-remaining = { $time } left
# $index: this request's place, $total: requests waiting in this window
mcp-approval-position = { $index } of { $total }
mcp-approval-locked = Available in a moment — locked so that a key or click from before the dialog cannot allow it
mcp-approval-deny = Deny
mcp-approval-once = Allow once
mcp-approval-session = Allow for this session
mcp-approval-window = Allow the CLI while this window is open
mcp-approval-notify-title = Pitwall approval request
# $agent: requesting agent ("An agent" if it gave no name), $tool: tool name
mcp-approval-notify-body = { $agent } is waiting for approval of { $tool }
mcp-approval-notify-agent = An agent
