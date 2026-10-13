// Typed wrappers around the Rust commands in src-tauri/src/commands.rs.
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
}

/** A host as submitted from the editor; the backend assigns an id to new hosts. */
export type HostDraft = Omit<Host, "id"> & { id?: string };

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
  const onData = new Channel<ArrayBuffer>();
  onData.onmessage = (buffer) => callbacks.onData(new Uint8Array(buffer));
  const onStatus = new Channel<SessionStatus>();
  onStatus.onmessage = (status) => callbacks.onStatus(status);
  return invoke<string>("connect", { hostId, ...size, secret, onData, onStatus });
}

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
}

export const accountStatus = () => invoke<AccountStatus>("account_status");

/** Creates an account on the sync server from Settings and logs in. */
export const signUp = (email: string, password: string) =>
  invoke<Account>("sign_up", { email, password });

/** Logs in to the sync server from Settings. */
export const logIn = (email: string, password: string) =>
  invoke<Account>("log_in", { email, password });

/** Stops syncing; hosts stay on this device. */
export const logOut = () => invoke<void>("log_out");

export const syncNow = () => invoke<void>("sync_now");

/** The host list changed because of a sync. */
export const onHostsChanged = (handler: () => void) => listen("hosts-changed", () => handler());

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
