import "./fonts";

import { AccountView } from "./account";
import { asCommandError, onHostsChanged, syncNow } from "./api";
import { showError } from "./dialogs";
import { element } from "./format";
import { HostSidebar } from "./hosts";
import { currentRoute } from "./pages";
import { type ShortcutName, isMac, shortcuts } from "./platform";
import { SettingsPage } from "./settings";
import { Workspace } from "./workspace/workspace";

const workspace = new Workspace();
const sidebar = new HostSidebar({
  connect: (host) => workspace.openTerminal(host),
  openBeside: (host, direction) => workspace.openBeside(host, direction),
  openSftp: (host) => workspace.openSftp(host),
  previewDrop: (host, x, y) => workspace.previewHostDrop(host, x, y),
  drop: (host, x, y) => workspace.dropHost(host, x, y),
  cancelDrop: () => workspace.endHostDrop(),
});
sidebar.load().catch((error) => showError("Couldn't load hosts", asCommandError(error).message));

// Sync: the host list reloads when another device changed it, and coming back
// to the window checks for changes right away.
void onHostsChanged(() => void sidebar.load());
const account = new AccountView();
window.addEventListener("focus", () => {
  if (account.loggedIn) void syncNow();
});

const settings = new SettingsPage(
  () => account.loggedInServer,
  () => void account.refresh(),
);

// Routes: #/ shows the sessions; #/account and #/settings show a full page instead.
const page = document.querySelector<HTMLElement>("#page")!;
function applyRoute() {
  const onPage = currentRoute() !== "sessions";
  document.querySelector(".workspace")!.classList.toggle("page-open", onPage);
  page.hidden = !onPage;
  account.render();
  settings.render();
}
window.addEventListener("hashchange", applyRoute);
Promise.all([account.refresh(), settings.refresh()])
  .catch(() => {})
  .finally(applyRoute);

const actions: Record<ShortcutName, () => void> = {
  splitRight: () => workspace.split("row"),
  splitDown: () => workspace.split("column"),
  nextTab: () => workspace.cycleTab(1),
  previousTab: () => workspace.cycleTab(-1),
};
const actionsByCombo = new Map(
  (Object.keys(shortcuts) as ShortcutName[]).map((name) => [shortcuts[name].combo, actions[name]]),
);

// Capture phase, so the focused terminal never sees these keys.
window.addEventListener(
  "keydown",
  (event) => {
    if (document.querySelector("dialog[open]")) return;
    const combo = [
      event.ctrlKey && "Ctrl",
      event.altKey && "Alt",
      event.metaKey && "Meta",
      event.shiftKey && "Shift",
      event.code,
    ]
      .filter(Boolean)
      .join("+");
    const action = actionsByCombo.get(combo);
    if (!action) return;
    event.preventDefault();
    event.stopPropagation();
    action();
  },
  { capture: true },
);

function shortcutRow(label: string, keys: string[]): HTMLElement {
  const row = element("li", "");
  const chips = element("span", "keys");
  chips.append(...keys.map((key) => element("kbd", "", key)));
  row.append(element("span", "shortcut-label", label), chips);
  return row;
}

document.querySelector("#shortcut-list")!.replaceChildren(
  ...Object.values(shortcuts).map(({ label, keys }) => shortcutRow(label, keys)),
  shortcutRow("Open a host beside the current pane", [isMac ? "⌥" : "Alt", "click"]),
  shortcutRow("Split with any host", ["drag host onto pane"]),
  shortcutRow("Split with an open tab", ["drag tab onto pane"]),
  shortcutRow("Rearrange a split", ["drag pane title"]),
  shortcutRow("Reorder tabs", ["drag tab"]),
);
