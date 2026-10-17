# Hostly

<img src="assets/logo.svg" width="96" alt="Hostly logo: a terminal prompt in a cloud">

A desktop SSH client in Rust, in the spirit of Termius: saved hosts, tabbed
terminals with splits, SFTP, port forwarding, snippets, local and serial
terminals, Mosh, and secrets kept in the OS keychain, synced end-to-end
encrypted through your own server.

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
npm run server          # docker compose up -d --build: PostgreSQL + API, published on HOSTLY_PUBLISH
npm run server:logs     # follow the server's logs
npm run server:stop     # stop it; the data stays in a Docker volume
```

**Without Docker**, using your own PostgreSQL:

```sh
createdb hostly_development          # once, with PostgreSQL running
cp .env.example .env                 # once; set DATABASE_URL to that database, and HOSTLY_BIND
cargo run -p hostly-server           # applies migrations, listens on HOSTLY_BIND
```

Check either with `curl http://<address>/health`, using the address from `HOSTLY_PUBLISH` or
`HOSTLY_BIND`; it should answer `ok`. With the values from `.env.example` both use the same port,
so run only one of them.

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

These builds aren't signed, so after each update macOS asks again for keychain access to every
saved password. For the copy you'll use every day, follow
[Deploy to production](#deploy-to-production).

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
- **macOS asks for Keychain access after a rebuild:** development builds are unsigned, so macOS treats each rebuild as a new app. Choose **Always Allow**. Your saved passwords are not lost. A signed release build asks only once; see [Deploy to production](#deploy-to-production).

## Deploy to production

These steps build a signed release of the app, install it, and point it at your hosted sync
server. They're for macOS; [Windows and Linux](#windows-and-linux) follow at the end.

### 1. Deploy the sync server (optional)

Skip this if you don't sync between devices. Otherwise deploy the server first, as in
[Sync server → Deploy](#deploy), and check that `curl https://<HOSTLY_DOMAIN>/health` answers
`ok`, with the domain from the server's `.env`.

### 2. Build your server into the app

In `.env` at the repository root, set `HOSTLY_SERVER_URL` to your server, the same domain as
`HOSTLY_DOMAIN` in the server's `.env`:

```sh
HOSTLY_SERVER_URL=https://<HOSTLY_DOMAIN>
```

The build makes it the app's default server. A Mac that already chose a server in Settings keeps
that one. Skip this if you don't sync.

### 3. Set the version

Raise `version` in `src-tauri/tauri.conf.json`, for example to `0.2.0`. The installer is named
after it (`Hostly_0.2.0_aarch64.dmg`). Keep `package.json` and `src-tauri/Cargo.toml` at the same
version.

### 4. Run the checks

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
npx tsc --noEmit
```

All three should pass before you build.

### 5. Find your signing identity

macOS remembers **Always Allow** on a keychain prompt for one app signature. An unsigned build
counts as a new app after every update, so macOS asks again for each saved password. A build
signed with your certificate keeps those answers across updates.

List the code-signing certificates on this Mac:

```sh
security find-identity -v -p codesigning
```

```
  1) 0123456789ABCDEF0123456789ABCDEF01234567 "Apple Development: Your Name (ABCDE12345)"
     1 valid identities found
```

The quoted text is your signing identity. If the list is empty, create one in Xcode: **Xcode →
Settings → Accounts**, add your Apple ID, then **Manage Certificates… → + → Apple Development**. A
free Apple ID is enough.

An Apple Development certificate is right for your own Macs. To hand Hostly to other people, use a
**Developer ID Application** certificate (paid Apple Developer Program) and notarize the app; see
Tauri's [macOS signing guide](https://tauri.app/distribute/sign/macos/).

### 6. Build the signed app

Pass the identity in the environment. Don't put it in `tauri.conf.json`: that file is committed,
and CI builds with it on machines that don't have your certificate.

```sh
APPLE_SIGNING_IDENTITY="Apple Development: Your Name (ABCDE12345)" npm run tauri build
```

This takes several minutes. macOS may ask to let `codesign` use your key; choose **Always Allow**.
A warning that notarization was skipped is expected. The results:

- `target/release/bundle/dmg/Hostly_<version>_aarch64.dmg` (`x64` instead of `aarch64` on Intel Macs)
- `target/release/bundle/macos/Hostly.app`

### 7. Check the signature

```sh
codesign -dv --verbose=2 target/release/bundle/macos/Hostly.app 2>&1 | grep -E "^(Authority|TeamIdentifier|Signature)="
```

Expect `Authority=Apple Development: Your Name (ABCDE12345)` and `TeamIdentifier=ABCDE12345`.
`Signature=adhoc` means the build isn't signed: check the identity's spelling and build again.

### 8. Install it

1. Quit `npm run tauri dev` if it's running. The installed app uses the same data folder and
   keychain entries as the dev build, so don't run both at once.
2. Open the `.dmg` and drag **Hostly** onto **Applications**.
3. Start Hostly from Applications or Spotlight.

It opens with the hosts, snippets, rules, saved passwords and sync login you already had.

### 9. Answer the keychain prompts once

The dev build saved your passwords, so macOS asks the first time the new app reads each one: the
sync login when Hostly starts, and a host's password the first time you connect to it or open its
editor. Choose **Always Allow** and enter your Mac password. That item won't ask again, including
after updates signed with the same identity.

### 10. Ship an update

1. Raise the version (step 3) and run the checks (step 4).
2. Build with the same identity (step 6).
3. Quit Hostly, open the new `.dmg`, and drag **Hostly** onto **Applications**, choosing
   **Replace**.

The keychain prompts don't come back. A different identity, or a build without one, is a new app
to macOS, which then asks once more per saved password.

### Opening it on another Mac

A build opens normally on the Mac that built it. Copied or downloaded to another Mac, it's blocked
on first launch, because it isn't notarized. Try to open it once, then go to **System Settings →
Privacy & Security** and choose **Open Anyway**.

### Windows and Linux

Build on that OS with `npm run tauri build` after installing its
[prerequisites](#1-install-the-prerequisites-once). Or download the installers from a run of the
**CI** workflow on GitHub, under **Artifacts** (`hostly-Windows`, `hostly-Linux`). CI builds have
no default sync server built in, so choose yours in Settings.

- **Windows:** run the `.msi` or the `-setup.exe`. They aren't signed, so SmartScreen warns: choose
  **More info → Run anyway**. Saved passwords go to Credential Manager, which doesn't ask per app.
- **Linux:** install the `.deb` with `sudo apt install ./Hostly_<version>_amd64.deb`, or the `.rpm`
  with `sudo dnf install ./Hostly-<version>-1.x86_64.rpm`, or make the `.AppImage` executable and
  run it. Saving passwords needs a Secret Service such as GNOME Keyring or KWallet.

## Run only the app against your hosted server

The app and the sync server are separate. Once the server runs somewhere (see [Deploy](#deploy)),
your computer only needs the app: no PostgreSQL and no Docker.

1. Install the prerequisites from step 1 (except PostgreSQL), then start the app as in step 2.
   An installer built with `npm run tauri build` works the same way.
2. Point the app at your server, either:
   - **In the app:** click the cog icon, enter `https://<HOSTLY_DOMAIN>`, choose **Test
     connection**, then **Save**. This device remembers it.
   - **Built in:** set `HOSTLY_SERVER_URL=https://<HOSTLY_DOMAIN>` in `.env` before
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
| Find in terminal / new local terminal | ⌘F / ⌘T | Ctrl+Shift+F / Ctrl+Shift+T | Ctrl+Shift+F / Ctrl+Shift+T |
| Local terminal shell | your login `$SHELL` | `pwsh`, else Windows PowerShell | your login `$SHELL` |
| Mosh | `brew install mosh` | not available | `apt install mosh` |
| Serial ports | `/dev/cu.*` | `COM*` | `/dev/ttyUSB*`, `/dev/ttyACM*`; your user needs the `dialout` group |

On Linux without a Secret Service, Hostly still connects; it just asks for passwords every time
instead of saving them. The SFTP end-to-end tests run on Unix only. Their test server serves real
OS paths, which Windows can't express as SFTP paths. The client code itself is the same on every
platform.

## Layout

| Path | What lives there |
|---|---|
| `crates/hostly-core` | UI-independent Rust: saved hosts, snippets and port-forwarding rules, keychain secrets, known_hosts trust, SSH sessions (russh), SFTP, port forwarding, local PTYs, serial ports, Mosh, sync and its encryption |
| `crates/hostly-protocol` | The synced models and the sync server's JSON API, shared by the app and the server |
| `crates/hostly-server` | The optional self-hosted sync server (axum and PostgreSQL) |
| `src-tauri` | Thin Tauri shell exposing the core as commands; streams terminal bytes over IPC channels |
| `src` | TypeScript UI: sidebar (hosts, snippets, port forwarding), editors, xterm.js tabs, SFTP, account and settings pages |

App data is stored in the per-user app data directory
(`~/Library/Application Support/app.hostly.desktop` on macOS):

- `hosts.json`, `snippets.json` and `forwards.json` hold hosts, snippets and port-forwarding rules. They contain no secrets.
- `known_hosts` holds host keys you approved in Hostly. `~/.ssh/known_hosts` is also honoured, but Hostly never writes to it.
- `sync.json` holds the login, the sync position and changes waiting to sync; `settings.json` the sync server.
- Passwords and key passphrases are stored in the system keychain under the service `app.hostly.desktop`, readable as typed so the host editor can show them. So are the sync session token and the account's vault key while you're logged in.

## Behaviour

- **Connections:** clicking a host opens a new connection every time, even when one to that host is already open.
- **Sidebar:** switch between **Hosts**, **Snippets** and **Forwarding** at the top. The **+** button and the search box act on the section you're in.
- **Tabs and splits:** drag tabs to reorder them, and scroll the mouse wheel over the tab bar to scroll it sideways. Split the active pane right (⌘G) or down (⇧⌘G); the new pane connects to the same host. To put **different hosts side by side**, drag a host from the sidebar onto a pane: the nearest edge decides where it opens (left/right side by side, top/bottom stacked), and dropping on the tab bar opens it in a new tab. **Drag an open tab onto a pane** the same way to join it into that split; its session moves over without reconnecting. Drag a pane's title bar to move it elsewhere in the split, or onto the tab bar to give it its own tab again. Escape cancels a drag. Or ⌥-click a host to open it beside the current pane (⇧⌥-click opens it below). Drag the dividers to resize panes. Switch tabs with ⇧⌘] and ⇧⌘[. Off macOS the shortcuts are Ctrl+Shift+G, Ctrl+Shift+H and Ctrl+PageDown/PageUp.
- **SFTP:** Local files are shown on the left and the host's files on the right. Double-click a folder to open it. Transfer files by dragging them between the panes, with the Upload/Download buttons, or by dropping files from Finder onto the remote pane. Folders transfer recursively; existing files are overwritten. You can create folders and rename files on both sides. Delete is only available for remote files.
- **Host keys:** the first connection shows the key's SHA-256 fingerprint for approval. If a host's key later changes, Hostly refuses to connect.
- **Authentication:** password, private key file (with or without a passphrase), or SSH agent (`SSH_AUTH_SOCK`). If a password or passphrase is missing, Hostly asks for it and uses it for that connection only. The host editor loads the saved secret masked; the eye button reveals it so you can select and copy it.
- **Ended sessions:** logging out (`exit` or Ctrl+D) closes the pane, and the tab once its last pane closes. If the connection drops, the pane stays open with the error; press Enter to reconnect.
- **Find in terminal:** ⌘F (Ctrl+Shift+F off macOS) opens a panel on the right that searches the active terminal, scrollback included. Enter steps to older matches, Shift+Enter to newer ones, and clicking a listed line jumps to it. It can match case and use regular expressions. Escape closes it.
- **Snippets:** saved commands. Click one to type it into the active terminal and press Enter; the paste button types it without running it. Multi-line scripts are pasted as one block, so shells with bracketed paste run them together.
- **Port forwarding:** rules that tunnel through a saved host: **local** (`ssh -L`, reach a service behind the host from this computer), **remote** (`ssh -R`, let the host reach a service on this computer) and **dynamic** (`ssh -D`, a SOCKS4/SOCKS5 proxy). Click a rule to start or stop it. Each running rule has its own SSH connection; its dot turns green while it listens, the pink badge counts open tunnels, and a red dot shows why it stopped. A tunnel whose destination refuses closes on its own, and the rule keeps running. Rules don't start by themselves when Hostly starts.
- **Local terminals:** the **+** at the end of the tab bar, or ⌘T (Ctrl+Shift+T), opens your login shell on this computer in a tab. It splits like any terminal.
- **Serial ports:** **+** → **Serial port…** lists the connected ports; pick one and a speed (8N1, no flow control) to open it as a terminal.
- **Mosh:** tick **Connect with Mosh** in the host editor. Hostly logs in over SSH with the host's saved credentials, starts `mosh-server` there, and runs the `mosh-client` installed on this computer. Both machines need Mosh installed (`brew install mosh`, `apt install mosh`); Windows has no `mosh-client`.
- **Accounts and sync** are optional; Hostly works fully without logging in.
  - **The account page:** the profile icon at the top right opens `#/account`. Logged out, it offers log in or create account against your own [sync server](#sync-server). Logged in, it shows the account, its sync status, password sync, **Change password**, **Sync now** and **Log out**.
  - **What syncs:** hosts, snippets, port-forwarding rules, and saved passwords and key passphrases.
  - **The first sync after logging in removes nothing.** Items only on this device are uploaded and the account's items are downloaded. Items are matched by their internal id, never by name or address, so look-alike duplicates are all kept on both sides. For a password both sides have, the account's wins.
  - **Afterwards, changes flow both ways.** Creating, editing or deleting syncs within seconds. Other devices' changes arrive every 15 seconds and whenever the window regains focus. If two devices edit the same item, the latest edit wins.
  - **Passwords sync end-to-end encrypted** with a key derived from your account password, which never leaves your devices. The server stores only ciphertext it can't decrypt. On each device they're decrypted into the system keychain, so the host editor's eye button still shows them and you can copy them.
  - **Changing the account password** keeps every saved password readable, and your other devices stay logged in. There is no reset: if you forget the password and no device is logged in, the synced passwords can't be recovered from the server (each device keeps its own copies).
  - **A device logged in before password sync** keeps syncing everything else; its account page asks for the password once to turn password sync on. An account created by an older Hostly is upgraded the first time this version logs in; older versions must then be updated to log in again.
  - **Logging out** stops syncing; hosts, snippets, rules and saved passwords stay on this device.
- **Look:** follows `DESIGN.md`, with Inter and JetBrains Mono bundled locally.

## Sync server

`crates/hostly-server` is the optional API that syncs hosts, snippets, port-forwarding rules and
saved passwords between your devices. It holds accounts, login sessions, and each account's items
in PostgreSQL. Hostly works fully without it.

**The server can't read your passwords.** The app derives two keys from your account password: a
login key, which it sends instead of the password, and an encryption key, which never leaves your
devices. Saved passwords and key passphrases reach the server only sealed with the account's vault
key (XChaCha20-Poly1305), which the server stores only wrapped with that encryption key. Host
details such as label, hostname, port, username, auth type, key path and group, and snippets and
rules, are stored as sent.

Older apps keep working against this server for hosts (through `/hosts`) until their account is
upgraded by logging in with this version; after that they need updating to log in.

### Configuration

Everything is read from environment variables or from `.env` at the repository root. Copy
`.env.example` to `.env`; `.env` is git-ignored. The server itself has no built-in address or
database: it refuses to start without `DATABASE_URL` and `HOSTLY_BIND`. A container has no `.env`,
so in Docker those reach the server through `docker-compose.yml` and the Dockerfile.

| Variable | Used by | Meaning |
|---|---|---|
| `HOSTLY_SERVER_URL` | app build | The app's default sync server, built in by `npm run tauri dev`/`build`: `https://<HOSTLY_DOMAIN>` for a hosted server. You can change it in Settings (cog icon). |
| `DATABASE_URL` | server | Required. `postgres://USER:PASSWORD@HOST:PORT/DATABASE`. Append `?sslmode=require` for hosted PostgreSQL. From a container, PostgreSQL on the host machine is at `host.docker.internal`. |
| `HOSTLY_BIND` | server | Required. Listen address, `IP:PORT`. `127.0.0.1` accepts only this machine; inside a container use `0.0.0.0`. The Dockerfile sets it for the image. |
| `RUST_LOG` | both | Log filter. Default `hostly_server=info,tower_http=info`. |
| `TEST_DATABASE_URL` | tests | A disposable database for `cargo test -p hostly-server`. When it's unset, the database tests are skipped. |
| `POSTGRES_USER` / `POSTGRES_PASSWORD` / `POSTGRES_DB` | compose | The bundled database. Each defaults to `hostly`; **set a strong password on a public server**. Unused with PostgreSQL on the host machine. |
| `HOSTLY_PUBLISH` | compose | Where the API is published on the machine running compose. |
| `HOSTLY_DOMAIN` | compose | Your domain, for automatic HTTPS with `--profile https`. |

### Run it locally

See [How to run → step 3](#3-start-the-sync-server-optional). With Docker it's `npm run server`;
without, it's `cargo run -p hostly-server` with your own PostgreSQL.

### Deploy

On the machine that will host the API, with Docker installed and this repository checked out.
PostgreSQL either runs in Docker next to the API (the bundled one), or is already installed on
that machine.

**With the bundled PostgreSQL:**

1. Create `.env` with a strong database password and your domain:
   ```sh
   POSTGRES_PASSWORD=<long random URL-safe password>
   HOSTLY_DOMAIN=<your domain>
   ```
2. Point the domain's DNS at that machine and open ports 80 and 443.
3. Start PostgreSQL, the API and Caddy, which gets and renews the HTTPS certificate automatically:
   ```sh
   docker compose --profile https up -d --build
   ```
4. Check it from anywhere with `curl https://<HOSTLY_DOMAIN>/health`, which should answer `ok`.
   Then point the apps at `https://<HOSTLY_DOMAIN>` (see
   [Run only the app](#run-only-the-app-against-your-hosted-server)).

**With PostgreSQL on the host machine:**

1. Create the database and its user:
   ```sh
   sudo -u postgres createuser --pwprompt hostly
   sudo -u postgres createdb --owner hostly hostly
   ```
2. On Linux, let containers reach PostgreSQL. They connect from Docker's networks, not from
   localhost. Docker Desktop (macOS, Windows) already forwards to the host's localhost, so skip this
   step there.
   - In `postgresql.conf`, set `listen_addresses = '*'`, and keep port 5432 closed in the firewall.
   - In `pg_hba.conf`, allow Docker's address range:
     ```
     host  hostly  hostly  172.16.0.0/12  scram-sha-256
     ```
   - Restart PostgreSQL: `sudo systemctl restart postgresql`.
3. Create `.env` with the database and your domain. Inside the container, `localhost` would be the
   container itself, so the host is `host.docker.internal`:
   ```sh
   DATABASE_URL=postgres://hostly:<password>@host.docker.internal:5432/hostly
   HOSTLY_DOMAIN=<your domain>
   ```
4. Check that `docker-compose.yml` hands that to the `server` service, with no `depends_on` on the
   bundled `postgres`:
   ```yaml
   server:
     environment:
       DATABASE_URL: ${DATABASE_URL}
     extra_hosts:
       - "host.docker.internal:host-gateway"   # Linux; harmless elsewhere
   ```
5. Point the domain's DNS at that machine and open ports 80 and 443.
6. Start only the API and Caddy, leaving the bundled PostgreSQL stopped:
   ```sh
   docker compose up -d --build server caddy
   ```
7. `docker compose logs server` should show `listening on http://0.0.0.0:<port>`.
   Then check `curl https://<HOSTLY_DOMAIN>/health` as above.

**Without Caddy**, the API is only published on `HOSTLY_PUBLISH` of that machine. Use that if you
already run a reverse proxy there. Keep plain HTTP off the public internet, because anyone who sees
the traffic could reuse the login key to read your synced hosts (though not your passwords, which
stay encrypted). Caddy forwards to the port in `deploy/Caddyfile`, so keep it the same as the
container's `HOSTLY_BIND`. The bundled database is never published. Its data lives in the
`postgres-data` volume and survives `docker compose down` (`down -v` deletes it).

You can also build the image on its own:
`docker build -f crates/hostly-server/Dockerfile -t hostly-server .` and run it with
`-e DATABASE_URL=… -p 127.0.0.1:<port>:<port>`, where `<port>` is the one in the image's
`HOSTLY_BIND`.

Migrations are embedded in the binary and run on startup. The API is documented in
`crates/hostly-protocol/src/lib.rs`.

### Log-in limits

To slow down password guessing, the server refuses log-ins for a while after too many failures:

- **Per address:** after 10 failed log-ins from one address within 15 minutes, that address can't
  log in until the 15 minutes are up. An IPv6 /64 counts as one address, and successful log-ins
  don't count. These counts live in memory, so a restart clears them.
- **Per account:** 5 failed log-ins in a row, from any addresses, lock the account for 15 minutes.
  While it's locked, even the right password is refused. If the failures go on after the lock
  ends, every 5 more lock it twice as long as before, up to 24 hours. A successful log-in resets
  the count.

The app shows how long to wait. For other clients, both answers carry a `Retry-After` header.

Behind a reverse proxy, every connection comes from the proxy, so the server takes the client's
address from `X-Forwarded-For`. It trusts that header only on connections from loopback or private
addresses. Caddy (the `https` profile) and nginx with `$proxy_add_x_forwarded_for` both set it
correctly. A client on your private network can therefore choose the address it's counted under,
but the per-account lock still applies to it.

## Roadmap

These features were identified from Termius' shipped app. They are being reimplemented independently, without copying Termius code.

- [x] Saved hosts, groups, search
- [x] SSH terminal tabs (PTY, resize, exit status)
- [x] Keychain-backed secrets, host-key verification
- [x] SFTP browser (dual pane, recursive transfers, progress, cancel)
- [x] Split panes, tab reordering
- [x] Port forwarding rules (local / remote / dynamic SOCKS)
- [x] Snippets
- [x] Local terminal tabs
- [x] Mosh, serial
- [x] Find in terminal (scrollback included)
- [x] Host sync across devices through a self-hosted server (accounts, merge without data loss)
- [x] Snippet and port-forwarding rule sync
- [x] End-to-end encrypted password sync
- [x] Login rate limiting on the sync server
