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
