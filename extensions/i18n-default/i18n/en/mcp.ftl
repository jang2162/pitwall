### MCP — status bar item, session list popup, connection count in the welcome window (16-m5-plan M5-10).
### Keys are wired to the screen by apps/desktop/src/mcp/strings.ts.

## Status bar item

# $count: MCP sessions whose session project is this project (no item at 0)
mcp-status-label = MCP { $count }
# $count: same as above
mcp-status-hint =
    { $count ->
        [one] 1 agent connected to this project — click to list it
       *[other] { $count } agents connected to this project — click to list them
    }

## Session list popup

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
mcp-sessions-close = Close

## Welcome window bottom line

# $count: all MCP sessions open in this app
mcp-welcome-connections = MCP: { $count } connected
