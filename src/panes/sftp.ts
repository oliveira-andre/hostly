// SFTP pane: local files on the left, the host's files on the right, transfers below.
import { getCurrentWebview } from "@tauri-apps/api/webview";

import * as api from "../api";
import type { CommandError, Host, TransferDirection, TransferProgress } from "../api";
import { connectWithRecovery } from "../connect";
import { displayName, element, formatSize } from "../format";
import { icon, iconButton } from "../icons";
import { FileBrowser, baseName } from "./file-browser";
import type { Pane, PaneEvents, PaneState } from "./pane";

export class SftpPane implements Pane {
  readonly kind = "sftp";
  readonly element = element("div", "sftp-pane");
  state: PaneState = "connecting";
  private sftpId?: string;
  private disposed = false;
  private unlistenFileDrop?: () => void;
  private readonly transfers = new TransferList();
  private readonly local: FileBrowser;
  private readonly remote: FileBrowser;

  constructor(
    readonly host: Host,
    private readonly events: PaneEvents,
  ) {
    this.local = new FileBrowser({
      side: "local",
      title: "Local",
      titleIcon: "hardDrive",
      source: { list: api.localList, mkdir: api.localMkdir, rename: api.localRename },
      transfer: { label: "Upload to host", icon: "upload", run: (paths) => this.transfer("upload", paths) },
    });
    this.remote = new FileBrowser({
      side: "remote",
      title: displayName(host),
      titleIcon: "server",
      source: {
        list: async (path) => api.sftpList(this.connectedId(), path),
        mkdir: async (path) => api.sftpMkdir(this.connectedId(), path),
        rename: async (from, to) => api.sftpRename(this.connectedId(), from, to),
        remove: async (path) => api.sftpRemove(this.connectedId(), path),
      },
      transfer: { label: "Download to local", icon: "download", run: (paths) => this.transfer("download", paths) },
    });

    const browsers = element("div", "sftp-browsers");
    browsers.append(this.local.element, this.remote.element);
    this.element.append(browsers, this.transfers.element);
    this.element.addEventListener("focusin", () => events.focused(this));
    this.element.addEventListener("pointerdown", () => events.focused(this));
  }

  async start() {
    api
      .localHome()
      .then((home) => this.local.navigate(home))
      .catch((error) => this.local.setMessage(api.asCommandError(error).message, true));
    void this.listenForFileDrops();
    await this.connect();
  }

  focus() {
    this.remote.focus();
  }

  private async connect() {
    this.setState("connecting");
    this.remote.setMessage("Connecting…");
    let failure = "Not connected.";
    const opened = await connectWithRecovery(
      this.host,
      (secret) => api.sftpOpen(this.host.id, secret),
      (message) => (failure = message),
    );
    if (this.disposed) {
      if (opened) void api.sftpClose(opened.sftpId);
      return;
    }
    if (!opened) {
      this.setState("closed");
      this.remote.setMessage(failure, true, { label: "Reconnect", run: () => void this.connect() });
      return;
    }
    this.sftpId = opened.sftpId;
    this.setState("connected");
    await this.remote.navigate(opened.home);
  }

  private connectedId(): string {
    if (!this.sftpId) throw { kind: "sessionClosed", message: "Not connected." } satisfies CommandError;
    return this.sftpId;
  }

  private transfer(direction: TransferDirection, paths: string[]) {
    const sftpId = this.sftpId;
    if (!sftpId || paths.length === 0) return;
    const target = direction === "upload" ? this.remote : this.local;
    const targetDir = target.path;
    for (const source of paths) {
      const transferId = crypto.randomUUID();
      void this.transfers
        .run({
          name: baseName(source),
          direction,
          start: (onProgress) => api.transfer(sftpId, transferId, direction, source, targetDir, onProgress),
          cancel: () => api.cancelTransfer(transferId),
        })
        .then((finished) => {
          if (finished && !this.disposed && target.path === targetDir) void target.refresh();
        });
    }
  }

  /** Files dragged in from the OS file manager upload when dropped on the remote side. */
  private async listenForFileDrops() {
    const unlisten = await getCurrentWebview().onDragDropEvent(({ payload }) => {
      const overRemote = payload.type !== "leave" && this.isOverRemote(payload.position);
      this.remote.element.classList.toggle("drop-target", overRemote && payload.type !== "drop");
      if (payload.type === "drop" && overRemote) this.transfer("upload", payload.paths);
    });
    if (this.disposed) unlisten();
    else this.unlistenFileDrop = unlisten;
  }

  private isOverRemote(position: { x: number; y: number }): boolean {
    if (this.element.offsetParent === null) return false;
    const hit = document.elementFromPoint(position.x / devicePixelRatio, position.y / devicePixelRatio);
    return hit !== null && this.remote.element.contains(hit);
  }

  private setState(state: PaneState) {
    this.state = state;
    this.events.stateChanged(this);
  }

  dispose() {
    this.disposed = true;
    this.transfers.cancelAll();
    this.unlistenFileDrop?.();
    if (this.sftpId) void api.sftpClose(this.sftpId);
    this.element.remove();
  }
}

interface TransferJob {
  name: string;
  direction: TransferDirection;
  start(onProgress: (progress: TransferProgress) => void): Promise<void>;
  cancel(): Promise<void>;
}

class TransferList {
  readonly element = element("section", "transfers");
  private readonly rows = element("div", "transfer-rows");
  private readonly running = new Set<() => void>();

  constructor() {
    const header = element("header", "transfers-header");
    const clear = element("button", "button-ghost small", "Clear finished");
    clear.type = "button";
    clear.addEventListener("click", () => this.clearFinished());
    header.append(element("span", "transfers-title", "Transfers"), clear);
    this.element.append(header, this.rows);
    this.element.hidden = true;
  }

  /** Resolves to true when the transfer completed. */
  async run(job: TransferJob): Promise<boolean> {
    this.element.hidden = false;
    const row = element("div", "transfer-row");
    row.dataset.state = "running";
    const name = element("span", "transfer-name", job.name);
    const detail = element("span", "transfer-detail", "Starting…");
    const bar = element("div", "progress");
    const fill = element("div", "progress-fill");
    bar.append(fill);
    const cancel = () => void job.cancel().catch(() => {});
    const cancelButton = iconButton("close", "Cancel transfer", cancel, "icon-button small");
    const text = element("div", "transfer-text");
    text.append(name, detail);
    row.append(icon(job.direction === "upload" ? "upload" : "download"), text, bar, cancelButton);
    this.rows.prepend(row);
    this.running.add(cancel);

    try {
      await job.start(({ transferred, total, currentFile }) => {
        fill.style.width = `${total ? (transferred / total) * 100 : 0}%`;
        const file = currentFile && baseName(currentFile) !== job.name ? ` · ${baseName(currentFile)}` : "";
        detail.textContent = `${formatSize(transferred)} / ${formatSize(total)}${file}`;
      });
      row.dataset.state = "done";
      fill.style.width = "100%";
      detail.textContent = job.direction === "upload" ? "Uploaded" : "Downloaded";
      return true;
    } catch (error) {
      const failure = api.asCommandError(error);
      row.dataset.state = failure.kind === "cancelled" ? "cancelled" : "failed";
      detail.textContent = failure.kind === "cancelled" ? "Cancelled" : failure.message;
      return false;
    } finally {
      this.running.delete(cancel);
      cancelButton.remove();
    }
  }

  cancelAll() {
    for (const cancel of this.running) cancel();
  }

  private clearFinished() {
    for (const row of this.rows.querySelectorAll<HTMLElement>(".transfer-row:not([data-state='running'])")) {
      row.remove();
    }
    this.element.hidden = this.rows.childElementCount === 0;
  }
}
