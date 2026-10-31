### The Redis key document (M3-18, apps/desktop/src/db/redis): top bar, key tree, value viewer and command console.

# $connection: connection name, $database: database number
db-redis-title = { $connection } · DB { $database }
db-redis-action-open = Open Redis Keys
db-redis-open-bad-args = No connection was given to open Redis keys for

## Top bar

db-redis-connect = Connect
db-redis-disconnect = Disconnect
db-redis-connecting = Connecting…
db-redis-not-connected = Not connected: Connect opens this connection
db-redis-database = Database number
db-redis-pattern = Key pattern
db-redis-pattern-placeholder = Pattern (e.g. user:*), empty for every key
db-redis-separator = Separator
db-redis-rescan = Scan again
db-redis-stop = Stop
# $message: reason
db-redis-connect-failed = Could not connect: { $message }

## Key tree

db-redis-keys = Redis keys
# $count: keys received
db-redis-scanning = { $count } keys · scanning…
# $count: keys received, $ms: elapsed milliseconds
db-redis-scanned = { $count } keys · { $ms } ms
# $count: keys received
db-redis-scan-stopped = { $count } keys · stopped
# $count: keys received
db-redis-scan-capped = Stopped at { $count } keys, the most the tree holds. Narrow it with a pattern
# $message: reason
db-redis-scan-failed = Could not scan the keys: { $message }
db-redis-no-keys = No matching keys
db-redis-bytes-key = byte key

## Types

db-redis-type-string = string
db-redis-type-hash = hash
db-redis-type-list = list
db-redis-type-set = set
db-redis-type-zset = zset
db-redis-type-stream = stream
db-redis-type-json = JSON

## Value viewer

db-redis-pick-key = Pick a key on the left
db-redis-missing = The key is gone: deleted or expired
# $ttl: remaining time to live (e.g. 12 s)
db-redis-ttl = TTL { $ttl }
db-redis-no-ttl = No expiry
db-redis-ttl-expired = Expired
# $size: size (e.g. 1.2 KiB)
db-redis-size = Size { $size }
# $count: number of elements
db-redis-total = { $count } in all
# $page: page number (from 1)
db-redis-page = Page { $page }
db-redis-prev-page = Previous page
db-redis-next-page = Next page
db-redis-reload = Read again
db-redis-view-text = Text
db-redis-view-json = JSON
db-redis-view-hex = Hex
# $type: type name Redis returned
db-redis-unsupported = This type cannot be shown: { $type }
# $message: reason
db-redis-read-failed = Could not read the value: { $message }
db-redis-loading = Reading…

## Column names (value tables)

db-redis-col-field = field
db-redis-col-value = value
db-redis-col-member = member
db-redis-col-score = score
db-redis-col-index = index
db-redis-col-id = id

## Key actions (context menu and value viewer)

db-redis-open-value = Show Value
db-redis-edit-ttl = Edit TTL
db-redis-clear-ttl = Remove TTL
db-redis-delete = Delete
db-redis-rename = Rename
db-redis-copy-name = Copy Name
# $key: key name
db-redis-ttl-title = Edit TTL: { $key }
db-redis-ttl-seconds = New TTL (seconds)
db-redis-ttl-invalid = Enter a whole number of seconds, 1 or more
# $key: key name
db-redis-rename-title = Rename: { $key }
db-redis-rename-to = New name
db-redis-rename-same = Same as the current name
db-redis-apply = Change
db-redis-cancel = Cancel
# $target: target key name
db-redis-overwrite-note = The key { $target } already exists: renaming overwrites it
# $key: key name
db-redis-deleted = Deleted { $key }
# $from: old name, $to: new name
db-redis-renamed = Renamed { $from } → { $to }
# $key: key name
db-redis-ttl-changed = Changed the TTL of { $key }
# $key: key name
db-redis-ttl-cleared = Removed the TTL of { $key }
db-redis-key-gone = The key is gone, so nothing changed (deleted or expired)
# $message: reason
db-redis-action-failed = Could not do it: { $message }
db-redis-name-copied = Copied the key name

## Command console

db-redis-console = Commands
db-redis-console-placeholder = Command (e.g. HGETALL user:1), Enter sends it
db-redis-console-send = Send
db-redis-console-needs-session = Connect first
db-redis-console-bytes-arg = Arguments that are not UTF-8 (bytes made with \xHH) cannot be sent from the console
# $message: reason
db-redis-console-parse-failed = Could not split the command: { $message }
db-redis-console-cancelled = Not sent: the confirmation was cancelled
db-redis-console-busy = Not sent: another confirmation is open
# $message: reason
db-redis-console-key-too-large = Not sent to the server because the key is large; page through it in the value viewer: { $message }
# $message: reason
db-redis-console-failed = Failed: { $message }
# $command: command name
db-redis-blocked-not-allowed = { $command } is not allowed in the console (unknown, keyspace-wide or connection-state command)
# $command: command name
db-redis-blocked-blocking = { $command } holds the connection while it waits, so it is not allowed
# $command: command name
db-redis-blocked-scan-count = The COUNT of { $command } is too large; use the key tree
db-redis-reply-nil = (nil)
# $value: integer
db-redis-reply-integer = (integer) { $value }
# $value: double
db-redis-reply-double = (double) { $value }
# $message: the server's error text
db-redis-reply-error = (error) { $message }
db-redis-reply-empty = (empty array)
# $elements: original element count, $size: original size
db-redis-reply-truncated = …the reply was cut at the console limit ({ $elements } elements, { $size } in all)
