// One side of the SFTP view: a navigable, selectable file list for local or remote files.
import { asCommandError, type FileEntry } from "../api";
import { confirmAction, promptText, showError } from "../dialogs";
import { element, formatDate, formatSize } from "../format";
import { type IconName, icon, iconButton } from "../icons";

export interface FileSource {
  list(path: string): Promise<FileEntry[]>;
  mkdir(path: string): Promise<void>;
  rename(from: string, to: string): Promise<void>;
  /** Absent for local files: deleting from this computer is left to the OS file manager. */
  remove?(path: string): Promise<void>;
}

export interface BrowserOptions {
  side: "local" | "remote";
  title: string;
  titleIcon: IconName;
  source: FileSource;
  /** Copies the given paths to the other side. */
  transfer: { label: string; icon: IconName; run(paths: string[]): void };
}

const DRAG_THRESHOLD_PX = 5;
const PART_SUFFIX = ".hostly-part";
const browsers = new WeakMap<Element, FileBrowser>();

function separator(path: string): string {
  return path.includes("\\") && !path.includes("/") ? "\\" : "/";
}

export function joinPath(dir: string, name: string): string {
  const sep = separator(dir);
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}

export function parentPath(path: string): string {
  const sep = separator(path);
  const trimmed = path.length > 1 && path.endsWith(sep) ? path.slice(0, -1) : path;
  const index = trimmed.lastIndexOf(sep);
  if (index < 0) return path;
  if (index === 0) return sep;
  const parent = trimmed.slice(0, index);
  // "C:\folder" → "C:\", not "C:".
  return /^[A-Za-z]:$/.test(parent) ? parent + sep : parent;
}

export function baseName(path: string): string {
  const sep = separator(path);
  const trimmed = path.length > 1 && path.endsWith(sep) ? path.slice(0, -1) : path;
  return trimmed.slice(trimmed.lastIndexOf(sep) + 1) || trimmed;
}

export class FileBrowser {
  readonly element = element("section", "file-browser");
  path = "";
  private entries: FileEntry[] = [];
  private selection = new Set<string>();
  private anchor = -1;
  private loadToken = 0;
  private readonly pathInput = element("input", "path-input");
  private readonly list = element("div", "file-list");
  private readonly status = element("div", "browser-status");
  private readonly renameButton: HTMLButtonElement;
  private readonly deleteButton?: HTMLButtonElement;
  private readonly transferButton: HTMLButtonElement;

  constructor(private readonly options: BrowserOptions) {
    browsers.set(this.element, this);
    this.element.dataset.side = options.side;

    const title = element("div", "browser-title");
    title.append(icon(options.titleIcon), element("span", "", options.title));

    const actions = element("div", "browser-actions");
    this.renameButton = iconButton("pencil", "Rename", () => void this.renameSelected());
    this.transferButton = iconButton(options.transfer.icon, options.transfer.label, () =>
      options.transfer.run([...this.selection]),
    );
    this.transferButton.classList.add("transfer-button");
    actions.append(
      iconButton("arrowUp", "Parent folder", () => void this.navigate(parentPath(this.path))),
      iconButton("refresh", "Refresh", () => void this.refresh()),
      iconButton("folderPlus", "New folder", () => void this.createFolder()),
      this.renameButton,
    );
    if (options.source.remove) {
      this.deleteButton = iconButton("trash", "Delete", () => void this.deleteSelected());
      actions.append(this.deleteButton);
    }
    actions.append(this.transferButton);

    const header = element("header", "browser-header");
    header.append(title, actions);

    this.pathInput.spellcheck = false;
    this.pathInput.setAttribute("aria-label", `${options.title} path`);
    this.pathInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter") void this.navigate(this.pathInput.value.trim());
      if (event.key === "Escape") this.pathInput.value = this.path;
    });

    this.list.tabIndex = 0;
    this.list.setAttribute("role", "listbox");
    this.list.setAttribute("aria-multiselectable", "true");
    this.list.addEventListener("keydown", (event) => this.onKey(event));

    this.element.append(header, this.pathInput, this.list, this.status);
    this.updateButtons();
  }

  focus() {
    this.list.focus();
  }

  /** Resolves to false if `path` couldn't be listed; `quiet` skips reporting that. */
  async navigate(path: string, quiet = false): Promise<boolean> {
    const token = ++this.loadToken;
    if (!this.path) this.setMessage("Loading…");
    try {
      const entries = await this.options.source.list(path);
      if (token !== this.loadToken) return true;
      this.path = path;
      // In-progress transfers write to hidden part files that are renamed into place.
      this.entries = entries.filter((entry) => !entry.name.endsWith(PART_SUFFIX));
      this.selection.clear();
      this.anchor = -1;
      this.pathInput.value = path;
      this.render();
      return true;
    } catch (error) {
      if (token !== this.loadToken || quiet) return false;
      const message = asCommandError(error).message;
      this.pathInput.value = this.path;
      // Keep the current listing visible; only an initial failure replaces it.
      if (this.path) this.setStatus(message, true);
      else this.setMessage(message, true);
      return false;
    }
  }

  async refresh(): Promise<void> {
    await this.navigate(this.path);
  }

  /** Replaces the list with a message, e.g. while connecting or after a failure. */
  setMessage(text: string, isError = false, action?: { label: string; run(): void }) {
    const message = element("div", isError ? "file-message error" : "file-message");
    message.append(element("p", "", text));
    if (action) {
      const button = element("button", "button-secondary", action.label);
      button.type = "button";
      button.addEventListener("click", action.run);
      message.append(button);
    }
    this.list.replaceChildren(message);
    this.setStatus("");
  }

  private render() {
    const header = element("div", "file-row file-columns");
    header.append(element("span", "", "Name"), element("span", "", "Size"), element("span", "", "Modified"));
    const rows = this.entries.map((entry, index) => this.row(entry, index));
    this.list.replaceChildren(header, ...(rows.length ? rows : [element("div", "file-empty", "This folder is empty.")]));
    this.paintSelection();
  }

  private row(entry: FileEntry, index: number): HTMLElement {
    const row = element("div", "file-row");
    row.setAttribute("role", "option");
    row.dataset.path = entry.path;
    if (entry.name.startsWith(".")) row.classList.add("dotfile");

    const name = element("span", "file-name");
    const kindIcon: IconName = entry.kind === "dir" ? "folder" : entry.kind === "symlink" ? "fileSymlink" : "file";
    name.append(icon(kindIcon), element("span", "", entry.name));
    name.dataset.kind = entry.kind;
    row.append(
      name,
      element("span", "file-size", entry.kind === "dir" ? "" : formatSize(entry.size)),
      element("span", "file-date", formatDate(entry.modified)),
    );
    row.title = entry.name;
    row.addEventListener("pointerdown", (event) => this.onRowPointerDown(event, index));
    row.addEventListener("dblclick", () => void this.open(entry));
    return row;
  }

  private async open(entry: FileEntry) {
    if (entry.kind === "dir") {
      await this.navigate(entry.path);
    } else if (entry.kind === "symlink") {
      // A link to a folder opens it; a link to a file transfers it.
      if (!(await this.navigate(entry.path, true))) this.options.transfer.run([entry.path]);
    } else {
      this.options.transfer.run([entry.path]);
    }
  }

  private onRowPointerDown(event: PointerEvent, index: number) {
    if (event.button !== 0) return;
    const path = this.entries[index].path;
    const wasSelected = this.selection.has(path);
    if (event.shiftKey && this.anchor >= 0) {
      const [from, to] = [Math.min(this.anchor, index), Math.max(this.anchor, index)];
      this.selection = new Set(this.entries.slice(from, to + 1).map((e) => e.path));
    } else if (event.metaKey || event.ctrlKey) {
      if (wasSelected) this.selection.delete(path);
      else this.selection.add(path);
      this.anchor = index;
    } else {
      // Pressing an already-selected row keeps the selection so it can be dragged.
      if (!wasSelected) this.selection = new Set([path]);
      this.anchor = index;
    }
    this.paintSelection();
    this.list.focus();
    if (this.selection.has(path)) this.trackDrag(event, path);
  }

  /** Drags the selection; dropping it on the other browser transfers it there. */
  private trackDrag(down: PointerEvent, pressed: string) {
    const target = down.currentTarget as HTMLElement;
    target.setPointerCapture(down.pointerId);
    let ghost: HTMLElement | undefined;
    let over: FileBrowser | undefined;

    const move = (event: PointerEvent) => {
      if (!ghost) {
        if (Math.hypot(event.clientX - down.clientX, event.clientY - down.clientY) < DRAG_THRESHOLD_PX) return;
        const count = this.selection.size;
        ghost = element("div", "drag-ghost", count === 1 ? baseName(pressed) : `${count} items`);
        document.body.append(ghost);
      }
      ghost.style.transform = `translate(${event.clientX + 12}px, ${event.clientY + 8}px)`;
      const hit = document.elementFromPoint(event.clientX, event.clientY)?.closest(".file-browser");
      const browser = hit ? browsers.get(hit) : undefined;
      const next = browser && browser.options.side !== this.options.side ? browser : undefined;
      if (next !== over) {
        over?.element.classList.remove("drop-target");
        next?.element.classList.add("drop-target");
        over = next;
      }
    };
    const end = (event: PointerEvent) => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", end);
      target.removeEventListener("pointercancel", end);
      over?.element.classList.remove("drop-target");
      if (ghost) {
        ghost.remove();
        if (over && event.type === "pointerup") this.options.transfer.run([...this.selection]);
      } else if (!event.shiftKey && !event.metaKey && !event.ctrlKey) {
        // A plain click (no drag) narrows a multi-selection to the clicked row.
        this.selection = new Set([pressed]);
        this.paintSelection();
      }
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", end);
    target.addEventListener("pointercancel", end);
  }

  private onKey(event: KeyboardEvent) {
    if (event.target !== this.list) return;
    if (event.key === "Enter") {
      const entry = this.entries.find((e) => this.selection.has(e.path));
      if (entry) void this.open(entry);
    } else if (event.key === "Backspace" && !event.metaKey) {
      void this.navigate(parentPath(this.path));
    } else if (event.key === "a" && (event.metaKey || event.ctrlKey)) {
      this.selection = new Set(this.entries.map((e) => e.path));
      this.paintSelection();
    } else {
      return;
    }
    event.preventDefault();
  }

  private paintSelection() {
    for (const row of this.list.querySelectorAll<HTMLElement>(".file-row[data-path]")) {
      const selected = this.selection.has(row.dataset.path!);
      row.classList.toggle("selected", selected);
      row.setAttribute("aria-selected", String(selected));
    }
    this.updateButtons();
    const count = this.entries.length;
    const selected = this.selection.size;
    this.setStatus(`${count} item${count === 1 ? "" : "s"}${selected ? ` · ${selected} selected` : ""}`);
  }

  private setStatus(text: string, isError = false) {
    this.status.textContent = text;
    this.status.classList.toggle("error", isError);
  }

  private updateButtons() {
    const count = this.selection.size;
    this.renameButton.disabled = count !== 1;
    if (this.deleteButton) this.deleteButton.disabled = count === 0;
    this.transferButton.disabled = count === 0;
  }

  private async createFolder() {
    const name = await promptText({
      title: "New folder",
      message: `Create a folder in ${this.path}`,
      confirmLabel: "Create",
    });
    if (name) await this.run("Couldn't create folder", () => this.options.source.mkdir(joinPath(this.path, this.validName(name))));
  }

  private async renameSelected() {
    const [path] = this.selection;
    const current = baseName(path);
    const name = await promptText({ title: "Rename", message: `Rename ${current} to:`, value: current, confirmLabel: "Rename" });
    if (name && name !== current) {
      await this.run("Couldn't rename", () => this.options.source.rename(path, joinPath(this.path, this.validName(name))));
    }
  }

  private async deleteSelected() {
    const paths = [...this.selection];
    const confirmed = await confirmAction({
      title: paths.length === 1 ? `Delete ${baseName(paths[0])}?` : `Delete ${paths.length} items?`,
      message: "Folders are deleted with everything inside them. This can't be undone.",
      confirmLabel: "Delete",
      danger: true,
    });
    if (confirmed) {
      await this.run("Couldn't delete", async () => {
        for (const path of paths) await this.options.source.remove!(path);
      });
    }
  }

  private validName(name: string): string {
    const trimmed = name.trim();
    if (!trimmed || trimmed === "." || trimmed === ".." || /[\\/]/.test(trimmed)) {
      throw new Error(`"${name}" is not a valid name.`);
    }
    return trimmed;
  }

  /** Runs a file operation, reports failures, and refreshes the listing either way. */
  private async run(failureTitle: string, operation: () => Promise<void>) {
    try {
      await operation();
    } catch (error) {
      await showError(failureTitle, error instanceof Error ? error.message : asCommandError(error).message);
    }
    await this.refresh();
  }
}
