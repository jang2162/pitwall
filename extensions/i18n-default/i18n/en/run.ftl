# The Run tool window, its console, the status bar run count and the close confirmation (17-m6-plan M6-06, src/run).
# M6-07 · M6-08 · M6-09 · M6-18 add their run screens' messages here.

## Run tool window

run-tabs-label = Runs
run-empty = Nothing has run yet. Start a configuration from the run selector.
run-no-host = Cannot show runs: not connected to the Host
# $state: running, stopping or exited
run-state =
    { $state ->
        [running] Running
        [stopping] Stopping
       *[exited] Exited
    }
# $code: exit code
run-exit-code = exit { $code }
# $signal: the signal that ended the run, e.g. “Terminated: 15”
run-exit-signal = { $signal }
run-launched-by-window = Started from this window
# $client: the agent's client name (e.g. “claude-code”)
run-launched-by-agent = Started by an agent ({ $client })
run-agent-unknown = unknown client
# $time: start time
run-started-at = Started at { $time }
# $name: run name
run-close-tab = Close “{ $name }”
run-rerun = Rerun
run-stop = Stop
run-scroll-lock = Scroll Lock
run-clear = Clear Console
run-search = Find
run-search-input = Find in console
run-search-next = Next Match
run-search-previous = Previous Match
run-search-close = Close Find
run-stop-failed = Could not stop the run
run-rerun-failed = Could not rerun
# $code: exit code, or none when ended by a signal. $signal: the signal name (when $code is none)
run-exited-line =
    { $code ->
        [none] [Process ended by { $signal }]
       *[other] [Process exited with code { $code }]
    }
# $bytes: number of bytes
run-dropped-line = [{ $bytes } bytes of earlier output were dropped]
# $message: the reason the Host reported
run-attach-failed = [Could not show the output: { $message }]

## Port chips (M6-08's ports, drawn by src/run/run-ports.tsx)

# $port: TCP port number
run-port-label = :{ $port }
# $url: the address the chip opens (http://localhost:<port>, or http://[::1]:<port> for an IPv6-only listener)
run-port-open = Open { $url }
# Shown on a port listening on every address (0.0.0.0 or ::), reachable from other machines
run-port-all-addresses = all addresses

## Status bar

# $count: running runs of this project
run-status-label = Running { $count }
# $count: as above
run-status-hint = { $count } running in this project — click to open the Run tool window

## Closing the project window with runs left (M6-06 ⑥)

# $count: runs still running
run-close-title =
    { $count ->
        [one] 1 run is still running
       *[other] { $count } runs are still running
    }
run-close-body = Closing this window ends them and their child processes.
run-close-human = Started by you
# $client: the agent's client name
run-close-agent = Started by an agent ({ $client })
run-close-confirm = End and Close
run-close-cancel = Cancel

## Run selector · actions (M6-07, src/run/run-picker.tsx · run-actions.ts)

run-picker-label = Run configuration
run-picker-none = No configurations
# $name: configuration name, $profile: profile name
run-picker-with-profile = { $name } · { $profile }
run-picker-choose = Choose a run configuration
run-picker-search = Find configurations and tasks
run-picker-empty = No matching configuration
run-picker-group-configs = Configurations
run-picker-group-detected = Detected
run-picker-group-unusable = Cannot be used
run-picker-group-more = More
# $detector: detector (npm · make · gradle …), $source: file the task came from
run-picker-detected-from = { $detector } · { $source }
run-picker-save-detected = Save as configuration
run-picker-edit = Edit configurations…
run-picker-new = New configuration…
# $detector: gradle · maven · mise
run-picker-read-tasks = Read { $detector } tasks
# $detector: as above
run-picker-reread-tasks = Read { $detector } tasks again
# $detector: as above, $message: why
run-picker-read-failed = Could not read the { $detector } tasks: { $message }
run-picker-run = Run
run-picker-debug = Debug
run-picker-debug-none = This configuration has no [debug] section
# $message: the error text
run-picker-debug-failed = Could not start debugging: { $message }
run-picker-stop = Stop
# $reason: why it cannot be used (the host's text)
run-picker-unusable-reason = Cannot be used: { $reason }
# $message: what the host reported
run-picker-list-failed = Could not read the run configurations: { $message }
# $message: what the host reported
run-start-failed = Could not run: { $message }
run-action-current = Run (current configuration)
run-action-debug = Debug (current configuration)
run-action-choose = Choose and run a configuration
run-action-stop = Stop run
run-action-edit = Edit run configurations

## First run confirmation (M6-07 ⑥, src/run/run-confirm-dialog.tsx)

# $name: configuration name
run-confirm-title = Run { $name } for the first time
run-confirm-body = You have not run this content before — an agent may have written it, or it changed outside Pitwall. Look at what will run before you choose.
run-confirm-kind = Type
run-confirm-command = Command
run-confirm-definition = Definition that runs
run-confirm-cwd = Working folder
run-confirm-profile = Profile
run-confirm-env = Environment variables
run-confirm-env-none = None
run-confirm-env-files = .env files
run-confirm-masked = Looks like a secret, so the value is hidden
run-confirm-secret-warning = The command holds a value that looks like a secret (an address with a password, or a value named like a secret). It is shown as written, not hidden — check what runs.
run-confirm-not-recorded = This configuration's content cannot be recorded, so you are asked every time it runs.
run-confirm-run = Run
run-confirm-cancel = Cancel

## Configuration editor (M6-07 ③, src/run/config-editor.tsx)

run-editor-title-new = New run configuration
# $name: configuration name
run-editor-title-edit = Edit run configuration — { $name }
run-editor-id = Id (file name)
run-editor-id-hint = Letters, digits, - and _, up to 64
run-editor-name = Name
run-editor-type = Type
run-editor-cwd = Working folder
run-editor-cwd-hint = Relative to the project root. Empty means the root
run-editor-env = Environment variables
run-editor-env-name = Name
run-editor-env-value = Value
run-editor-env-add = Add variable
# $name: variable name
run-editor-env-remove = Remove { $name }
run-editor-env-file = .env files
run-editor-env-file-hint = Relative to the project root, one per line (applied in order)
run-editor-list-hint = One per line
run-editor-location = Save to
run-editor-location-shared = Shared (.pitwall/run/<id>.toml — committed)
run-editor-location-local = Personal (<id>.local.toml — this machine only)
run-editor-form = Form
run-editor-toml = View TOML
run-editor-toml-hint = Saving writes this content (read-only)
# $path: file to write
run-editor-toml-path = { $path }
# $message: why the preview failed
run-editor-toml-failed = Could not make the preview: { $message }
# $field: field name
run-editor-required = Fill in { $field }
run-editor-save = Save
run-editor-cancel = Cancel
run-editor-delete = Delete
# $message: what the host reported
run-editor-save-failed = Could not save: { $message }
# $message: what the host reported
run-editor-delete-failed = Could not delete: { $message }
run-editor-kept = Values outside the form (profiles, [debug]) are kept as they are.

## Settings window "Run" (M6-07 ③)

run-settings-title = Run
run-settings-body = Run configurations belong to each project. Create and edit them from the project window's title bar: the run configuration selector (▾) → "Edit configurations…".

## Debug (M6-18 — the Debug tool window, the breakpoint gutter, the breakpoint editor and list, src/debug/)

debug-source-outside = Outside the project
debug-source-held = Held by the debugger
# $adapter: debug adapter id, $name: the name the adapter gave the source (not a path)
debug-source-adapter = { $adapter } source · { $name }
# $path: the path the program reported, when it differs from the real one
debug-source-reported = reported as { $path }
debug-no-host = Not connected to the Host
debug-no-session = No debug session. Start one with the debug button or Debug (current configuration).
debug-sessions-label = Debug sessions
# $state: running · stopped · ended
debug-state = { $state ->
    [running] Running
    [stopped] Paused
   *[ended] Ended
}
# $reason: the reason the adapter gave (breakpoint, step, exception, pause …) — shown as is
debug-stopped-reason = Paused: { $reason }
debug-resume = Resume
debug-pause = Pause
debug-step-over = Step Over
debug-step-into = Step Into
debug-step-out = Step Out
debug-stop = Stop
debug-debugger = Debugger
debug-frames = Frames
debug-threads = Threads
debug-variables = Variables
debug-watches = Watches
debug-console = Console
debug-no-frames = Frames appear when the program pauses.
debug-no-variables = Variables appear when a frame is selected.
debug-outside-frame = Outside the project — opens read-only
debug-add-watch = Add a watch expression…
debug-remove-watch = Remove watch
debug-console-input = Expression to evaluate in the paused program
debug-console-hint = Enter runs the expression inside the program. ↑ ↓ go through the history.
# $count: how many more children the next page loads
debug-more = { $count } more…
debug-value-cut = (cut — the value is longer)
debug-too-deep = (too deep to show)
# $message: the error text
debug-read-failed = Could not read: { $message }
debug-start-failed = Could not start debugging
debug-control-failed = The debugger did not do it
# $file: file name, $line: line number
debug-bp-title = Breakpoint · { $file }:{ $line }
debug-bp-enabled = Enabled
debug-bp-condition = Condition (pause only when true)
debug-bp-log-message = Log message (do not pause — { "{" }expr{ "}" } is evaluated)
debug-bp-hit-condition = Hit count
debug-bp-remove = Remove
debug-bp-done = Done
debug-bp-plain = Breakpoint
# $text: the expression or message as written
debug-bp-describe-condition = Condition: { $text }
debug-bp-describe-log = Logs: { $text }
debug-bp-describe-hit = Hit count: { $text }
debug-bp-disabled = Disabled
# $message: why the adapter could not verify it (may be empty)
debug-bp-unverified = Not verified by the debugger { $message }
debug-bp-agent = Set by an agent
debug-bp-agent-expression = Set by an agent — its expression runs inside the program
debug-bp-list-title = Breakpoints
debug-bp-list-empty = No breakpoints
debug-bp-list-unsaved = Unsaved — goes to the debugger when the file is saved
# $message: the Host's reason (English)
debug-bp-list-problem = The stored breakpoints could not be read: { $message }
action-debug-step-over = Step Over
action-debug-step-into = Step Into
action-debug-step-out = Step Out
action-debug-resume = Resume Program
action-debug-pause = Pause Program
action-debug-toggle-breakpoint = Toggle Line Breakpoint
action-debug-view-breakpoints = View Breakpoints
action-debug-evaluate = Evaluate Expression
