### DB result grid (M3-16, apps/desktop/src/db/grid): status line, filter, copy, cell detail. Shared by the SQL console, table data and Redis views.

db-grid-label = Query Result

## Status line

# $count: rows received
db-grid-loading = Loading… { $count } rows
# $count: row count
db-grid-rows = { $count } rows
# $visible: rows shown after the filter, $total: rows received
db-grid-filtered = { $visible } of { $total } rows
# $ms: elapsed milliseconds
db-grid-elapsed = { $ms } ms
# $count: the grid row limit
db-grid-capped = Stopped at the limit ({ $count } rows)
db-grid-capped-hint = The grid keeps only this many rows. Narrow the query to see more.
# $count: rows changed
db-grid-affected = { $count } rows affected
db-grid-cancelled = Cancelled
# $message: the error
db-grid-failed = Failed: { $message }
db-grid-no-columns = The statement returns no rows

## Filter (over the rows already received — the server is not asked again)

db-grid-filter = Filter received rows
db-grid-filter-all-columns = All columns
db-grid-filter-clear = Clear filter

## Copy (⌘C copies TSV, ⌘⇧C picks a format)

db-grid-copy = Copy
db-grid-copy-as = Copy As…
db-grid-copy-as-title = Copy As
db-grid-format-csv = CSV
db-grid-format-json = JSON
db-grid-format-insert = INSERT Statements
db-grid-format-tsv = TSV
# $format: format name, $count: row count
db-grid-copied = Copied { $count } rows as { $format }
db-grid-copy-nothing = No cells are selected
# $row: row number (from 1), $column: column name
db-grid-copy-truncated = A truncated cell (row { $row }, { $column }) cannot be copied as INSERT — a value cut at the cell limit is not the original
db-grid-copy-unsupported = This database has no literal for that value, so it cannot be copied as INSERT
db-grid-copy-nul = A value with a NUL character cannot be written as a literal for this database
db-grid-copy-too-large = Too many cells are selected to copy as INSERT
# $message: the error
db-grid-copy-failed = Could not copy: { $message }

## Cell detail panel (double-click or ↩)

db-grid-detail = Cell Detail
db-grid-detail-show = Show Cell Detail
db-grid-detail-close = Close Cell Detail
db-grid-detail-none = Select a cell to see its value here
db-grid-detail-text = Text
db-grid-detail-json = JSON
db-grid-detail-hex = Hex
db-grid-detail-json-invalid = Not valid JSON, shown as text
# $size: original size, $received: size of the received prefix
db-grid-detail-truncated = Truncated at the cell limit — original { $size }, received { $received }
# $count: items not shown
db-grid-detail-more = … { $count } more
db-grid-detail-expand = Expand
db-grid-detail-collapse = Collapse
