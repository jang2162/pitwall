### Table data (M3-17, apps/desktop/src/db/table-data): top bar, editing, submit preview, close confirmation, actions.

db-table-data-title = Table Data
# $table: table name
db-table-data-tab-title = { $table }

## Top bar

db-table-data-connect = Connect
db-table-data-disconnect = Disconnect
db-table-data-where = WHERE
db-table-data-where-placeholder = e.g. id > 10 AND name LIKE 'a%'
db-table-data-order-by = ORDER BY
db-table-data-order-by-placeholder = e.g. created_at DESC
db-table-data-read = Reload
db-table-data-read-hint = Read again with the WHERE and ORDER BY (Enter in a field)
db-table-data-add-row = Add Row
db-table-data-delete-row = Delete Row
db-table-data-submit = Submit
db-table-data-revert = Revert
# $count: changes (edited, added and deleted rows)
db-table-data-pending = { $count ->
    [one] { $count } unsubmitted change
   *[other] { $count } unsubmitted changes
}
db-table-data-not-connected = Not connected — press Connect to open a session and read the table.
db-table-data-disconnected = The connection was lost — press Connect to reconnect.
db-table-data-pending-blocks-read = There are unsubmitted changes, so the table is not read again — submit or revert them first.
# $message: error text
db-table-data-error = Error: { $message }
# $statements: statements, $rows: changed rows
db-table-data-applied = Applied { $statements } statements — { $rows } rows changed.

## Why editing is off

db-table-data-block-read-only = This is a read-only connection, so the data cannot be edited.
db-table-data-block-no-key = This table has no key (a primary key or a unique key on NOT NULL columns only), so a row cannot be pointed at exactly and the data cannot be edited.
db-table-data-block-key-not-in-result = The result does not hold the key columns, so the data cannot be edited.
# $message: error text
db-table-data-block-failed = The key could not be read, so the data cannot be edited: { $message }

## Change marks (spoken by the grid)

db-table-data-mark-changed = Changed
db-table-data-mark-inserted = Row to add
db-table-data-mark-deleted = Row to delete

## Cell editing

# $column: column name
db-table-data-edit-label = Value of { $column }
db-table-data-edit-null = NULL
db-table-data-edit-apply = Apply
db-table-data-edit-cancel = Cancel
db-table-data-edit-key = Key columns are not edited — an edit that changes the key is not accepted (delete the row and add a new one).
db-table-data-edit-truncated = This value was cut short, so it is not edited — the whole value was not read.
db-table-data-edit-deleted = A row to delete is not edited — press Delete Row again to keep it, then edit.
db-table-data-fk-null = The value is NULL, so it points at no row.
db-table-data-fk-pending = An added row or an edited cell is not followed — a foreign key is followed by the value that was read.

## Submit preview

db-table-data-preview-title = Submit Changes
db-table-data-preview-own = Applies in one transaction and commits — if any statement affects other than one row, everything is rolled back.
db-table-data-preview-open = Applies inside the open transaction — a failure undoes only this edit, and the commit is separate.
# $count: statements
db-table-data-preview-statements = { $count ->
    [one] { $count } statement
   *[other] { $count } statements
}
db-table-data-preview-apply = Apply
db-table-data-preview-cancel = Cancel
db-table-data-preview-applying = Applying…

## Apply failures

# $change: the change ("Update 1" etc.), $rows: affected rows
db-table-data-fail-row-count = { $change } affected { $rows } rows — another session may have changed or deleted that row.
db-table-data-fail-changed = The session changed after the preview — submit again to see a new preview.
# $message: the database's error text
db-table-data-fail-db = The database refused: { $message }
# $reason: the Host's reason name, $column: column name (empty if none)
db-table-data-fail-invalid = This change is not accepted ({ $reason }{ $column })
db-table-data-undone-edit = Nothing changed (the edit transaction was rolled back).
db-table-data-undone-savepoint = Only this edit was undone — earlier changes in the open transaction remain.
db-table-data-undone-transaction = The server ended the whole transaction — see the session notice.
db-table-data-undone-unknown = It is not known whether it was undone — check the table over another connection.
# $index: from 1
db-table-data-change-update = Update { $index }
db-table-data-change-insert = Insert { $index }
db-table-data-change-delete = Delete { $index }

## Close confirmation

db-table-data-close-title = There are unsubmitted changes
# $table: table name, $count: changes
db-table-data-close-body = { $count } changes to { $table } were not submitted. Discarding them on close cannot be undone.
db-table-data-close-confirm = Discard and Close

## Actions

db-table-data-action-open = Open Table Data
db-table-data-action-submit = Submit Changes
db-table-data-action-revert = Revert Changes
db-table-data-action-add-row = Add Row
db-table-data-action-delete-row = Delete Row
