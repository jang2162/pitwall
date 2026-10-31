### DB explorer tool window (apps/desktop/src/db/explorer, 14-m3-plan.md M3-13).

db-explorer-label = Database Explorer

## Toolbar.

db-explorer-add = Add Connection
db-explorer-refresh = Refresh
db-explorer-find = Find
db-explorer-find-placeholder = Table or view name (searched on the server)
db-explorer-find-clear = Clear Find

## Empty and list states.

db-explorer-empty = This project has no database connections
db-explorer-untrusted = Trust this project to connect — only the connection list is shown
# $error: error text
db-explorer-list-failed = Cannot read the connection list: { $error }
# $problem: why the settings could not be read
db-explorer-config-problem = Cannot read the project settings: { $problem }
# $problem: why the approval record could not be read (with its path)
db-explorer-state-problem = Cannot read the approval record — every password and approval is asked again until then. Fix or remove the record file: { $problem }

## Connection states (shown as text, not only as color).

db-explorer-status-idle = Not connected
db-explorer-status-connecting = Connecting
db-explorer-status-connected = Connected
db-explorer-status-lost = Disconnected
db-explorer-status-config-changed = Settings changed — reconnect
db-explorer-status-secret-required = Password needed
db-explorer-status-approval-required = Approval needed
db-explorer-status-untrusted = Untrusted
db-explorer-status-invalid = Invalid settings
# $message: why it was lost
db-explorer-status-lost-hint = Disconnected: { $message } — expand it again to connect
db-explorer-status-config-changed-hint = The connection settings changed. It does not reconnect by itself — expand it to connect with the new settings

## Tree groups.

db-explorer-group-tables = Tables
db-explorer-group-views = Views
db-explorer-detail-columns = Columns
db-explorer-detail-indexes = Indexes
db-explorer-detail-foreign-keys = Foreign Keys
# $filter: the text being searched for
db-explorer-filtered = only names containing “{ $filter }”

## Status rows.

db-explorer-loading = Loading…
db-explorer-more = Show More
db-explorer-none = None
# $error: error text
db-explorer-failed = Cannot read: { $error }

## Items.

db-explorer-not-null = NOT NULL
db-explorer-primary-key = Primary key
db-explorer-unique = Unique
db-explorer-partial = Partial

## Context menu.

db-explorer-menu-new-console = New Console
db-explorer-menu-ddl = Show DDL
db-explorer-menu-row-count = Row Count
db-explorer-menu-copy-name = Copy Name
db-explorer-menu-table-data = Open Table Data
db-explorer-menu-open-redis = Open
db-explorer-menu-edit = Edit Connection
db-explorer-menu-delete = Delete
db-explorer-menu-connect = Connect
db-explorer-menu-disconnect = Disconnect
db-explorer-row-count-other-database = Count rows of a database other than the connection's own in a console

## Deleting a connection.

# $name: connection name
db-explorer-delete-title = Delete the connection { $name }
db-explorer-delete-body = The shared and personal settings files and this connection's saved secrets and approvals are removed. This cannot be undone.
db-explorer-delete-confirm = Delete

## Notifications.

db-explorer-row-count-title = Row count
# $name: table name, $count: row count
db-explorer-row-count-body = { $name }: { $count } rows
# $action: action name, $error: error text
db-explorer-action-failed = { $action } failed: { $error }
# $name: name
db-explorer-copied = Copied { $name }

## DDL document (⌘B).

# $name: object name
db-explorer-ddl-title = { $name } DDL
db-explorer-ddl-read = Read DDL
db-explorer-ddl-reload = Reload
db-explorer-ddl-restored = This tab was restored — reading it opens this connection's metadata connection
