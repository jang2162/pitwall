### MCP — status bar item, session list popup, connection count in the welcome window (16-m5-plan M5-10).
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
