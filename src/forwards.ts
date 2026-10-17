// Sidebar section: saved port-forwarding rules, started and stopped with a click.
import * as api from "./api";
import type { Forward, ForwardDraft, ForwardKind, ForwardStatus, Host } from "./api";
import { connectWithRecovery } from "./connect";
import { confirmAction, showError } from "./dialogs";
import { displayName, element } from "./format";
import { type IconName, icon, iconButton } from "./icons";
import type { SidebarSection } from "./sidebar";

const KIND_LETTER: Record<ForwardKind, string> = { local: "L", remote: "R", dynamic: "D" };

const KIND_HINT: Record<ForwardKind, string> = {
  local:
    "Connections to the port on this computer go through the host to the destination, as the host sees it. Like ssh -L.",
  remote:
    "The host listens on the port, and its connections reach the destination from this computer. Like ssh -R. Listening beyond the host's localhost needs GatewayPorts on its SSH server.",
  dynamic:
    "A SOCKS proxy on this computer: apps set to use it reach any address from the host. Like ssh -D.",
};

/** Hides the usual loopback address to keep the line short. */
function endpoint(address: string, port: number): string {
  return address === "127.0.0.1" || address === "localhost" ? String(port) : `${address}:${port}`;
}

/** "L 5433 → localhost:5432", the way ssh's -L/-R/-D options read. */
export function describeForward(rule: Forward): string {
  const bind = endpoint(rule.bindAddress, rule.bindPort);
  if (rule.kind === "dynamic") return `D SOCKS ${bind}`;
  return `${KIND_LETTER[rule.kind]} ${bind} → ${rule.destinationHost}:${rule.destinationPort}`;
}

export class ForwardList implements SidebarSection {
  readonly name = "Forwarding";
  readonly newLabel = "New port forwarding rule";
  readonly placeholder = "Search rules";
  readonly list = document.querySelector<HTMLElement>("#forward-list")!;
  private rules: Forward[] = [];
  private readonly statuses = new Map<string, ForwardStatus>();
  private filterText = "";
  private readonly editor: ForwardEditor;

  /** `hosts` lists the saved hosts a rule can tunnel through. */
  constructor(private readonly hosts: () => readonly Host[]) {
    this.editor = new ForwardEditor(hosts, (rule) => void this.saved(rule));
    void api.onForwardStatus((status) => {
      if (status.state === "stopped") this.statuses.delete(status.id);
      else this.statuses.set(status.id, status);
      this.render();
    });
  }

  create() {
    if (this.hosts().length === 0) {
      void showError("Add a host first", "A port forwarding rule tunnels through one of your saved hosts.");
      return;
    }
    this.editor.open();
  }

  filter(text: string) {
    this.filterText = text.trim().toLowerCase();
    this.render();
  }

  async load() {
    const [rules, statuses] = await Promise.all([api.listForwards(), api.forwardStatuses()]);
    this.rules = rules;
    this.statuses.clear();
    for (const status of statuses) {
      // A rule deleted on another device stops here too.
      if (rules.some((rule) => rule.id === status.id)) this.statuses.set(status.id, status);
      else void api.stopForward(status.id);
    }
    this.render();
  }

  /** Re-renders, e.g. after hosts were renamed. */
  render() {
    const hostName = (rule: Forward) => {
      const host = this.hosts().find((h) => h.id === rule.hostId);
      return host ? displayName(host) : "";
    };
    const visible = this.rules
      .filter((rule) =>
        [rule.label, describeForward(rule), hostName(rule)].some((value) =>
          value.toLowerCase().includes(this.filterText),
        ),
      )
      .sort((a, b) => (a.label || describeForward(a)).localeCompare(b.label || describeForward(b)));
    if (visible.length === 0) {
      const message =
        this.rules.length === 0
          ? "No rules yet. Tunnel ports through your hosts, or start a SOCKS proxy."
          : "No matching rules.";
      this.list.replaceChildren(element("p", "host-list-empty", message));
      return;
    }
    this.list.replaceChildren(...visible.map((rule) => this.item(rule, hostName(rule))));
  }

  private isOn(rule: Forward): boolean {
    const state = this.statuses.get(rule.id)?.state;
    return state === "running" || state === "starting";
  }

  private async saved(rule: Forward) {
    const index = this.rules.findIndex((r) => r.id === rule.id);
    if (index >= 0) this.rules[index] = rule;
    else this.rules.push(rule);
    this.render();
    // A running rule restarts, so the change takes effect.
    if (this.isOn(rule)) {
      await api.stopForward(rule.id);
      void this.start(rule);
    }
  }

  private async start(rule: Forward) {
    const host = this.hosts().find((h) => h.id === rule.hostId);
    if (!host) {
      void showError("Host not found", "The host for this rule was deleted. Edit the rule to pick another one.");
      return;
    }
    // Failures show on the rule itself, through its status.
    await connectWithRecovery(host, (secret) => api.startForward(rule.id, secret), () => {});
  }

  private toggle(rule: Forward) {
    if (this.isOn(rule)) void api.stopForward(rule.id);
    else void this.start(rule);
  }

  private async remove(rule: Forward) {
    const confirmed = await confirmAction({
      title: "Delete rule?",
      message: `${rule.label || describeForward(rule)} will be removed${this.isOn(rule) ? " and stopped" : ""}.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!confirmed) return;
    try {
      await api.deleteForward(rule.id);
      this.rules = this.rules.filter((r) => r.id !== rule.id);
      this.render();
    } catch (error) {
      await showError("Couldn't delete rule", api.asCommandError(error).message);
    }
  }

  private item(rule: Forward, hostName: string): HTMLElement {
    const status = this.statuses.get(rule.id);
    const on = this.isOn(rule);
    const item = element("div", "host-item forward-item");
    item.tabIndex = 0;
    item.title = `${describeForward(rule)} through ${hostName || "a deleted host"}\nClick: ${on ? "stop" : "start"}`;

    const avatar = element("span", "host-avatar");
    const dot = element("span", "status-dot");
    dot.dataset.state =
      status?.state === "running"
        ? "connected"
        : status?.state === "starting"
          ? "connecting"
          : status?.state === "error"
            ? "closed"
            : "idle";
    avatar.append(icon("forward"), dot);

    const text = element("div", "host-text");
    const name = element("span", "host-name", rule.label || describeForward(rule));
    const open = status?.state === "running" ? status.connections : 0;
    if (open > 0) {
      const count = element("span", "connection-count", String(open));
      count.title = `${open} open ${open === 1 ? "connection" : "connections"}`;
      name.append(count);
    }
    const via = hostName ? `via ${hostName}` : "via a deleted host";
    const line = element("span", "host-address", rule.label ? `${describeForward(rule)} · ${via}` : via);
    if (status?.state === "error") {
      line.textContent = status.message ?? "Stopped";
      line.classList.add("error");
    }
    text.append(name, line);

    const button = (name: IconName, label: string, run: () => void) =>
      iconButton(name, label, (event) => {
        event.stopPropagation();
        run();
      }, "icon-button small");
    const actions = element("div", "host-actions");
    actions.append(
      button("pencil", "Edit", () => this.editor.open(rule)),
      button("trash", "Delete", () => void this.remove(rule)),
    );
    const power = button(on ? "stop" : "play", on ? "Stop" : "Start", () => this.toggle(rule));
    power.classList.add("forward-power");
    item.append(avatar, text, actions, power);
    item.addEventListener("click", () => this.toggle(rule));
    item.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && event.target === item) this.toggle(rule);
    });
    return item;
  }
}

class ForwardEditor {
  private readonly dialog = document.querySelector<HTMLDialogElement>("#forward-dialog")!;
  private readonly form = document.querySelector<HTMLFormElement>("#forward-form")!;
  private readonly error = document.querySelector<HTMLElement>("#forward-form-error")!;
  private editing?: Forward;

  constructor(
    private readonly hosts: () => readonly Host[],
    private readonly onSaved: (rule: Forward) => void,
  ) {
    this.field("kind").addEventListener("change", () => this.showKind());
    this.form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.save();
    });
    this.form.querySelector("[data-close]")!.addEventListener("click", () => this.dialog.close());
  }

  private field<T extends HTMLInputElement | HTMLSelectElement = HTMLInputElement>(name: string): T {
    return this.form.elements.namedItem(name) as T;
  }

  open(rule?: Forward) {
    this.editing = rule;
    this.form.reset();
    this.error.textContent = "";
    document.querySelector("#forward-dialog-title")!.textContent = rule
      ? "Edit port forwarding rule"
      : "New port forwarding rule";
    const hostSelect = this.field<HTMLSelectElement>("hostId");
    hostSelect.replaceChildren(
      ...[...this.hosts()]
        .sort((a, b) => displayName(a).localeCompare(displayName(b)))
        .map((host) => {
          const option = element("option", "", `${displayName(host)} (${host.username}@${host.hostname})`);
          option.value = host.id;
          return option;
        }),
    );
    if (rule && !this.hosts().some((h) => h.id === rule.hostId)) {
      const missing = element("option", "", "Deleted host: pick another");
      missing.value = "";
      hostSelect.prepend(missing);
    }
    if (rule) {
      this.field("label").value = rule.label;
      this.field<HTMLSelectElement>("kind").value = rule.kind;
      hostSelect.value = this.hosts().some((h) => h.id === rule.hostId) ? rule.hostId : "";
      this.field("bindAddress").value = rule.bindAddress;
      this.field("bindPort").value = String(rule.bindPort);
      this.field("destinationHost").value = rule.destinationHost ?? "";
      this.field("destinationPort").value = rule.destinationPort ? String(rule.destinationPort) : "";
    }
    this.showKind();
    this.dialog.showModal();
    this.field("label").focus();
  }

  private kind(): ForwardKind {
    return this.field<HTMLSelectElement>("kind").value as ForwardKind;
  }

  private showKind() {
    const kind = this.kind();
    this.form.querySelector<HTMLElement>("[data-kind-hint]")!.textContent = KIND_HINT[kind];
    this.form.querySelector<HTMLElement>("[data-destination]")!.hidden = kind === "dynamic";
    this.form.querySelector<HTMLElement>("[data-bind-label]")!.textContent =
      kind === "remote" ? "Listen on the host" : "Listen on this computer";
    this.field("destinationHost").required = kind !== "dynamic";
    this.field("destinationPort").required = kind !== "dynamic";
  }

  private async save() {
    const kind = this.kind();
    const value = (name: string) => this.field(name).value.trim();
    const draft: ForwardDraft = {
      id: this.editing?.id,
      label: value("label"),
      hostId: this.field<HTMLSelectElement>("hostId").value,
      kind,
      bindAddress: value("bindAddress"),
      bindPort: Number(value("bindPort")),
      ...(kind === "dynamic"
        ? {}
        : { destinationHost: value("destinationHost"), destinationPort: Number(value("destinationPort")) }),
    };
    if (!draft.hostId) {
      this.error.textContent = "Pick the host to tunnel through.";
      return;
    }
    try {
      this.onSaved(await api.saveForward(draft));
      this.dialog.close();
    } catch (error) {
      this.error.textContent = api.asCommandError(error).message;
    }
  }
}
