import "./fonts";

import { AccountView } from "./account";
import { asCommandError, onForwardsChanged, onHostsChanged, onSnippetsChanged, syncNow } from "./api";
import { showError } from "./dialogs";
import { element } from "./format";
import { ForwardList } from "./forwards";
import { HostSidebar } from "./hosts";
import { icon } from "./icons";
import { showMenu } from "./menu";
import { currentRoute } from "./pages";
import { type ShortcutName, isMac, shortcuts } from "./platform";
import { SearchPanel } from "./search-panel";
import { chooseSerialPort } from "./serial";
import { SettingsPage } from "./settings";
import { Sidebar } from "./sidebar";
import { SnippetList } from "./snippets";
import { Workspace } from "./workspace/workspace";

const search = new SearchPanel();
const workspace = new Workspace({
  activePaneChanged: (pane) => search.target(pane),
  toggleSearch: () => search.toggle(),
});
const hosts = new HostSidebar(
  {
    connect: (host) => workspace.openTerminal(host),
    openBeside: (host, direction) => workspace.openBeside(host, direction),
    openSftp: (host) => workspace.openSftp(host),
    previewDrop: (host, x, y) => workspace.previewHostDrop(host, x, y),
    drop: (host, x, y) => workspace.dropHost(host, x, y),
    cancelDrop: () => workspace.endHostDrop(),
  },
  // Rules show their host's name.
  () => forwards.render(),
);
const snippets = new SnippetList(
  (script, run) => workspace.activeTerminal()?.typeSnippet(script, run) ?? false,
);
const forwards = new ForwardList(() => hosts.all);
new Sidebar([hosts, snippets, forwards]);
const loadOrReport = (what: string, load: () => Promise<void>) =>
  load().catch((error) => showError(`Couldn't load ${what}`, asCommandError(error).message));
void loadOrReport("hosts", () => hosts.load());
void loadOrReport("snippets", () => snippets.load());
void loadOrReport("port forwarding rules", () => forwards.load());

// Sync: lists reload when another device changed them, and coming back to the
// window checks for changes right away.
void onHostsChanged(() => void hosts.load());
void onSnippetsChanged(() => void snippets.load());
void onForwardsChanged(() => void forwards.load());
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
  if (onPage) search.close(false);
  account.render();
  settings.render();
}
window.addEventListener("hashchange", applyRoute);
Promise.all([account.refresh(), settings.refresh()])
  .catch(() => {})
  .finally(applyRoute);

const openSerial = async () => {
  const choice = await chooseSerialPort();
  if (choice) workspace.openSerial(choice.path, choice.baudRate);
};
const newTab = document.querySelector<HTMLButtonElement>("#new-tab")!;
newTab.append(icon("plus"));
newTab.addEventListener("click", () =>
  showMenu(newTab, [
    {
      label: "Local terminal",
      icon: "laptop",
      hint: shortcuts.newLocal.keys.join(" "),
      run: () => workspace.openLocal(),
    },
    { label: "Serial port…", icon: "cable", run: () => void openSerial() },
  ]),
);

const onSessionsPage = (run: () => void) => () => {
  if (currentRoute() === "sessions") run();
};
const actions: Record<ShortcutName, () => void> = {
  newLocal: onSessionsPage(() => workspace.openLocal()),
  splitRight: () => workspace.split("row"),
  splitDown: () => workspace.split("column"),
  nextTab: () => workspace.cycleTab(1),
  previousTab: () => workspace.cycleTab(-1),
  find: onSessionsPage(() => search.open()),
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
  shortcutRow("Run a snippet in the active terminal", ["click it in Snippets"]),
  shortcutRow("Split with any host", ["drag host onto pane"]),
  shortcutRow("Split with an open tab", ["drag tab onto pane"]),
  shortcutRow("Rearrange a split", ["drag pane title"]),
  shortcutRow("Reorder tabs", ["drag tab"]),
);
