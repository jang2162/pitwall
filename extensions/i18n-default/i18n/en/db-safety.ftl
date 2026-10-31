### DB safety parts (apps/desktop/src/db/safety, 14-m3-plan.md M3-19): connection marks, the risky-query confirmation, refusal
### sentences, TLS marks, session notices, and the status bar's `db` item.

## Connection marks — always a label next to the color (10-ui.md "Accessibility").

db-label-production = Production
db-label-read-only = Read-only

## The TLS state of a connection (`ServerInfo.tls`).

db-tls-unverified = Server certificate not verified
db-tls-unverified-hint = Connected over TLS without checking the server certificate or name, so an intercepting server looks the same. Set sslmode to verify-full in the connection settings to check it
db-tls-plaintext = Plaintext
db-tls-plaintext-hint = Connected without TLS: the password, queries and results cross the network unencrypted

## Risk grades (`RiskGrade`).

db-grade-read = Read
db-grade-write = Write
db-grade-unbounded-write = Unbounded write
db-grade-ddl = Schema change
db-grade-destructive = Destructive

## Titles of the risky-query confirmation (by the highest grade).

db-confirm-title-read = Confirm before running
db-confirm-title-write = These statements change data
db-confirm-title-unbounded-write = These statements can change many rows at once
db-confirm-title-ddl = These statements change the schema
db-confirm-title-destructive = These statements cannot be undone
db-confirm-title-unread = Some statements could not be analyzed — treated as irreversible because what they do is unknown

## The risky-query confirmation.

# $grade: grade name
db-confirm-grade = Grade: { $grade }
# $connection: connection name
db-confirm-connection = Connection: { $connection }
# $connection: connection name
db-confirm-production = { $connection } is a production connection: everything but queries needs a confirmation
# $connection: connection name
db-confirm-name-prompt = Type the connection name { $connection } to run
db-confirm-name = Connection name
db-confirm-name-mismatch = Does not match the connection name
# $count: number of statements
db-confirm-statements = { $count ->
        [one] One statement
       *[other] { $count } statements
    }
# $count: statements not shown
db-confirm-more = …and { $count } more
db-confirm-unparsed = Statement that could not be analyzed
# $index: statement number (from 1)
db-confirm-statement = Statement { $index }
db-confirm-cancel = Cancel
db-confirm-run = Run
db-confirm-rejected-hash-mismatch = The statements changed after you confirmed, so this asks again
db-confirm-rejected-grade-too-low = A statement is riskier than what you confirmed, so this asks again
db-confirm-rejected-name-mismatch = The typed connection name did not match, so this asks again
db-confirm-rejected-other = The earlier confirmation was not accepted, so this asks again

## Transaction effects (`DbTxEffect`) and the cancel confirmation (3002).

db-effect-implicit-commit = This statement commits the open transaction first: the earlier changes are committed even if it fails
db-cancel-rollback-title = Cancelling rolls back the whole transaction
db-cancel-rollback-body = Stopping this write makes the database undo the whole open transaction: every change not yet committed in this tab is lost
db-cancel-rollback-confirm = Stop and roll back

## Reasons (`RiskReason`), shown next to each statement's grade.

db-reason-unparsed = Could not be analyzed: what it does is unknown, so it counts as destructive
db-reason-too-large = Too large to analyze, so it counts as destructive
db-reason-too-deep = Nested too deeply to analyze, so it counts as destructive
db-reason-drop = Drops an object, a column or a partition
db-reason-truncate = Deletes every row of a table
db-reason-copy-program = Runs a shell command on the database server
db-reason-ddl = Creates or changes schema objects or privileges
db-reason-missing-where = Has no WHERE, so every row changes
db-reason-unbounded-where = Its WHERE does not limit the rows (always true, or a subquery)
db-reason-multi-table = Changes rows through a join or several tables, so the range is not guaranteed
db-reason-or-replace = Can delete other rows that conflict (OR REPLACE)
db-reason-merge = MERGE can insert, update and delete many rows
db-reason-dml = Inserts, updates or deletes rows
db-reason-locking-read = Locks the rows it reads (FOR UPDATE, FOR SHARE)
db-reason-side-effect-function = Calls a function with side effects
db-reason-server-admin = An administrative action that reaches other sessions or the whole server
db-reason-replication-reset = Deletes the binary and relay logs and the replication position; this cannot be undone
db-reason-select-into = Writes the query result into variables, a file or a new table
db-reason-transaction-control = Opens or ends a transaction
db-reason-session-state = Changes session settings
db-reason-lock = Locks or unlocks tables
db-reason-procedural = Runs a procedure or a prepared statement whose contents are unknown
db-reason-attach = Attaches or detaches another database file
db-reason-pragma = Reads or changes SQLite settings (PRAGMA)
db-reason-maintenance = Cleans up or analyzes tables (VACUUM, ANALYZE, OPTIMIZE)
db-reason-data-transfer = Loads or exports data from or to files or elsewhere
db-reason-other = A kind of statement that is not classified, so it counts as a write

## Refusal sentences: why the Host did not run something (`data.reason`).

db-refusal-read-only-connection = This connection is read-only and only runs queries; the statement did not run
db-refusal-untrusted = This project is not trusted, so it does not connect to databases. Trust the project, then connect again
db-refusal-not-project-window = A database connection is used only from its project's window
db-refusal-not-a-query = Only a single query runs here; the statement did not run
# $message: the Host's reason
db-refusal-transaction-statement = This transaction statement cannot run: { $message }
db-refusal-busy = Another run is still going in this tab
db-refusal-config-changed = The connection settings changed; connect again to run new statements
db-refusal-transaction-open = A transaction is open, so this does not switch to auto-commit. Commit or roll back first

## Session notices (`DbSessionNotice`).

# $message: the server or driver error
db-notice-rolled-back = The transaction was rolled back: { $message }
db-notice-committed-implicitly = This statement committed the open transaction (implicit commit): the earlier changes are committed even though it failed
db-notice-transaction-ended = The transaction ended. The server did not say whether it committed or rolled back; check from another connection
# $message: the error of that step
db-notice-transaction-ended-message = The transaction ended. The server did not say whether it committed or rolled back; check from another connection ({ $message })
# $message: why the state could not be read
db-notice-transaction-unknown = Could not ask the server for the transaction state: it may be open or ended ({ $message })
db-notice-transaction-opened = A transaction this tab did not open is now open; it lasts until you commit or roll back
db-notice-connection-lost = The connection was lost
db-notice-connection-lost-rolled-back = The connection was lost; the database rolled back the open transaction
db-notice-connection-lost-outcome-unknown = The connection was lost during a commit, so the outcome is unknown; check from another connection
db-notice-config-changed = The connection settings changed; connect again to run new statements
# $until: when keeping ends
db-notice-unattended = The window closed, so the open transaction is kept until { $until }; it rolls back unless this tab reattaches by then
db-notice-unattended-rolled-back = Nobody reattached, so the open transaction was rolled back

## The status bar's `db` item (the current tab's session).

db-status-mode-auto = Auto-commit
db-status-mode-manual = Manual commit
# $elapsed: time since the transaction opened (m:ss)
db-status-transaction = TX { $elapsed }
# $elapsed: time since the transaction opened (m:ss)
db-status-failed = TX failed { $elapsed }, rollback only
db-status-disconnected = Disconnected
db-status-config-changed = Settings changed, connect again
# $connection: connection name
# $mode: commit mode
db-status-hint = { $connection } · { $mode }

## Asking for DB access (apps/desktop/src/db/safety/access-prompt.tsx, 14-m3-plan.md M3-13 — 3101 password needed, 3102 approval needed).

db-access-cancel = Cancel
# $connection: connection name
db-access-secret-title = Password for { $connection }
db-access-secret-missing = No password is saved
db-access-secret-target-changed = The connection target differs from when the password was saved — it was not sent. Enter the password to send to this target
db-access-secret-unavailable = The keychain cannot be used (locked or no service)
db-access-secret-label = Password
db-access-secret-save = Save and Connect
db-access-approve-title-env = Send an environment variable's value to this connection
db-access-approve-title-file = Open an SQLite file outside the project
db-access-approve-title-tunnel = Open an SSH tunnel
db-access-approve-title-cert-files = Read certificate files for TLS
db-access-approve-hint = The committed connection settings ask for this. It is asked again when it changes
db-access-approve = Approve and Connect
db-access-previous = Approved before
db-access-now = Now
db-access-variable = Environment variable
db-access-path = File
db-access-destination = SSH destination
db-access-driver = Driver
db-access-host = Host
db-access-port = Port
db-access-user = User
db-access-tunnel = Tunnel
db-access-sslmode = sslmode
db-access-sslrootcert = Root certificate
db-access-sslcert = Client certificate
db-access-sslkey = Client key
db-access-none = (none)
# $error: error text
db-access-failed = Failed: { $error }
db-access-sending = Sending — Cancel now only stops waiting. A value the host already received is saved and used the next time this connection opens
