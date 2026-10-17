# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Hostly is a desktop SSH client in the spirit of Termius. It provides saved hosts, tabbed terminals with split panes, find in terminal, a dual-pane SFTP browser, port forwarding (local, remote, SOCKS), snippets, local and serial terminals, and Mosh. It keeps secrets in the OS keychain, and syncs hosts, snippets, rules and (end-to-end encrypted) secrets through a self-hosted server. It is a Tauri 2 app: a Rust core does SSH and storage, and a vanilla TypeScript + xterm.js UI runs in the webview. It is a clean-room rebuild: Termius only served as a feature reference, and no Termius code or assets are used. The project was renamed from "Ferrite"; the folder is still named `ferrite`.

## Commands

Rust lives in `~/.cargo/bin`. If `cargo` isn't found, prefix commands with `export PATH="$HOME/.cargo/bin:$PATH"`.

```sh
npm install
npm run tauri dev                       # dev window: Vite on :1420 + Rust app
npx tsc --noEmit                        # type-check the UI (no frontend test suite exists)
npm run build                           # tsc + vite build into dist/
npx tauri build --debug --bundles app   # quick debug Hostly.app in target/debug/bundle/macos
npm run tauri build                     # release bundle

cargo test --workspace                  # all Rust tests
cargo test -p hostly-core --lib         # core unit tests only
cargo test -p hostly-core --test sftp   # one integration file (also: shell_session, forward, local_session, mosh, serial)
cargo test -p hostly-core --test sftp upload_then_download_round_trips_a_directory_tree   # one test
cargo test -p hostly                    # Tauri crate tests (data migration)
cargo test -p hostly-server             # API, migration and end-to-end sync tests; need TEST_DATABASE_URL (in .env), else they skip
cargo run -p hostly-server              # sync server on 127.0.0.1:8080, configured from .env (see .env.example)
npm run server                          # same server in Docker: docker compose up -d --build (PostgreSQL included)
npm run server:logs / server:stop       # follow its logs / docker compose down (data kept in a volume)
cargo clippy --workspace --all-targets  # kept at zero warnings
cargo fmt --all
npm run check:linux                     # full check + .deb + screenshot on Debian in Docker (results: target/linux-check/)
```

CI (`.github/workflows/ci.yml`) runs the same checks plus `tauri build` on Ubuntu, Windows and macOS, and uploads the installers. A separate `sync-server` job runs the server tests against a PostgreSQL service and builds the server's Docker image (`crates/hostly-server/Dockerfile`). The root `docker-compose.yml` runs PostgreSQL plus the server with zero configuration; its `https` profile adds Caddy (`deploy/Caddyfile`) for automatic TLS on `HOSTLY_DOMAIN`.

Locally, `.env` points at the owner's Homebrew PostgreSQL: `hostly_development` and `hostly_test`, with trust auth for their user. Don't touch other databases on that server.

Regenerate the app icons from `assets/logo.svg` with `npx tauri icon assets/logo.svg`. The generator also writes `src-tauri/icons/android` and `src-tauri/icons/ios`; delete them, since the app is desktop-only.

The owner usually keeps `npm run tauri dev` running. Every Rust edit rebuilds and restarts their app, and every frontend edit reloads the webview, which closes open sessions. Each rebuilt dev binary is unsigned, so macOS keychain permission prompts afterwards are expected. They don't mean saved data was lost.

## Architecture

Three layers, plus the sync server (see "Accounts and sync" below). Keep logic in the lowest layer that can hold it.

### `crates/hostly-core`

UI-independent Rust with no Tauri dependency:

- `store`: `ItemStore<T>` keeps one kind of saved item in a versioned JSON file (`{"version":1,"<field>":[…]}`), written with write-then-rename. `SnippetStore` (`snippets.json`) and `ForwardStore` (`forwards.json`) are aliases.
- `hosts`: `HostStore = ItemStore<Host>` in `hosts.json`. An item deserialized without an `id` gets a new one.
- `secrets`: the `SecretStore` trait. `Keychain` uses keyring 4 in v1 mode; `MemoryStore` is for tests.
- `known_hosts`: reads `~/.ssh/known_hosts` but never writes it. Approved keys go to the app's own `known_hosts`. A changed key is refused.
- `ssh`: `connect()` checks the host key in `ClientHandler::check_server_key` and authenticates with a password, key file or ssh-agent, returning a `Connection`. From there:
  - `Connection::open_shell` starts a PTY shell. A spawned `run_session` loop serves it, and a `SessionHandle` (an mpsc sender) handles write, resize and close.
  - `Connection::open_sftp` returns an `SftpClient`.
  - `Connection::start_forward` (`forward`) runs one local/remote/dynamic (SOCKS4/4a/5) forward on that connection and returns a `ForwardHandle`. Remote forwards receive `forwarded-tcpip` channels through `ClientHandler::server_channel_open_forwarded_tcpip`, routed over `Connection.incoming`.
  - `Connection::start_mosh` (`mosh`) execs `mosh-server`, then runs the system's `mosh-client` (`find_mosh_client`) in a local PTY.
- `session`: `SessionHandle`/`SessionEvent`, shared by SSH shells, local PTYs (`pty`, portable-pty), serial ports (`serial`, serialport without libudev) and Mosh. `ssh` re-exports them.
- `vault`: end-to-end encryption. `AccountKeys::derive` (Argon2id, salt from the email) gives the login key and wrapping key; `VaultKey` wraps itself and seals/opens secrets (XChaCha20-Poly1305, bound to the host id).
- `sftp`, `local_fs`, `files`: remote and local listings share the `FileEntry` type, plus recursive transfers.

### `src-tauri`

A thin command layer, registered in `lib.rs` `generate_handler!`:

- `commands.rs`: hosts, secrets, host-key trust and terminal sessions (`connect`, which picks Mosh for `host.mosh`; `open_local`; `open_serial`). `SessionSink` streams any session's events and tracks it.
- `library.rs`: snippet and forwarding-rule CRUD. `forwarding.rs`: running rules, one `Run` per rule with a run id so a stopped run's late events change nothing; `forward-status` events.
- `files.rs`: SFTP, local files and transfers.
- `state.rs`: `AppState` holds the host, snippet and forward stores, the secret store, and live sessions, SFTP clients, transfer cancel flags and forwarding runs, each in a map keyed by UUID. `AppState::open` builds it for a data directory (tests use it too).
- `connect_request()` resolves credentials for both `connect` and `sftp_open`. A `secret` argument overrides the keychain.
- Capabilities grant only `core:default`, `withGlobalTauri` is off, and a CSP is set in `tauri.conf.json`.

### `src/`

The UI:

- `api.ts` wraps every command with typed arguments in camelCase. Rust snake_case parameters map to camelCase automatically.
- `sidebar.ts`: the left sidebar's sections (`hosts.ts`, `snippets.ts`, `forwards.ts`), which share the "+" button and the search box.
- `workspace/workspace.ts`: each tab owns a `Layout`. The tab bar's "+" (`menu.ts`) opens local and serial (`serial.ts`) terminals.
- `workspace/layout.ts`: a split tree whose leaves are panes. Re-rendering moves existing pane elements rather than recreating them.
- `workspace/tab-bar.ts`: drag to reorder tabs, or below the strip to hand the tab to the workspace. Tabs activate on click, not on press, so the visible tab stays put while another is dragged onto it.
- `panes/` implements the `Pane` interface (`terminal.ts`, `sftp.ts` with `file-browser.ts`). Panes report back through `PaneEvents`. A `TerminalPane` has a `TerminalTarget`: an SSH host, `local` or `serial`; `pane.host` is set only for SSH.
- `search-panel.ts`: the right-hand Find panel (⌘F / Ctrl+Shift+F, Esc closes). It follows the active pane via `WorkspaceHooks.activePaneChanged` and searches the pane's `search` (`panes/terminal-search.ts`), which wraps `@xterm/addon-search` for highlights and stepping, and lists matching lines itself. Decorations need `allowProposedApi`.

### Cross-cutting contracts

- **Streaming terminal I/O.** Output goes through a `tauri::ipc::Channel` as `InvokeResponseBody::Raw`, which the UI always receives as an `ArrayBuffer`. A second channel carries the `closed` status. Input goes back through the `write` command as a byte array.
- **Error kinds drive the UI.**
  - Core `Error::kind()` returns stable strings. Tauri wraps them as `CommandError { kind, message, hostKey? }`.
  - `src/connect.ts` `connectWithRecovery` (used by both terminal and SFTP) handles recovery:
    - `unknownHostKey`: the user confirms the fingerprint, then `trust_host_key` is called and the connection retried.
    - `passwordRequired`, `passphraseRequired`, and `authFailed` for password hosts: `promptCredential` asks, then retries. On success the secret is saved through `set_host_secret` if "Save in keychain" was checked.
  - A new error variant needs a `kind()` entry. If it's recoverable, it also needs a case in `recover()`.
- **Secrets.**
  - Never stored in `hosts.json` or any other app file in the clear. `sync.json` only holds sealed ones (the inbox).
  - Keychain service = the app identifier `app.hostly.desktop`; accounts `host/<uuid>`, plus `sync/session` and `sync/vault-key` while logged in. Host secrets are stored as typed, so the editor's eye button can show them; the owner wants that.
  - Every change to a host secret goes through `commands::store_secret`: keychain write, then `sync.record(Kind::Secret, …)`, under `AppState.secret_writes` so a secret arriving from sync can't overwrite a newer one.
  - `save_host`'s `secret` argument: omitted keeps the saved one, `""` deletes it, anything else replaces it.
  - The host editor loads the saved secret via `get_host_secret` and sends it back only if it was edited.
  - Do not change the identifier or key format: that strands users' saved passwords, as the Ferrite→Hostly rename did once. `state.rs` keeps a tested, one-time migration from `app.ferrite.desktop`.
- **App data.** Stored in `~/Library/Application Support/app.hostly.desktop/`: `hosts.json` and `known_hosts`.
- **SFTP.**
  - Each SFTP pane opens its own SSH connection.
  - Transfers are recursive. They report progress over a channel and are cancelled through an `AtomicBool` keyed by a transfer id that the UI generates.
  - Each file is written to `.<name>.hostly-part` and renamed into place on success, so a cancel or failure never destroys an existing file. The UI hides `*.hostly-part` entries.
  - During recursion, symlinked directories are skipped; links to files are followed.
  - The local side deliberately has no delete.
- **Drags use pointer events, not HTML5 drag-and-drop.** `src/drag.ts` (`trackDrag`, `DragGhost`) is shared by hosts, tabs and panes; SFTP file drags have their own. `Workspace.placementAt` decides every drop, following Termius: the nearest pane edge picks the split side. A host opens a new connection there. A tab moves its live sessions there with no reconnect (`Layout.adopt`) and the tab closes. A split pane's title bar moves the pane within the split (`Layout.move`), or onto the tab bar into its own tab. A tab can't drop on its own panes, nor a pane on itself; Escape cancels. Tauri's native drag-drop stays enabled so files dropped from Finder on the remote SFTP pane upload (`onDragDropEvent`).
- **Keyboard shortcuts.**
  - Defined once in `src/platform.ts` as combo strings and per-key labels. `main.ts` handles them in a capture-phase listener so terminals never see them, and the empty-state list and tooltips are rendered from the same table.
  - Don't bind ⌘D (the owner rejected it, being next to Ctrl+D/EOF) or ⌘V (Paste). Off macOS, shortcuts need Ctrl+Shift, because Ctrl+letter belongs to the shell.
- **Locks and the main thread.** Non-`async` commands run on the main thread, so a lock that is never released freezes the whole window. `std::sync::Mutex` isn't reentrant, and a guard created inside a struct literal or tail expression lives until that expression ends. Take each lock in its own `let` statement, and never call a helper that locks the same mutex while a guard is alive (`settings::view` once did, and hung the app on launch).
- **Untrusted text.** Render user data with `textContent`. Host-derived strings written into xterm go through `plain()` (`src/format.ts`), which strips control characters.
- **Terminal lifecycle.** A clean session end (`exit`, Ctrl+D) closes the pane, and the tab once its last pane closes. A dropped connection keeps the pane open with the error, and Enter reconnects. Terminals open xterm in `start()` only after `fontsReady`, because xterm measures glyphs when it opens.

## Accounts and sync

There are four pieces:

- **`crates/hostly-protocol`** holds the contract: the synced models (`Host`, `Snippet`, `Forward`, sealed secrets) and the `/api/v1` JSON types. Its module doc has the endpoint table and the encryption scheme. The app and the server must both go through these types.
  - Every kind syncs through `/items` (`Kind`, `ItemRecord`, `ItemChange`; data decoded with `Item::decode`). `/hosts` serves the same log limited to hosts for older apps.
  - `Credentials.key_version`: 0 = the password as typed (older apps), 1 = a login key derived on the device. `PUT /account/password` changes the login key and the wrapped vault key, and upgrades accounts from 0.
- **`crates/hostly-server`** is the self-hosted API: axum and PostgreSQL. It's configured from `.env` (see `.env.example`).
  - Log-ins are limited in `login_limits.rs`: per client address in memory, and per account in the `failed_logins` and `locked_until` columns. Each repeated lock lasts twice as long; a successful log-in resets it.
  - The client address follows `X-Forwarded-For` only through loopback or private hops. So `app()` must be served with `into_make_service_with_connect_info::<SocketAddr>()`. Tests act as different clients by sending `X-Forwarded-For` from 127.0.0.1.
  - `items.rs` holds the change log (one `items` table, sequence `item_revisions`); `hosts.rs` is the `/hosts` view of it. A derived key sent for a key-version-0 account gets 409 `plainPassword`, which doesn't count as a failure.
  - Migrations are embedded and checksummed: never edit one that has run anywhere; add a new one. `tests/migration.rs` replays them in a throwaway schema.
- **`hostly-core::sync`** is the client engine:
  - `SyncFile` (`sync.json`) holds the login, the pull cursor, `pending` (local changes the server hasn't accepted yet, keyed `kind/id`, recorded even while logged out) and `inbox` (received sealed secrets not yet in the keychain). A file older than `STATE_VERSION` merges again.
  - `sync_once` takes `Local` (the three stores) and the round's `SealedSecrets`, or `None` on a device without the vault key, which then leaves secrets alone. `sync::secrets` seals what a round needs (`wanted`, `seal`) and writes the inbox to the keychain (`deliver`); the app runs those off the async workers.
  - `sync_once` runs one round: first merge (once per login) → push → pull.
  - It never holds a lock across a network call, and discards a round if the login changed meanwhile. Lock order is `hosts`, `snippets`, `forwards`, then `sync`.
  - `ApiClient::create_account`/`open_account`/`change_password` do the key derivation; `open_account` upgrades a key-version-0 account on the spot.
- **`src-tauri/src/account.rs`** handles the app side:
  - the login commands;
  - the background loop, which runs every 15 s and on `Notify` wakes triggered by local saves and deletes (`SyncService::record`), login and `sync_now`;
  - `hosts-changed` and `sync-status` events to the UI;
  - the session token and vault key, kept in the keychain (`sync/session`, `sync/vault-key`) and read once per run;
  - `unlock_password_sync` (logs in again on a device that has no vault key yet) and `change_password`.

Which server to use:
- It's set once, in Settings (the cog icon, `#/settings`, `src/settings.ts`). It's stored by `hostly-core::settings` in `settings.json`.
- Otherwise the built-in default applies. `src-tauri/build.rs` builds it in from `HOSTLY_SERVER_URL`, taken from the environment or from the root `.env`; only that key is read.
- `sign_up`/`log_in` take just email and password and use `settings::sync_server()`, failing with `noSyncServer` if none is set.
- Changing the server is refused while logged in, because a login belongs to one server.
- Full-page views are hash routes in `src/pages.ts`: `#/account` and `#/settings`. They render into `#page` and hide the tabs.

Sync rules, which the owner asked for explicitly:

- The first merge after login never removes anything, on either side. Items are matched only by kind and id, so look-alike duplicates are all kept. Server tombstones don't delete local items, and deletes made while logged out are dropped.
- After the merge, creates, edits and deletes flow both ways. The newest `modified_at` wins; ties go to the incoming change. The server serializes one account's pushes, so `since` cursors never skip a change.
- Hosts, snippets and forwarding rules sync. Passwords and passphrases sync only sealed with the vault key; the server must never be able to read them. The vault key comes from the account password (the owner chose this over a separate passphrase); changing the password only re-wraps it.
- Logging out keeps hosts, snippets, rules and saved passwords on the device.

The engine's rules are tested with an in-memory fake server in `crates/hostly-core/src/sync/tests.rs`. The server's tests need PostgreSQL via `TEST_DATABASE_URL`, and skip themselves when it's unset. Argon2 is built optimized in the dev profile (root `Cargo.toml`); unoptimized, each log-in takes seconds.

## Platforms

- macOS, Windows and Linux share one codebase.
- Platform-specific code is cfg-gated:
  - ssh-agent access (`connect_agent` in `ssh.rs`): `SSH_AUTH_SOCK` on Unix; on Windows the OpenSSH named pipe, falling back to Pageant;
  - local file permissions (`local_fs.rs`);
  - the default local shell (`pty::default_shell`): the login `$SHELL` on Unix, `pwsh.exe` or Windows PowerShell on Windows;
  - Mosh needs a `mosh-client` on this computer (`find_mosh_client` also looks in Homebrew and MacPorts paths, since GUI apps get a minimal PATH); Windows normally has none.
- A macOS build never type-checks the `cfg(windows)` branches; only the Windows CI job does.
- Off macOS:
  - shortcuts use Ctrl+Shift (see `platform.ts`);
  - terminals copy and paste with Ctrl+Shift+C and Ctrl+Shift+V via the clipboard plugin (`TerminalPane.clipboardKey`).
- The capability file grants only `core:default` plus clipboard text read and write.
- Linux needs WebKitGTK 4.1. Saving passwords there needs a Secret Service; without one, Hostly asks for passwords every time.

## Tests

- Integration tests run against a real in-process russh server: `crates/hostly-core/tests/common/mod.rs`, using tempdirs.
- The shell test server:
  - echoes input;
  - prints `pty COLSxROWS` and `resized COLSxROWS`;
  - exits with status 7 on `exit\r`.
- Its SFTP side serves the real filesystem. Because of that, `tests/sftp.rs` is `#![cfg(unix)]`: Windows paths can't be expressed as SFTP paths.
- It also connects `direct-tcpip` channels, listens for `tcpip-forward` (the forwarding tests), and answers `mosh-server …` with a `MOSH CONNECT` line unless `mosh_installed` is cleared. Only session channels echo their input.
- `tests/local_session.rs`, `tests/mosh.rs` and `tests/serial.rs` are Unix-only; serial runs against a PTY pair.
- New SSH or SFTP behaviour belongs in `hostly-core` with an end-to-end test there. The UI is verified only by type-checking and building.

## Design

The UI follows `DESIGN.md`:

- Violet-dark surfaces: `#1B1A23` recessed, `#22212C` base, `#2A2937` cards.
- Purple `#9580FF` for actions and focus glow.
- Neon semantics: green = connected, orange = connecting, red = closed or error, pink = live activity such as transfer progress and scrollbars, cyan = links and symlinks.
- Inter and JetBrains Mono are bundled via `@fontsource` (`src/fonts.ts`); icons are lucide via `src/icons.ts`.
- Tokens live on `:root` in `src/styles.css`, and the terminal palette is in `panes/terminal.ts`.
- Machine output (addresses, sizes, dates, statuses) uses the mono font.
