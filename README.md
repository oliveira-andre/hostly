# Hostly

<img src="assets/logo.svg" width="96" alt="Hostly logo: a terminal prompt in a cloud">

A desktop SSH client in Rust, in the spirit of Termius: saved hosts, tabbed
terminals, and secrets kept in the OS keychain.

## How to run

### 1. Install the prerequisites (once)

- **Node.js 22 or newer.** CI uses Node 24.
- **Rust 1.88 or newer**, installed with [rustup](https://rustup.rs). Then open a new terminal, or run `source "$HOME/.cargo/env"`.
- **PostgreSQL** (tested with 17). You only need it to run the sync server (step 3).
- **System tools for your OS:**
  - **macOS:** Xcode Command Line Tools: `xcode-select --install`
  - **Windows 10/11:** [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with "Desktop development with C++". WebView2 is already installed on Windows 10/11.
  - **Linux (Debian/Ubuntu):**
    ```sh
    sudo apt install build-essential libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev patchelf
    ```

### 2. Start the app

```sh
cd hostly            # the project folder
npm install          # first time, and after dependencies change
npm run tauri dev    # opens the Hostly window
```

The first start compiles all the Rust dependencies, which takes a few minutes. After that it starts
quickly. While it's running, saving a frontend file reloads the window, and saving a Rust file
rebuilds and restarts the app. Either one closes open sessions.

### 3. Start the sync server (optional)

You only need this to sync hosts between devices. Hostly works without it.

**With Docker**, which needs no PostgreSQL and no configuration:

```sh
npm run server          # docker compose up -d --build: PostgreSQL + API on http://127.0.0.1:8080
npm run server:logs     # follow the server's logs
npm run server:stop     # stop it; the data stays in a Docker volume
```

**Without Docker**, using your own PostgreSQL:

```sh
createdb hostly_development          # once, with PostgreSQL running
cp .env.example .env                 # once; set DATABASE_URL to that database
cargo run -p hostly-server           # applies migrations, serves http://127.0.0.1:8080
```

Only run one of them, since both use port 8080. Check either with
`curl http://127.0.0.1:8080/health`, which should answer `ok`.

Then, in Hostly, click the profile icon at the top right and choose **Create account** (or
**Log in**). The app already knows the server address, from `HOSTLY_SERVER_URL` in `.env`. To use
a different server, click the **cog icon** (Settings) next to it.

### 4. Build an installer

```sh
npm run tauri build
```

Installers are written to `target/release/bundle/`:

| OS | Output |
|---|---|
| macOS | `dmg/Hostly_<version>_<arch>.dmg`, `macos/Hostly.app` |
| Windows | `msi/*.msi`, `nsis/*-setup.exe` |
| Linux | `deb/*.deb`, `rpm/*.rpm`, `appimage/*.AppImage` |

The builds aren't code-signed yet:
- **macOS:** right-click the app, choose **Open**, then confirm.
- **Windows:** in the SmartScreen warning, choose **More info → Run anyway**.

### 5. Check your changes

```sh
cargo test --workspace                  # Rust tests, including real SSH/SFTP and sync end-to-end tests
cargo clippy --workspace --all-targets  # lints
npx tsc --noEmit                        # TypeScript type-check
npm run check:linux                     # everything above on Linux, in Docker (results in target/linux-check/)
```

### Troubleshooting

- **`cargo: command not found`:** run `source "$HOME/.cargo/env"` or open a new terminal.
- **`Port 1420 is already in use`:** another `npm run tauri dev` is still running. Stop it first.
- **macOS asks for Keychain access after a rebuild:** development builds are unsigned, so macOS treats each rebuild as a new app. Choose **Always Allow**. Your saved passwords are not lost.

## Run only the app against your hosted server

The app and the sync server are separate. Once the server runs somewhere (see [Deploy](#deploy)),
your computer only needs the app: no PostgreSQL and no Docker.

1. Install the prerequisites from step 1 (except PostgreSQL), then start the app as in step 2.
   An installer built with `npm run tauri build` works the same way.
2. Point the app at your server, either:
   - **In the app:** click the cog icon, enter `https://sync.example.com`, choose **Test
     connection**, then **Save**. This device remembers it.
   - **Built in:** set `HOSTLY_SERVER_URL=https://sync.example.com` in `.env` before
     `npm run tauri dev` or `npm run tauri build`. Every app you build then defaults to that server,
     which is handy for installers you hand to your other machines. Settings can still override it.
3. Click the profile icon, then **Create account** or **Log in**.

## Platforms

Hostly runs on macOS, Windows 10/11 and Linux. To build it you need the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS. The `CI` workflow
(`.github/workflows/ci.yml`) builds, checks and tests on all three platforms and uploads unsigned
installers: `.dmg` for macOS, `.msi`/`.exe` for Windows, and `.deb`/`.rpm`/`.AppImage` for Linux.

| | macOS | Windows | Linux |
|---|---|---|---|
| Web view | WKWebView (built in) | WebView2 (preinstalled on 10/11) | WebKitGTK 4.1 |
| Saved passwords | Keychain | Credential Manager | Secret Service (GNOME Keyring, KWallet) |
| SSH agent | `SSH_AUTH_SOCK` | OpenSSH agent pipe, then Pageant | `SSH_AUTH_SOCK` |
| Copy / paste in terminal | ⌘C / ⌘V | Ctrl+Shift+C / Ctrl+Shift+V | Ctrl+Shift+C / Ctrl+Shift+V |
| Split right / down | ⌘G / ⇧⌘G | Ctrl+Shift+G / Ctrl+Shift+H | Ctrl+Shift+G / Ctrl+Shift+H |

On Linux without a Secret Service, Hostly still connects; it just asks for passwords every time
instead of saving them. The SFTP end-to-end tests run on Unix only. Their test server serves real
OS paths, which Windows can't express as SFTP paths. The client code itself is the same on every
platform.

## Layout

| Path | What lives there |
|---|---|
| `crates/hostly-core` | UI-independent Rust: hosts file, keychain secrets, known_hosts trust, SSH sessions (russh) |
| `src-tauri` | Thin Tauri shell exposing the core as commands; streams terminal bytes over IPC channels |
| `src` | TypeScript UI: host sidebar, host editor, xterm.js tabs |

App data is stored in the per-user app data directory
(`~/Library/Application Support/app.hostly.desktop` on macOS):

- `hosts.json` holds host metadata only. It contains no secrets.
- `known_hosts` holds host keys you approved in Hostly. `~/.ssh/known_hosts` is also honoured, but Hostly never writes to it.
- Passwords and key passphrases are stored in the system keychain under the service `app.hostly.desktop`.

## Behaviour

- **Connections:** clicking a host opens a new connection every time, even when one to that host is already open.
- **Tabs and splits:** drag tabs to reorder them, and scroll the mouse wheel over the tab bar to scroll it sideways. Split the active pane right (⌘G) or down (⇧⌘G); the new pane connects to the same host. To put **different hosts side by side**, drag a host from the sidebar onto a pane: the nearest edge decides where it opens (left/right side by side, top/bottom stacked), and dropping on the tab bar opens it in a new tab. **Drag an open tab onto a pane** the same way to join it into that split; its session moves over without reconnecting. Drag a pane's title bar to move it elsewhere in the split, or onto the tab bar to give it its own tab again. Escape cancels a drag. Or ⌥-click a host to open it beside the current pane (⇧⌥-click opens it below). Drag the dividers to resize panes. Switch tabs with ⇧⌘] and ⇧⌘[. Off macOS the shortcuts are Ctrl+Shift+G, Ctrl+Shift+H and Ctrl+PageDown/PageUp.
- **SFTP:** Local files are shown on the left and the host's files on the right. Double-click a folder to open it. Transfer files by dragging them between the panes, with the Upload/Download buttons, or by dropping files from Finder onto the remote pane. Folders transfer recursively; existing files are overwritten. You can create folders and rename files on both sides. Delete is only available for remote files.
- **Host keys:** the first connection shows the key's SHA-256 fingerprint for approval. If a host's key later changes, Hostly refuses to connect.
- **Authentication:** password, private key file (with or without a passphrase), or SSH agent (`SSH_AUTH_SOCK`). If a password or passphrase is missing, Hostly asks for it and uses it for that connection only. The host editor loads the saved secret masked; the eye button reveals it so you can select and copy it.
- **Ended sessions:** logging out (`exit` or Ctrl+D) closes the pane, and the tab once its last pane closes. If the connection drops, the pane stays open with the error; press Enter to reconnect.
- **Accounts and sync** are optional; Hostly works fully without logging in.
  - **The account page:** the profile icon at the top right opens `#/account`. Logged out, it offers log in or create account against your own [sync server](#sync-server). Logged in, it shows the account, its sync status, **Sync now** and **Log out**.
  - **The first sync after logging in removes nothing.** Hosts only on this device are uploaded and the account's hosts are downloaded. Hosts are matched by their internal id, never by name or address, so look-alike duplicates are all kept on both sides.
  - **Afterwards, changes flow both ways.** Creating, editing or deleting a host syncs within seconds. Other devices' changes arrive every 15 seconds and whenever the window regains focus. If two devices edit the same host, the latest edit wins.
  - **Logging out** stops syncing; the hosts stay on this device.
  - **Passwords and key passphrases are never synced.** Each device keeps its own in the system keychain.
- **Look:** follows `DESIGN.md`, with Inter and JetBrains Mono bundled locally.

## Sync server

`crates/hostly-server` is the optional API that syncs saved hosts between your devices. It holds
accounts (email and password), login sessions, and each account's hosts in PostgreSQL. Hostly works
fully without it. **Passwords and key passphrases are never synced.** They stay in each device's
keychain, and the server only stores host details such as label, hostname, port, username, auth type,
key path and group.

### Configuration

Everything is read from environment variables or from `.env` at the repository root. Copy
`.env.example` to `.env`; `.env` is git-ignored. All the settings have local defaults, so
`npm run server` works without a `.env`.

| Variable | Used by | Meaning |
|---|---|---|
| `HOSTLY_SERVER_URL` | app build | The app's default sync server, built in by `npm run tauri dev`/`build`. You can change it in Settings (cog icon). |
| `DATABASE_URL` | `cargo run` | `postgres://USER:PASSWORD@HOST:PORT/DATABASE`. Required for `cargo run`. Append `?sslmode=require` for hosted PostgreSQL. |
| `HOSTLY_BIND` | `cargo run` | Listen address. Default `127.0.0.1:8080`; the Docker image uses `0.0.0.0:8080`. |
| `RUST_LOG` | both | Log filter. Default `hostly_server=info,tower_http=info`. |
| `TEST_DATABASE_URL` | tests | A disposable database for `cargo test -p hostly-server`. When it's unset, the database tests are skipped. |
| `POSTGRES_USER` / `POSTGRES_PASSWORD` / `POSTGRES_DB` | compose | The bundled database. Each defaults to `hostly`; **set a strong password on a public server**. |
| `HOSTLY_PUBLISH` | compose | Where the API is published. Default `127.0.0.1:8080`. |
| `HOSTLY_DOMAIN` | compose | Your domain, for automatic HTTPS with `--profile https`. |

### Run it locally

See [How to run → step 3](#3-start-the-sync-server-optional). With Docker it's `npm run server`;
without, it's `cargo run -p hostly-server` with your own PostgreSQL.

### Deploy

On the machine that will host the API, with Docker installed and this repository checked out:

1. Create `.env` with a strong database password and your domain:
   ```sh
   POSTGRES_PASSWORD=<long random URL-safe password>
   HOSTLY_DOMAIN=sync.example.com
   ```
2. Point the domain's DNS at that machine and open ports 80 and 443.
3. Start PostgreSQL, the API and Caddy, which gets and renews the HTTPS certificate automatically:
   ```sh
   docker compose --profile https up -d --build
   ```
4. Check it from anywhere with `curl https://sync.example.com/health`, which should answer `ok`.
   Then point the apps at `https://sync.example.com` (see
   [Run only the app](#run-only-the-app-against-your-hosted-server)).

**Without the `https` profile**, the API is only published on `127.0.0.1:8080` of that machine.
Use that if you already run a reverse proxy there. Keep plain HTTP off the public internet, because
the app sends your account password when you log in. The database is never published. Its data
lives in the `postgres-data` volume and survives `docker compose down` (`down -v` deletes it).

You can also build the image on its own:
`docker build -f crates/hostly-server/Dockerfile -t hostly-server .` and run it with
`-e DATABASE_URL=… -p 127.0.0.1:8080:8080`.

Migrations are embedded in the binary and run on startup. The API is documented in
`crates/hostly-protocol/src/lib.rs`.

## Roadmap

These features were identified from Termius' shipped app. They are being reimplemented independently, without copying Termius code.

- [x] Saved hosts, groups, search
- [x] SSH terminal tabs (PTY, resize, exit status)
- [x] Keychain-backed secrets, host-key verification
- [x] SFTP browser (dual pane, recursive transfers, progress, cancel)
- [x] Split panes, tab reordering
- [ ] Port forwarding rules (local / remote / dynamic SOCKS)
- [ ] Snippets
- [ ] Local terminal tabs
- [ ] Mosh, serial
- [x] Host sync across devices through a self-hosted server (accounts, merge without data loss)
- [ ] End-to-end encrypted password sync
- [ ] Login rate limiting on the sync server
