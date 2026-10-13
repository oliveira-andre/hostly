// The #/settings page: where Hostly syncs to. Set once; logging in uses it.
import * as api from "./api";
import type { SettingsView } from "./api";
import { element } from "./format";
import { icon } from "./icons";
import { currentRoute, backButton, navigate, toggle } from "./pages";

export class SettingsPage {
  private settings?: SettingsView;
  private readonly page = document.querySelector<HTMLElement>("#page")!;
  private readonly button = document.querySelector<HTMLButtonElement>("#settings-button")!;

  constructor(
    /** The account this device is logged in to, if any: switching servers needs a log-out. */
    private readonly loggedInServer: () => string | null,
    /** Called after the sync server changes. */
    private readonly onChange: () => void,
  ) {
    this.button.append(icon("settings", 20));
    this.button.addEventListener("click", () => toggle("settings"));
  }

  async refresh() {
    this.settings = await api.getSettings();
    this.render();
  }

  render() {
    this.button.classList.toggle("active", currentRoute() === "settings");
    if (currentRoute() === "settings" && this.settings) this.renderPage(this.settings);
  }

  private renderPage(settings: SettingsView) {
    const card = element("div", "account-card");
    const title = element("h1", "", "Settings");
    const heading = element("h2", "", "Sync server");
    const intro = element(
      "p",
      "hint",
      "The Hostly server your hosts sync through. You set it once; logging in and creating accounts use it.",
    );

    const form = element("form", "account-form");
    const label = element("label", "");
    const input = element("input", "mono");
    Object.assign(input, {
      type: "url",
      name: "server",
      spellcheck: false,
      value: settings.syncServer ?? "",
      placeholder: settings.defaultSyncServer ?? "https://sync.example.com",
    });
    input.autocomplete = "off";
    label.append("Server address", input);

    const origin = element(
      "p",
      "hint",
      settings.defaultSyncServer
        ? `Leave empty to use the default from .env: ${settings.defaultSyncServer}`
        : "No default was built in. Set HOSTLY_SERVER_URL in .env before building to add one.",
    );
    const result = element("p", "check-result");
    result.setAttribute("role", "status");

    const loggedIn = this.loggedInServer();
    const actions = element("div", "account-actions");
    const test = element("button", "button-secondary", "Test connection");
    test.type = "button";
    const save = element("button", "button-primary", "Save");
    save.type = "submit";
    actions.append(test, save);
    form.append(label, origin, result, actions);

    if (settings.syncServer && !loggedIn) {
      const reset = element("button", "button-ghost small", "Use the default");
      reset.type = "button";
      reset.addEventListener("click", () => void this.save(null, result));
      form.append(reset);
    }

    if (loggedIn) {
      input.disabled = save.disabled = true;
      const note = element("p", "hint");
      note.append(`You're logged in to ${loggedIn}. `);
      const link = element("a", "", "Log out on the account page");
      link.href = "#/account";
      note.append(link, " to switch servers.");
      form.append(note);
    }

    const report = (message: string, ok: boolean) => {
      result.textContent = message;
      result.classList.toggle("ok", ok);
      result.classList.toggle("error", !ok);
    };
    test.addEventListener("click", async () => {
      const server = input.value.trim() || settings.effectiveSyncServer;
      if (!server) {
        report("Enter a server address first.", false);
        return;
      }
      test.disabled = true;
      report("Checking…", true);
      try {
        await api.checkSyncServer(server);
        report(`${server} is a reachable Hostly sync server.`, true);
      } catch (error) {
        report(api.asCommandError(error).message, false);
      } finally {
        test.disabled = false;
      }
    });
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.save(input.value.trim() || null, result);
    });

    card.append(title, heading, intro, form);
    this.page.replaceChildren(backButton(), card);
  }

  private async save(server: string | null, result: HTMLElement) {
    try {
      this.settings = await api.setSyncServer(server);
      this.onChange();
      navigate("account");
    } catch (error) {
      result.textContent = api.asCommandError(error).message;
      result.classList.add("error");
      result.classList.remove("ok");
    }
  }
}
