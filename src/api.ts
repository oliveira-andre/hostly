// Typed wrappers around the Rust commands registered in src-tauri/src/lib.rs.
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type AuthMethod =
  | { type: "password" }
  | { type: "key"; path: string }
  | { type: "agent" };

export interface Host {
  id: string;
  label: string;
  hostname: string;
  port: number;
  username: string;
  auth: AuthMethod;
  group?: string;
  /** Connect with Mosh, bootstrapped over SSH. */
  mosh?: boolean;
}

/** A host as submitted from the editor; the backend assigns an id to new hosts. */
export type HostDraft = Omit<Host, "id"> & { id?: string };

/** A saved command to run in a terminal. */
export interface Snippet {
  id: string;
  name: string;
  script: string;
}

export type SnippetDraft = Omit<Snippet, "id"> & { id?: string };

export type ForwardKind = "local" | "remote" | "dynamic";

/** A saved port-forwarding rule, tunneled through one of the saved hosts. */
export interface Forward {
  id: string;
  label: string;
  hostId: string;
  kind: ForwardKind;
  bindAddress: string;
  bindPort: number;
  /** Local and remote rules only. */
  destinationHost?: string;
  destinationPort?: number;
}

export type ForwardDraft = Omit<Forward, "id"> & { id?: string };

/** A rule that is running, or stopped with an error. Rules not listed are stopped. */
export interface ForwardStatus {
  id: string;
  state: "starting" | "running" | "stopped" | "error";
  message: string | null;
  /** Tunneled connections open right now. */
  connections: number;
}

export interface SerialPortInfo {
  name: string;
  description: string | null;
}

export interface HostKeyInfo {
  host: string;
  port: number;
  algorithm: string;
  fingerprint: string;
  publicKey: string;
}

export interface CommandError {
  kind: string;
  message: string;
  hostKey?: HostKeyInfo;
}

export type SessionStatus = {
  type: "closed";
  exitStatus: number | null;
  error: string | null;
};

export type FileKind = "file" | "dir" | "symlink" | "other";

export interface FileEntry {
  name: string;
  path: string;
  kind: FileKind;
  size: number;
  /** Unix seconds. */
  modified: number | null;
  permissions: number | null;
}

export interface TransferProgress {
  transferred: number;
  total: number;
  currentFile: string;
}

export type TransferDirection = "upload" | "download";

export function asCommandError(error: unknown): CommandError {
  if (typeof error === "object" && error !== null && "kind" in error && "message" in error) {
    return error as CommandError;
  }
  return { kind: "internal", message: String(error) };
}

// Hosts

export const listHosts = () => invoke<Host[]>("list_hosts");

/** `secret`: undefined keeps the saved one, "" deletes it, anything else replaces it. */
export const saveHost = (host: HostDraft, secret?: string) =>
  invoke<Host>("save_host", { host, secret });

export const deleteHost = (id: string) => invoke<void>("delete_host", { id });

/** The saved password or key passphrase, read from the system keychain. */
export const getHostSecret = (id: string) => invoke<string | null>("get_host_secret", { id });

export const setHostSecret = (id: string, secret: string) =>
  invoke<void>("set_host_secret", { id, secret });

// Snippets and port forwards

export const listSnippets = () => invoke<Snippet[]>("list_snippets");

export const saveSnippet = (snippet: SnippetDraft) => invoke<Snippet>("save_snippet", { snippet });

export const deleteSnippet = (id: string) => invoke<void>("delete_snippet", { id });

export const listForwards = () => invoke<Forward[]>("list_forwards");

export const saveForward = (forward: ForwardDraft) => invoke<Forward>("save_forward", { forward });

/** Stops the rule first if it is running. */
export const deleteForward = (id: string) => invoke<void>("delete_forward", { id });

/** Resolves once the rule is listening. `secret` overrides the host's saved one. */
export const startForward = (id: string, secret?: string) =>
  invoke<void>("start_forward", { id, secret });

export const stopForward = (id: string) => invoke<void>("stop_forward", { id });

export const forwardStatuses = () => invoke<ForwardStatus[]>("forward_statuses");

export const onForwardStatus = (handler: (status: ForwardStatus) => void) =>
  listen<ForwardStatus>("forward-status", (event) => handler(event.payload));

export const trustHostKey = (key: HostKeyInfo) =>
  invoke<void>("trust_host_key", { host: key.host, port: key.port, publicKey: key.publicKey });

// Terminal sessions

export interface SessionCallbacks {
  onData(bytes: Uint8Array): void;
  onStatus(status: SessionStatus): void;
}

/** Opens a shell and resolves to its session id. Output may arrive before it resolves. */
export function connect(
  hostId: string,
  size: { cols: number; rows: number },
  secret: string | undefined,
  callbacks: SessionCallbacks,
) {
  return invoke<string>("connect", { hostId, ...size, secret, ...sessionChannels(callbacks) });
}

function sessionChannels(callbacks: SessionCallbacks) {
  const onData = new Channel<ArrayBuffer>();
  onData.onmessage = (buffer) => callbacks.onData(new Uint8Array(buffer));
  const onStatus = new Channel<SessionStatus>();
  onStatus.onmessage = (status) => callbacks.onStatus(status);
  return { onData, onStatus };
}

/** Starts the user's shell on this computer; resolves to its session id. */
export const openLocal = (size: { cols: number; rows: number }, callbacks: SessionCallbacks) =>
  invoke<string>("open_local", { ...size, ...sessionChannels(callbacks) });

export const listSerialPorts = () => invoke<SerialPortInfo[]>("list_serial_ports");

/** Opens a serial port as a terminal session (8N1); resolves to its session id. */
export const openSerial = (path: string, baudRate: number, callbacks: SessionCallbacks) =>
  invoke<string>("open_serial", { path, baudRate, ...sessionChannels(callbacks) });

export const write = (sessionId: string, bytes: Uint8Array) =>
  invoke<void>("write", { sessionId, data: Array.from(bytes) });

export const resize = (sessionId: string, cols: number, rows: number) =>
  invoke<void>("resize", { sessionId, cols, rows });

export const disconnect = (sessionId: string) => invoke<void>("disconnect", { sessionId });

// SFTP and local files

export const sftpOpen = (hostId: string, secret?: string) =>
  invoke<{ sftpId: string; home: string }>("sftp_open", { hostId, secret });

export const sftpClose = (sftpId: string) => invoke<void>("sftp_close", { sftpId });

export const sftpList = (sftpId: string, path: string) =>
  invoke<FileEntry[]>("sftp_list", { sftpId, path });

export const sftpMkdir = (sftpId: string, path: string) =>
  invoke<void>("sftp_mkdir", { sftpId, path });

export const sftpRename = (sftpId: string, from: string, to: string) =>
  invoke<void>("sftp_rename", { sftpId, from, to });

/** Removes directories recursively. */
export const sftpRemove = (sftpId: string, path: string) =>
  invoke<void>("sftp_remove", { sftpId, path });

export const localHome = () => invoke<string>("local_home");

export const localList = (path: string) => invoke<FileEntry[]>("local_list", { path });

export const localMkdir = (path: string) => invoke<void>("local_mkdir", { path });

export const localRename = (from: string, to: string) => invoke<void>("local_rename", { from, to });

/** Copies `source` (file or directory) into `targetDir`; resolves when finished. */
export function transfer(
  sftpId: string,
  transferId: string,
  direction: TransferDirection,
  source: string,
  targetDir: string,
  onProgress: (progress: TransferProgress) => void,
) {
  const channel = new Channel<TransferProgress>();
  channel.onmessage = onProgress;
  return invoke<void>("transfer", {
    sftpId,
    transferId,
    direction,
    source,
    targetDir,
    onProgress: channel,
  });
}

export const cancelTransfer = (transferId: string) =>
  invoke<void>("cancel_transfer", { transferId });

// Account and sync

export interface Account {
  id: string;
  email: string;
}

export interface SyncStatus {
  state: "loggedOut" | "syncing" | "synced" | "error";
  /** Unix milliseconds. */
  lastSyncedAt: number | null;
  message: string | null;
}

export interface AccountStatus {
  server: string | null;
  account: Account | null;
  sync: SyncStatus;
  /** This device holds the vault key, so saved passwords sync end-to-end encrypted. */
  passwordSync: boolean;
}

export const accountStatus = () => invoke<AccountStatus>("account_status");

/** Creates an account on the sync server from Settings and logs in. */
export const signUp = (email: string, password: string) =>
  invoke<Account>("sign_up", { email, password });

/** Logs in to the sync server from Settings. */
export const logIn = (email: string, password: string) =>
  invoke<Account>("log_in", { email, password });

/** Logs in again on a device logged in before end-to-end encryption, so passwords sync too. */
export const unlockPasswordSync = (password: string) =>
  invoke<Account>("unlock_password_sync", { password });

/** Saved passwords stay readable on every device; only the account password changes. */
export const changePassword = (current: string, newPassword: string) =>
  invoke<void>("change_password", { current, new: newPassword });

/** Stops syncing; hosts, snippets, forwards and passwords stay on this device. */
export const logOut = () => invoke<void>("log_out");

export const syncNow = () => invoke<void>("sync_now");

/** The host list changed because of a sync. */
export const onHostsChanged = (handler: () => void) => listen("hosts-changed", () => handler());

export const onSnippetsChanged = (handler: () => void) =>
  listen("snippets-changed", () => handler());

export const onForwardsChanged = (handler: () => void) =>
  listen("forwards-changed", () => handler());

export const onSyncStatus = (handler: (status: SyncStatus) => void) =>
  listen<SyncStatus>("sync-status", (event) => handler(event.payload));

// Settings

export interface SettingsView {
  /** Chosen in Settings; null means the built-in default is used. */
  syncServer: string | null;
  /** HOSTLY_SERVER_URL from .env when the app was built. */
  defaultSyncServer: string | null;
  /** What logging in uses. */
  effectiveSyncServer: string | null;
}

export const getSettings = () => invoke<SettingsView>("get_settings");

/** null or "" returns to the default. Fails with kind "loggedIn" while logged in. */
export const setSyncServer = (server: string | null) =>
  invoke<SettingsView>("set_sync_server", { server });

/** Resolves if `server` is a reachable Hostly sync server. */
export const checkSyncServer = (server: string) => invoke<void>("check_sync_server", { server });
