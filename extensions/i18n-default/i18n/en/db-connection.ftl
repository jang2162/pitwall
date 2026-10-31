### DB connection dialog (apps/desktop/src/db/connection, 14-m3-plan.md M3-14): General · SSH Tunnel · SSL · Options tabs, the
### save location, [Test Connection].

## Actions (names in Find Action).

action-db-connection-add = Add DB Connection
action-db-connection-edit = Edit DB Connection

## Title · loading.

db-conn-title-new = New DB Connection
db-conn-title-edit = Edit Connection: { $name }
db-conn-loading = Loading…
db-conn-load-failed = Could not read the connection settings: { $message }
db-conn-not-found = There is no connection “{ $id }” — its settings file was removed or belongs to another worktree
db-conn-invalid = The settings file is invalid and cannot be edited here — fix the file directly: { $message }
db-conn-config-problem = Could not read the project settings: { $problem }

## Tabs.

db-conn-tab-general = General
db-conn-tab-ssh = SSH Tunnel
db-conn-tab-ssl = SSL
db-conn-tab-options = Options

## General tab.

db-conn-field-id = Connection ID
db-conn-field-id-hint = The settings file name (.pitwall/db/<id>.toml). It cannot change after saving
db-conn-field-name = Name
db-conn-field-driver = Driver
db-conn-driver = { $driver ->
    [postgres] PostgreSQL
    [mysql] MySQL / MariaDB
    [sqlite] SQLite
    [redis] Redis
   *[other] { $driver }
}
db-conn-field-host = Host
db-conn-field-host-tunnel = With a tunnel, this is the name as seen from the bastion
db-conn-field-port = Port
db-conn-field-port-default = Default { $port }
db-conn-field-database = Database
db-conn-field-database-redis = DB Number
db-conn-field-user = User
db-conn-field-path = File
db-conn-field-path-hint = A relative path is resolved from the project root
db-conn-required = Fill in “{ $field }”

## Password — the value goes only to the keychain (`db.secret.set`), never to the settings file or the screen state.

db-conn-password = Password
db-conn-password-source-none = None
db-conn-password-source-keychain = Keychain
db-conn-password-source-env = Environment Variable
db-conn-password-stored = Saved — type a new one to replace it
db-conn-password-missing = No saved password
db-conn-password-target-changed = The saved password was for another target (host · port · user · tunnel · TLS), so it is not used — type it again
db-conn-password-unavailable = The keychain is unavailable (locked, etc.)
db-conn-password-after-approval = Something must be confirmed before connecting; the saved password is checked after that
db-conn-password-untrusted = Trust the project to save a password and test the connection
db-conn-password-hint = Saved only in the keychain — never written to the settings file
db-conn-env-variable = Variable Name
db-conn-env-hint = Read from this project's runtime environment when connecting. You are asked to confirm on first use and when the target changes
db-conn-state-problem = The approvals record could not be read, so every confirmation is asked again — “password needed” may come from this. Fix or delete the record file: { $problem }

## SSH Tunnel tab.

db-conn-ssh-use = Connect through an SSH tunnel
db-conn-ssh-host = Destination
db-conn-ssh-pick = Host from ~/.ssh/config
db-conn-ssh-direct = Enter directly
db-conn-ssh-direct-placeholder = ssh://[user@]host[:port]
db-conn-ssh-no-hosts = No host to pick in ~/.ssh/config — enter one directly
db-conn-ssh-untrusted = Trust the project to read the host list of ~/.ssh/config
db-conn-ssh-hint = The system ssh uses your ~/.ssh/config · agent · ProxyJump · known_hosts as they are. The destination is confirmed on the first connection

## SSL tab.

db-conn-ssl-mode = sslmode
db-conn-ssl-auto = Chosen from the host (disable for loopback, otherwise verify-full)
db-conn-ssl-unverified = Does not verify the server certificate
db-conn-ssl-plaintext = Connects without TLS (plaintext)
db-conn-ssl-unsupported = The { $driver } driver does not take { $mode }
db-conn-ssl-unsupported-mysql-verify-ca = The driver cannot skip only the name check — use verify-full or match the certificate name
db-conn-ssl-unsupported-chosen = This driver does not take the chosen sslmode — pick another value
db-conn-ssl-rootcert = CA Certificate
db-conn-ssl-cert = Client Certificate
db-conn-ssl-key = Client Key
db-conn-ssl-path-hint = A relative path is resolved from the project root. The files are confirmed on the first connection

## Options tab.

db-conn-read-only = Read-only
db-conn-read-only-hint = Only queries run — enforced by the classifier and a read-only transaction in the DB
db-conn-production = Production
db-conn-production-hint = Shown in red whatever the color, and every statement that is not a query asks for confirmation
db-conn-color = Color
db-conn-color-none = None
db-conn-color-tag = { $tag ->
    [red] Red
    [orange] Orange
    [yellow] Yellow
    [green] Green
    [blue] Blue
    [purple] Purple
    [gray] Gray
   *[other] { $tag }
}
db-conn-connect-timeout = Connect Timeout (seconds)
db-conn-connect-timeout-hint = 1 to 600 seconds; 10 seconds when empty

## Save location.

db-conn-location = Save To
db-conn-location-shared = Shared — .pitwall/db/{ $id }.toml (committed)
db-conn-location-local = Personal — .pitwall/db/{ $id }.local.toml (not committed)
db-conn-location-local-hint = The personal file overlays the shared one — keys only in the shared file stay

## Buttons · results.

db-conn-test = Test Connection
db-conn-testing = Testing…
db-conn-save = Save
db-conn-saving = Saving…
db-conn-cancel = Cancel
db-conn-test-ok = Connected — server { $version }
db-conn-test-ok-no-version = Connected
db-conn-test-tls = { $tls ->
    [verified] TLS — the server certificate was verified
    [unverified] TLS — the server certificate was not verified
    [plaintext] Plaintext — connected without TLS
   *[other] File DB — no TLS
}
db-conn-test-draft = Tests the unsaved draft — a typed password is used only for this test
db-conn-test-failed = Could not connect: { $message }
db-conn-test-secret = A password is needed — type it in the password field and test again
db-conn-test-approval = Something must be confirmed before connecting ({ $kind ->
    [env] environment variable
    [file] file outside the project
    [tunnel] SSH tunnel destination
   *[other] certificate files
}) — you are asked when connecting after saving
db-conn-save-failed = Could not save: { $message }
db-conn-secret-failed = The settings were saved but the password was not: { $message }
db-conn-saved = Saved the connection “{ $name }”
