### SQL console (M3-15, apps/desktop/src/db/console): top bar, results, query plan, open transaction prompt, actions.

db-console-title = SQL Console
# $connection: connection name
db-console-tab-title = { $connection } console

## Top bar

db-console-connection = Connection
db-console-no-connection = Choose a connection
db-console-database = Database
db-console-database-default = (configured)
db-console-schema = Schema
db-console-schema-default = (server default)
db-console-connect = Connect
db-console-disconnect = Disconnect
db-console-run = Run
db-console-run-hint = Runs the selected statements, or the statement at the caret
db-console-cancel = Cancel
db-console-cancel-hint = Stops the running query on the server
db-console-explain = Explain
db-console-mode = TX
db-console-commit = Commit
db-console-rollback = Rollback
db-console-not-connected = Not connected — press Connect to open a session.
db-console-failed = The transaction failed and can only be rolled back — press Rollback.
db-console-disconnected = The connection was lost — press Connect to reconnect.
db-console-config-changed = The connection settings changed — commit or roll back, then connect again.
db-console-choose-locked = The connection, database and schema stay while a transaction is open.

## Results

db-console-results = Query Results
# $index: statement number (from 1)
db-console-result-tab = Result { $index }
db-console-plan = Query Plan
db-console-results-empty = Results of the statements you run appear here.
db-console-plan-cost = Cost
db-console-plan-rows = Rows
db-console-plan-empty = The plan is empty.
db-console-resize = Border between the console and its results

## Open transaction prompt (closing the tab, the window or the app)

db-console-close-title = A transaction is open
db-console-close-failed-title = A failed transaction is open
db-console-close-body = This console's manual transaction has not ended. Commit or roll it back before closing.
db-console-close-failed-body = The transaction failed and can only be rolled back. Roll it back before closing.
db-console-close-cancel = Cancel
db-console-close-rollback = Roll Back
db-console-close-commit = Commit

## Actions

db-console-action-open = New SQL Console
db-console-action-run = Run SQL
db-console-action-run-all = Run Whole Console
db-console-action-cancel = Cancel Query
db-console-action-explain = Show Query Plan

## Errors and notices

# $message: reason
db-console-load-failed = Cannot read the console: { $message }
# $message: reason
db-console-save-failed = Cannot save the console: { $message }
# $message: the Host's error
db-console-error = { $message }
db-console-nothing-to-run = There is no statement to run.
db-console-explain-one = A plan is for one statement — put the caret in a single statement.
db-console-unattended-title = Transactions rolled back while no one was attached
# $connection: connection name, $since: when the transaction opened, $minutes: minutes kept
db-console-unattended-body = { $connection } — the transaction opened at { $since } was rolled back after { $minutes } minutes.
# $connection: connection name
db-console-unattended-body-unknown = { $connection } — the window closed, so its open transaction was rolled back.
