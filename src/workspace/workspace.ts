// Tabs, each holding a split layout of panes.
import type { Host } from "../api";
import { DragGhost, trackDrag } from "../drag";
import { displayName, element } from "../format";
import { icon, iconButton } from "../icons";
import type { Pane, PaneEvents, PaneState } from "../panes/pane";
import { SftpPane } from "../panes/sftp";
import { TerminalPane } from "../panes/terminal";
import { shortcutHint } from "../platform";
import { Layout, type LayoutEvents, type SplitDirection, type SplitPosition } from "./layout";
import { TabBar } from "./tab-bar";

class Tab {
  readonly button = element("div", "tab");
  readonly body = element("div", "tab-body");
  readonly layout: Layout;
  activePane: Pane;
  private readonly label = element("span", "tab-label");
  private readonly kindIcon = element("span", "tab-icon");
  private readonly status = element("span", "status-dot");

  constructor(pane: Pane, onCloseTab: (tab: Tab) => void, layoutEvents: LayoutEvents) {
    this.activePane = pane;
    this.layout = new Layout(pane, this.body, layoutEvents);
    this.button.setAttribute("role", "tab");
    const close = iconButton("close", "Close tab", (event) => {
      event.stopPropagation();
      onCloseTab(this);
    }, "tab-close");
    this.button.append(this.status, this.kindIcon, this.label, close);
    // Middle-click closes, like a browser tab.
    this.button.addEventListener("auxclick", (event) => {
      if (event.button === 1) onCloseTab(this);
    });
    this.update();
  }

  get title(): string {
    return this.label.textContent ?? "";
  }

  /** Refreshes the label, icon and status dot from the tab's panes. */
  update() {
    const panes = this.layout.panes();
    const pane = this.activePane;
    // "storage · apps" for a split of two hosts; "+1" counts repeats of the same host.
    const names = [...new Set(panes.map((p) => displayName(p.host)))];
    const repeats = panes.length - names.length;
    this.label.textContent = `${names.join(" · ")}${repeats ? ` +${repeats}` : ""}`;
    this.button.title = [
      ...panes.map((p) => `${p.kind === "sftp" ? "SFTP · " : ""}${displayName(p.host)}`),
      "Drag onto a pane to split with it",
    ].join("\n");
    this.kindIcon.replaceChildren(icon(pane.kind === "sftp" ? "folderOpen" : "terminal", 14));
    this.status.dataset.state = summarize(panes.map((p) => p.state));
    this.body.classList.toggle("split", panes.length > 1);
  }
}

/** What is being dragged over the workspace. */
type DragSource = { kind: "host"; host: Host } | { kind: "tab"; tab: Tab } | { kind: "pane"; pane: Pane };

interface Beside {
  pane: Pane;
  direction: SplitDirection;
  position: SplitPosition;
}

interface Placement {
  /** Screen area highlighted while dragging. */
  area: DOMRect;
  label: string;
  /** Absent: a new tab. */
  beside?: Beside;
}

/** Any pane connecting → connecting; any connected → connected; otherwise closed. */
function summarize(states: PaneState[]): PaneState {
  if (states.includes("connecting")) return "connecting";
  return states.includes("connected") ? "connected" : "closed";
}

export class Workspace {
  private tabs: Tab[] = [];
  private active?: Tab;
  private readonly strip = document.querySelector<HTMLElement>("#tabs")!;
  private readonly bodies = document.querySelector<HTMLElement>("#tab-bodies")!;
  private readonly emptyState = document.querySelector<HTMLElement>("#empty-state")!;
  private readonly tabBar = new TabBar(this.strip, () => this.syncOrder(), {
    preview: (button, x, y) => this.withTab(button, (tab) => this.previewDrop({ kind: "tab", tab }, x, y)),
    drop: (button, x, y) => this.withTab(button, (tab) => this.drop({ kind: "tab", tab }, x, y)),
    cancel: () => this.endDrop(),
  });
  private readonly layoutEvents: LayoutEvents = {
    close: (pane) => this.closePane(pane),
    grab: (pane, handle, down) => this.grabPane(pane, handle, down),
  };
  private readonly dropIndicator = element("div", "drop-indicator");
  private ghost?: DragGhost;
  private readonly paneEvents: PaneEvents = {
    stateChanged: (pane) => this.tabOf(pane)?.update(),
    focused: (pane) => this.markActive(pane),
    closeRequested: (pane) => this.closePane(pane),
  };

  constructor() {
    this.dropIndicator.append(element("span", "drop-label"));
    this.dropIndicator.hidden = true;
    document.body.append(this.dropIndicator);
    const actions = document.querySelector<HTMLElement>("#tab-actions")!;
    actions.append(
      iconButton("splitRight", shortcutHint("splitRight"), () => this.split("row")),
      iconButton("splitDown", shortcutHint("splitDown"), () => this.split("column")),
      iconButton("folderOpen", "Open SFTP for this host", () => {
        if (this.active) this.openSftp(this.active.activePane.host);
      }),
    );
    this.updateActions();
  }

  openTerminal(host: Host) {
    this.openTab(new TerminalPane(host, this.paneEvents));
  }

  openSftp(host: Host) {
    this.openTab(new SftpPane(host, this.paneEvents));
  }

  /** Opens a new terminal to the active pane's host beside it. */
  split(direction: SplitDirection) {
    if (this.active) this.openBeside(this.active.activePane.host, direction);
  }

  /** Opens `host` beside the active pane, or in a new tab when nothing is open. */
  openBeside(host: Host, direction: SplitDirection) {
    if (this.active) this.splitWith(host, { pane: this.active.activePane, direction, position: "after" });
    else this.openTerminal(host);
  }

  /** Shows where a host dragged to (x, y) would open. */
  previewHostDrop(host: Host, x: number, y: number) {
    this.previewDrop({ kind: "host", host }, x, y);
  }

  /** Opens a dragged host where it was dropped: beside a pane, or in a new tab. */
  dropHost(host: Host, x: number, y: number) {
    this.drop({ kind: "host", host }, x, y);
  }

  endHostDrop() {
    this.endDrop();
  }

  private previewDrop(source: DragSource, x: number, y: number) {
    this.ghost ??= ghostFor(source);
    this.ghost.moveTo(x, y);
    const placement = this.placementAt(source, x, y);
    if (!placement) {
      this.dropIndicator.hidden = true;
      return;
    }
    const { left, top, width, height } = placement.area;
    Object.assign(this.dropIndicator.style, {
      left: `${left}px`,
      top: `${top}px`,
      width: `${width}px`,
      height: `${height}px`,
    });
    this.dropIndicator.firstElementChild!.textContent = placement.label;
    this.dropIndicator.hidden = false;
  }

  private endDrop() {
    this.ghost?.remove();
    this.ghost = undefined;
    this.dropIndicator.hidden = true;
  }

  private drop(source: DragSource, x: number, y: number) {
    const placement = this.placementAt(source, x, y);
    this.endDrop();
    if (!placement) return;
    const { beside } = placement;
    switch (source.kind) {
      case "host":
        if (beside) this.splitWith(source.host, beside);
        else this.openTerminal(source.host);
        break;
      case "tab":
        if (beside) this.mergeTab(source.tab, beside);
        break;
      case "pane":
        if (beside) this.movePane(source.pane, beside);
        else this.detachPane(source.pane, x);
        break;
    }
  }

  private splitWith(host: Host, { pane: target, direction, position }: Beside) {
    const tab = this.tabOf(target);
    if (!tab) return;
    const pane = new TerminalPane(host, this.paneEvents);
    tab.layout.split(target, pane, direction, position);
    this.markActive(pane);
    pane.start();
  }

  /** Moves the open sessions of `source` into the split beside `target`, without reconnecting. */
  private mergeTab(source: Tab, { pane: target, direction, position }: Beside) {
    const tab = this.tabOf(target);
    if (!tab || tab === source) return;
    const moved = source.activePane;
    tab.layout.adopt(target, source.layout, direction, position);
    this.removeTab(source);
    if (this.active !== tab) this.activate(tab);
    this.markActive(moved);
    moved.focus();
  }

  /** Moves `pane` to another place in its tab's split. */
  private movePane(pane: Pane, { pane: target, direction, position }: Beside) {
    const tab = this.tabOf(pane);
    if (!tab || tab !== this.tabOf(target)) return;
    tab.layout.move(pane, target, direction, position);
    this.markActive(pane);
    pane.focus();
  }

  /** Takes `pane` out of its split into a new tab at the strip position under `x`. */
  private detachPane(pane: Pane, x: number) {
    const from = this.tabOf(pane);
    if (!from) return;
    const neighbour = this.neighbourOf(from, pane);
    if (!neighbour || !from.layout.remove(pane)) return;
    this.markActive(neighbour);
    this.addTab(pane, this.tabBar.indexAt(x));
  }

  /** A press on a split pane's title bar: drag it within the split or onto the tab bar. */
  private grabPane(pane: Pane, handle: HTMLElement, down: PointerEvent) {
    const source: DragSource = { kind: "pane", pane };
    const leaf = handle.parentElement;
    trackDrag(handle, down, {
      click: () => pane.focus(),
      start: () => leaf?.classList.add("dragging"),
      move: (event) => this.previewDrop(source, event.clientX, event.clientY),
      drop: (event) => {
        leaf?.classList.remove("dragging");
        this.drop(source, event.clientX, event.clientY);
      },
      cancel: () => {
        leaf?.classList.remove("dragging");
        this.endDrop();
      },
    });
  }

  /**
   * Over a pane, the nearest edge decides the split: left/right place the
   * dragged item side by side, top/bottom stack it. Over the tab bar, a host
   * opens in a new tab and a pane moves to one; an open tab only splits with
   * a pane of another tab.
   */
  private placementAt(source: DragSource, x: number, y: number): Placement | undefined {
    const hit = document.elementFromPoint(x, y);
    if (!hit) return undefined;
    const newTab = (label: string) => ({ area: this.bodies.getBoundingClientRect(), label });
    if (hit.closest(".tab-bar")) {
      if (source.kind === "host") return newTab("Open in new tab");
      if (source.kind === "pane" && (this.tabOf(source.pane)?.layout.panes().length ?? 0) > 1) {
        return newTab("Move to new tab");
      }
      return undefined;
    }
    if (!hit.closest("#tab-bodies")) return undefined;
    const found = this.active?.layout.paneAt(hit);
    if (!found) return this.active || source.kind !== "host" ? undefined : newTab("Open in new tab");
    if (source.kind === "pane" && found.pane === source.pane) return undefined;
    if (source.kind === "tab" && this.tabOf(found.pane) === source.tab) return undefined;

    const rect = found.tile.getBoundingClientRect();
    const fx = (x - rect.left) / rect.width;
    const fy = (y - rect.top) / rect.height;
    const edges = [
      { distance: fx, direction: "row", position: "before", label: "Split left" },
      { distance: 1 - fx, direction: "row", position: "after", label: "Split right" },
      { distance: fy, direction: "column", position: "before", label: "Split up" },
      { distance: 1 - fy, direction: "column", position: "after", label: "Split down" },
    ] as const;
    const edge = edges.reduce((a, b) => (b.distance < a.distance ? b : a));
    const half = edge.direction === "row" ? { width: rect.width / 2 } : { height: rect.height / 2 };
    const area = new DOMRect(
      edge.direction === "row" && edge.position === "after" ? rect.left + rect.width / 2 : rect.left,
      edge.direction === "column" && edge.position === "after" ? rect.top + rect.height / 2 : rect.top,
      half.width ?? rect.width,
      half.height ?? rect.height,
    );
    return {
      area,
      label: edge.label,
      beside: { pane: found.pane, direction: edge.direction, position: edge.position },
    };
  }

  /** Moves to the next (1) or previous (-1) tab. */
  cycleTab(offset: number) {
    if (!this.active || this.tabs.length < 2) return;
    const index = this.tabs.indexOf(this.active);
    this.activate(this.tabs[(index + offset + this.tabs.length) % this.tabs.length]);
  }

  private openTab(pane: Pane) {
    this.addTab(pane);
    pane.start();
  }

  /** Shows `pane` in a new tab at strip position `index` (default: the end). */
  private addTab(pane: Pane, index = this.tabs.length) {
    const tab = new Tab(pane, (t) => this.closeTab(t), this.layoutEvents);
    this.tabs.splice(index, 0, tab);
    this.bodies.append(tab.body);
    this.tabBar.add(tab.button, () => this.activate(tab), index);
    this.activate(tab);
  }

  private activate(tab: Tab) {
    this.active = tab;
    for (const t of this.tabs) {
      const isActive = t === tab;
      t.button.classList.toggle("active", isActive);
      t.button.setAttribute("aria-selected", String(isActive));
      t.body.hidden = !isActive;
    }
    this.emptyState.hidden = true;
    this.markActive(tab.activePane);
    requestAnimationFrame(() => tab.activePane.focus());
    this.updateActions();
  }

  private markActive(pane: Pane) {
    const tab = this.tabOf(pane);
    if (!tab) return;
    tab.activePane = pane;
    tab.layout.setActive(pane);
    tab.update();
  }

  private closePane(pane: Pane) {
    const tab = this.tabOf(pane);
    if (!tab) return;
    const neighbour = this.neighbourOf(tab, pane);
    if (!neighbour || !tab.layout.remove(pane)) {
      this.closeTab(tab);
      return;
    }
    pane.dispose();
    this.markActive(neighbour);
    neighbour.focus();
  }

  /** The pane that takes over when `pane` leaves `tab`, if any. */
  private neighbourOf(tab: Tab, pane: Pane): Pane | undefined {
    const panes = tab.layout.panes();
    const index = panes.indexOf(pane);
    return panes[index - 1] ?? panes[index + 1];
  }

  private closeTab(tab: Tab) {
    if (!this.tabs.includes(tab)) return;
    for (const pane of tab.layout.panes()) pane.dispose();
    this.removeTab(tab);
  }

  /** Takes `tab` off the strip, leaving its panes alone. */
  private removeTab(tab: Tab) {
    const index = this.tabs.indexOf(tab);
    if (index < 0) return;
    this.tabs.splice(index, 1);
    tab.button.remove();
    tab.body.remove();
    if (this.active === tab) {
      const next = this.tabs[Math.min(index, this.tabs.length - 1)];
      if (next) this.activate(next);
      else {
        this.active = undefined;
        this.emptyState.hidden = false;
        this.updateActions();
      }
    }
  }

  private tabOf(pane: Pane): Tab | undefined {
    return this.tabs.find((tab) => tab.layout.panes().includes(pane));
  }

  private withTab(button: HTMLElement, run: (tab: Tab) => void) {
    const tab = this.tabs.find((t) => t.button === button);
    if (tab) run(tab);
  }

  /** Mirrors the tab strip's order after a drag. */
  private syncOrder() {
    const order = [...this.strip.children];
    this.tabs.sort((a, b) => order.indexOf(a.button) - order.indexOf(b.button));
  }

  private updateActions() {
    for (const button of document.querySelectorAll<HTMLButtonElement>("#tab-actions button")) {
      button.disabled = !this.active;
    }
  }
}

function ghostFor(source: DragSource): DragGhost {
  switch (source.kind) {
    case "host":
      return new DragGhost("server", displayName(source.host));
    case "tab":
      return new DragGhost(source.tab.activePane.kind === "sftp" ? "folderOpen" : "terminal", source.tab.title);
    case "pane":
      return new DragGhost(source.pane.kind === "sftp" ? "folderOpen" : "terminal", displayName(source.pane.host));
  }
}
