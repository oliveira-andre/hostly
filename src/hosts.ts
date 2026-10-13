// Sidebar host list and the add/edit host dialog.
import * as api from "./api";
import type { AuthMethod, Host, HostDraft } from "./api";
import { confirmAction, showError } from "./dialogs";
import { trackDrag } from "./drag";
import { address, displayName, element } from "./format";
import { icon, iconButton } from "./icons";
import { isMac } from "./platform";
import { type SecretField, secretField } from "./secret-input";
import type { SplitDirection } from "./workspace/layout";

const altKey = isMac ? "⌥" : "Alt";

export interface HostActions {
  connect(host: Host): void;
  /** ⌥-click: beside the active pane (⇧ stacks it below instead). */
  openBeside(host: Host, direction: SplitDirection): void;
  openSftp(host: Host): void;
  /** Dragging a host: preview where it would open, then open it there. */
  previewDrop(host: Host, x: number, y: number): void;
  drop(host: Host, x: number, y: number): void;
  cancelDrop(): void;
}

export class HostSidebar {
  private hosts: Host[] = [];
  private filter = "";
  private readonly list = document.querySelector<HTMLElement>("#host-list")!;
  private readonly editor = new HostEditor((host) => this.replace(host));

  constructor(private readonly actions: HostActions) {
    document.querySelector("#new-host")!.addEventListener("click", () => this.editor.open());
    document.querySelector("#host-filter")!.addEventListener("input", (event) => {
      this.filter = (event.target as HTMLInputElement).value.trim().toLowerCase();
      this.render();
    });
  }

  async load() {
    this.hosts = await api.listHosts();
    this.render();
  }

  private replace(host: Host) {
    const index = this.hosts.findIndex((h) => h.id === host.id);
    if (index >= 0) this.hosts[index] = host;
    else this.hosts.push(host);
    this.render();
  }

  private async remove(host: Host) {
    const confirmed = await confirmAction({
      title: "Delete host?",
      message: `${displayName(host)} and its saved secret will be removed.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!confirmed) return;
    try {
      await api.deleteHost(host.id);
      this.hosts = this.hosts.filter((h) => h.id !== host.id);
      this.render();
    } catch (error) {
      await showError("Couldn't delete host", api.asCommandError(error).message);
    }
  }

  private render() {
    const visible = this.hosts
      .filter((host) =>
        [host.label, host.hostname, host.username, host.group ?? ""].some((value) =>
          value.toLowerCase().includes(this.filter),
        ),
      )
      .sort((a, b) => displayName(a).localeCompare(displayName(b)));

    if (visible.length === 0) {
      const message = this.hosts.length === 0 ? "No hosts yet." : "No matching hosts.";
      this.list.replaceChildren(element("p", "host-list-empty", message));
      return;
    }

    const groups = new Map<string, Host[]>();
    for (const host of visible) {
      const group = host.group?.trim() ?? "";
      groups.set(group, [...(groups.get(group) ?? []), host]);
    }
    const names = [...groups.keys()].sort((a, b) => (a === "" ? -1 : b === "" ? 1 : a.localeCompare(b)));
    this.list.replaceChildren(
      ...names.flatMap((name) => [
        ...(name ? [element("h3", "host-group", name)] : []),
        ...groups.get(name)!.map((host) => this.item(host)),
      ]),
    );
  }

  /** Drags a host into the workspace; the workspace shows where it would open. */
  private trackDrag(item: HTMLElement, host: Host, down: PointerEvent, onDragStart: () => void) {
    trackDrag(item, down, {
      start: () => {
        onDragStart();
        item.classList.add("dragging");
      },
      move: (event) => this.actions.previewDrop(host, event.clientX, event.clientY),
      drop: (event) => {
        item.classList.remove("dragging");
        this.actions.drop(host, event.clientX, event.clientY);
      },
      cancel: () => {
        item.classList.remove("dragging");
        this.actions.cancelDrop();
      },
    });
  }

  private item(host: Host): HTMLElement {
    const item = element("div", "host-item");
    item.tabIndex = 0;
    item.title = `${address(host)}\nClick: new connection · ${altKey}-click: open beside the current pane · Drag onto a pane to split`;

    const avatar = element("span", "host-avatar");
    avatar.append(icon("server"));
    const text = element("div", "host-text");
    text.append(element("span", "host-name", displayName(host)), element("span", "host-address", address(host)));

    const button = (name: Parameters<typeof iconButton>[0], label: string, run: () => void) =>
      iconButton(name, label, (event) => {
        event.stopPropagation();
        run();
      }, "icon-button small");
    const actions = element("div", "host-actions");
    actions.append(
      button("folderOpen", "Open SFTP", () => this.actions.openSftp(host)),
      button("pencil", "Edit", () => this.editor.open(host)),
      button("trash", "Delete", () => void this.remove(host)),
    );

    item.append(avatar, text, actions);
    let dragged = false;
    item.addEventListener("pointerdown", (event) => {
      dragged = false;
      if (event.button === 0 && !(event.target as Element).closest("button")) {
        this.trackDrag(item, host, event, () => (dragged = true));
      }
    });
    // Every click opens another connection, even to a host that already has one.
    item.addEventListener("click", (event) => {
      if (dragged) return;
      if (event.altKey) this.actions.openBeside(host, event.shiftKey ? "column" : "row");
      else this.actions.connect(host);
    });
    item.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && event.target === item) this.actions.connect(host);
    });
    return item;
  }
}

class HostEditor {
  private readonly dialog = document.querySelector<HTMLDialogElement>("#host-dialog")!;
  private readonly form = document.querySelector<HTMLFormElement>("#host-form")!;
  private readonly error = document.querySelector<HTMLElement>("#host-form-error")!;
  private readonly secretFields: SecretField[];
  private editing?: Host;
  /** The keychain secret shown when the dialog opened; unchanged fields aren't re-saved. */
  private loadedSecret: string | null = null;
  private loadToken = 0;

  constructor(private readonly onSaved: (host: Host) => void) {
    this.secretFields = [secretField(this.input("password")), secretField(this.input("passphrase"))];
    this.input("authType").addEventListener("change", () => this.showAuthFields());
    this.form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.save();
    });
    this.form.querySelector("[data-close]")!.addEventListener("click", () => this.dialog.close());
  }

  open(host?: Host) {
    this.editing = host;
    this.loadedSecret = null;
    this.loadToken++;
    this.form.reset();
    for (const field of this.secretFields) field.conceal();
    this.error.textContent = "";
    document.querySelector("#host-dialog-title")!.textContent = host ? "Edit host" : "New host";
    if (host) {
      this.input("label").value = host.label;
      this.input("hostname").value = host.hostname;
      this.input("port").value = String(host.port);
      this.input("username").value = host.username;
      this.input("group").value = host.group ?? "";
      this.input("authType").value = host.auth.type;
      this.input("keyPath").value = host.auth.type === "key" ? host.auth.path : "";
      if (host.auth.type !== "agent") void this.loadSecret(host);
    }
    this.showAuthFields();
    this.dialog.showModal();
    this.input(host ? "label" : "hostname").focus();
  }

  /** Fills the password or passphrase from the keychain, masked until revealed. */
  private async loadSecret(host: Host) {
    const token = this.loadToken;
    const input = this.input(host.auth.type === "password" ? "password" : "passphrase");
    const placeholder = input.placeholder;
    input.placeholder = "Loading from keychain…";
    try {
      const secret = await api.getHostSecret(host.id);
      if (token !== this.loadToken) return;
      this.loadedSecret = secret;
      if (secret !== null && input.value === "") input.value = secret;
    } catch (error) {
      if (token === this.loadToken) {
        this.error.textContent = `Couldn't read the keychain: ${api.asCommandError(error).message}`;
      }
    } finally {
      input.placeholder = placeholder;
    }
  }

  private input(name: string) {
    return this.form.elements.namedItem(name) as HTMLInputElement;
  }

  private value(name: string) {
    return this.input(name).value.trim();
  }

  private showAuthFields() {
    const type = this.input("authType").value;
    for (const section of this.form.querySelectorAll<HTMLElement>("[data-auth]")) {
      section.hidden = section.dataset.auth !== type;
    }
  }

  /** undefined keeps the saved secret, "" deletes it, anything else replaces it. */
  private secretToSave(type: AuthMethod["type"]): string | undefined {
    const previous = this.editing?.auth.type;
    if (type === "agent") return previous && previous !== "agent" ? "" : undefined;
    const entered = this.input(type === "password" ? "password" : "passphrase").value;
    if (previous === type && entered === (this.loadedSecret ?? "")) return undefined;
    return entered;
  }

  private async save() {
    const type = this.value("authType") as AuthMethod["type"];
    const auth: AuthMethod = type === "key" ? { type, path: this.value("keyPath") } : { type };
    const draft: HostDraft = {
      id: this.editing?.id,
      label: this.value("label"),
      hostname: this.value("hostname"),
      port: Number(this.value("port")),
      username: this.value("username"),
      group: this.value("group") || undefined,
      auth,
    };
    try {
      this.onSaved(await api.saveHost(draft, this.secretToSave(type)));
      this.dialog.close();
    } catch (error) {
      this.error.textContent = api.asCommandError(error).message;
    }
  }
}
