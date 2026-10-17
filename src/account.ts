// The #/account page and the profile button that opens it.
import * as api from "./api";
import type { AccountStatus, SyncStatus } from "./api";
import { confirmAction } from "./dialogs";
import { element } from "./format";
import { icon } from "./icons";
import { backButton, currentRoute, navigate, toggle } from "./pages";
import { secretField } from "./secret-input";

/** Plain http is only safe on this machine; elsewhere passwords would cross the network unencrypted. */
function insecure(server: string): boolean {
  return /^http:\/\//i.test(server) && !/^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:|\/|$)/i.test(server);
}

function statusText(sync: SyncStatus): string {
  switch (sync.state) {
    case "syncing":
      return "Syncing…";
    case "synced":
      return sync.lastSyncedAt
        ? `Synced at ${new Date(sync.lastSyncedAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`
        : "Synced";
    case "error":
      return `Sync failed: ${sync.message ?? "unknown error"}`;
    case "loggedOut":
      return sync.message ?? "Not logged in";
  }
}

function statusDot(sync: SyncStatus): HTMLElement {
  const dot = element("span", "status-dot");
  dot.dataset.state = sync.state === "synced" ? "connected" : sync.state === "syncing" ? "connecting" : "closed";
  return dot;
}

/** Account state shared by the page and the profile button. */
export class AccountView {
  private status: AccountStatus = {
    server: null,
    account: null,
    sync: { state: "loggedOut", lastSyncedAt: null, message: null },
    passwordSync: false,
  };
  /** The server logging in would use (from Settings). */
  private server: string | null = null;
  private mode: "logIn" | "signUp" = "logIn";
  private readonly page = document.querySelector<HTMLElement>("#page")!;
  private readonly profile = document.querySelector<HTMLButtonElement>("#profile-button")!;

  constructor() {
    this.profile.addEventListener("click", () => toggle("account"));
    void api.onSyncStatus((sync) => {
      this.status = { ...this.status, sync };
      if (sync.state === "loggedOut") {
        this.status = { ...this.status, server: null, account: null, passwordSync: false };
      }
      this.render();
    });
  }

  get loggedIn(): boolean {
    return this.status.account !== null;
  }

  /** The server this device is logged in to. */
  get loggedInServer(): string | null {
    return this.status.server;
  }

  async refresh() {
    const [status, settings] = await Promise.all([api.accountStatus(), api.getSettings()]);
    this.status = status;
    this.server = settings.effectiveSyncServer;
    this.render();
  }

  render() {
    this.renderProfile();
    if (currentRoute() === "account") this.renderPage();
  }

  private renderProfile() {
    const { account, sync } = this.status;
    this.profile.classList.toggle("active", currentRoute() === "account");
    this.profile.title = account ? `${account.email} · ${statusText(sync)}` : "Account: log in to sync hosts";
    if (!account) {
      this.profile.replaceChildren(icon("account", 20));
      return;
    }
    const avatar = element("span", "avatar", account.email.charAt(0).toUpperCase());
    this.profile.replaceChildren(avatar, statusDot(sync));
  }

  private renderPage() {
    const card = element("div", "account-card");
    if (this.status.account) this.renderAccount(card);
    else if (this.server) this.renderLogin(card, this.server);
    else this.renderNoServer(card);
    this.page.replaceChildren(backButton(), card);
  }

  private renderAccount(card: HTMLElement) {
    const { account, server, sync } = this.status;
    const header = element("div", "account-header");
    const who = element("div", "account-who");
    who.append(element("span", "account-email", account!.email), element("span", "account-server", server ?? ""));
    header.append(element("span", "avatar large", account!.email.charAt(0).toUpperCase()), who);

    const status = element("div", "sync-status");
    status.append(statusDot(sync), element("span", "", statusText(sync)));

    const note = element(
      "p",
      "hint",
      "Hosts, snippets and port forwarding rules sync with this account across your devices.",
    );

    const actions = element("div", "account-actions");
    const syncButton = element("button", "button-secondary", "Sync now");
    syncButton.type = "button";
    syncButton.disabled = sync.state === "syncing";
    syncButton.addEventListener("click", () => void api.syncNow());
    const logOut = element("button", "button-primary danger");
    logOut.type = "button";
    logOut.append(icon("logOut"), element("span", "", "Log out"));
    logOut.addEventListener("click", () => void this.logOut());
    actions.append(syncButton, logOut);

    card.append(header, status, note, this.passwordSyncSection(), this.changePasswordSection(), actions);
  }

  /** Saved passwords sync once this device holds the vault key. */
  private passwordSyncSection(): HTMLElement {
    const section = element("div", "password-sync");
    if (this.status.passwordSync) {
      const row = element("div", "password-sync-on");
      row.append(
        icon("shield"),
        element(
          "span",
          "",
          "Saved passwords and passphrases sync end-to-end encrypted with your account password. The server can't read them.",
        ),
      );
      section.append(row);
      return section;
    }
    const form = element("form", "account-form");
    const password = passwordInput("password", "");
    const error = element("p", "form-error");
    error.setAttribute("role", "alert");
    const submit = element("button", "button-primary", "Turn on password sync");
    submit.type = "submit";
    form.append(
      element("h2", "", "Sync saved passwords"),
      element(
        "p",
        "hint",
        "This device logged in before passwords could sync. Enter your account password once to sync them end-to-end encrypted.",
      ),
      labelled("Account password", password),
      error,
      submit,
    );
    secretField(password);
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      error.textContent = "";
      submit.disabled = true;
      try {
        await api.unlockPasswordSync(password.value);
        await this.refresh();
      } catch (failure) {
        error.textContent = api.asCommandError(failure).message;
      } finally {
        submit.disabled = false;
      }
    });
    section.append(form);
    return section;
  }

  private changePasswordSection(): HTMLElement {
    const details = element("details", "change-password");
    const summary = element("summary", "", "Change password");
    details.append(summary);
    if (!this.status.passwordSync) {
      details.append(element("p", "hint", "Turn on password sync on this device first."));
      return details;
    }
    const form = element("form", "account-form");
    const current = passwordInput("current", "");
    const next = passwordInput("new", "At least 8 characters");
    next.minLength = 8;
    const confirm = passwordInput("confirm", "");
    const message = element("p", "form-error");
    message.setAttribute("role", "alert");
    const submit = element("button", "button-primary", "Change password");
    submit.type = "submit";
    form.append(
      element(
        "p",
        "hint",
        "Saved passwords stay readable on all your devices, and they stay logged in. There is no reset: if you forget the new password, synced passwords can't be recovered from the server.",
      ),
      labelled("Current password", current),
      labelled("New password", next),
      labelled("Confirm new password", confirm),
      message,
      submit,
    );
    for (const input of [current, next, confirm]) secretField(input);
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      message.textContent = "";
      message.classList.remove("ok");
      if (next.value !== confirm.value) {
        message.textContent = "The new passwords don't match.";
        return;
      }
      submit.disabled = true;
      try {
        await api.changePassword(current.value, next.value);
        form.reset();
        message.textContent = "Password changed.";
        message.classList.add("ok");
      } catch (failure) {
        const error = api.asCommandError(failure);
        message.textContent =
          error.kind === "invalidCredentials" ? "The current password is wrong." : error.message;
      } finally {
        submit.disabled = false;
      }
    });
    details.append(form);
    return details;
  }

  private renderNoServer(card: HTMLElement) {
    const open = element("button", "button-primary", "Open settings");
    open.type = "button";
    open.addEventListener("click", () => navigate("settings"));
    card.append(
      element("h1", "", "Log in to sync"),
      element(
        "p",
        "hint",
        "Hostly works without an account. To sync your hosts, first set the address of your Hostly sync server in Settings.",
      ),
      open,
    );
  }

  private renderLogin(card: HTMLElement, server: string) {
    const signingUp = this.mode === "signUp";
    const title = element("h1", "", signingUp ? "Create your account" : "Log in to sync");
    const intro = element(
      "p",
      "hint",
      "Hostly works without an account. Log in to sync hosts, snippets, port forwarding rules and saved passwords through your Hostly server. Passwords are end-to-end encrypted with your account password, so the server can't read them. Anything already on this device is kept.",
    );

    const serverRow = element("div", "server-row");
    const change = element("a", "", "Change");
    change.href = "#/settings";
    serverRow.append(element("span", "server-label", "Server"), element("span", "server-value", server), change);
    const warning = element(
      "p",
      "form-warning",
      insecure(server) ? "This server isn't encrypted (http://). Use https:// for servers outside this computer." : "",
    );

    const tabs = element("div", "segmented");
    for (const [mode, label] of [["logIn", "Log in"], ["signUp", "Create account"]] as const) {
      const tab = element("button", mode === this.mode ? "active" : "", label);
      tab.type = "button";
      tab.addEventListener("click", () => {
        this.mode = mode;
        this.renderPage();
      });
      tabs.append(tab);
    }

    const form = element("form", "account-form");
    const field = (label: string, input: HTMLInputElement) => {
      const wrapper = element("label", "");
      wrapper.append(label, input);
      return wrapper;
    };
    const input = (name: string, type: string, placeholder = "") => {
      const node = element("input", "");
      Object.assign(node, { name, type, placeholder, required: true, spellcheck: false });
      node.autocomplete = "off";
      return node;
    };
    const email = input("email", "email", "you@example.com");
    const password = input("password", "password");
    password.minLength = 8;
    const confirm = input("confirm", "password");
    const error = element("p", "form-error");
    error.setAttribute("role", "alert");
    const submit = element("button", "button-primary", signingUp ? "Create account" : "Log in");
    submit.type = "submit";

    form.append(field("Email", email), field("Password", password));
    secretField(password);
    if (signingUp) {
      form.append(
        field("Confirm password", confirm),
        element(
          "p",
          "hint",
          "This password also encrypts your saved passwords. The server can't reset it: if you forget it, synced passwords can't be recovered, though each device keeps its own copies.",
        ),
      );
      secretField(confirm);
    }
    form.append(error, submit);

    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      error.textContent = "";
      if (signingUp && password.value !== confirm.value) {
        error.textContent = "The passwords don't match.";
        return;
      }
      submit.disabled = true;
      try {
        await (signingUp ? api.signUp : api.logIn)(email.value.trim(), password.value);
        await this.refresh();
      } catch (failure) {
        error.textContent = api.asCommandError(failure).message;
      } finally {
        submit.disabled = false;
      }
    });

    card.append(title, intro, serverRow, warning, tabs, form);
    email.focus();
  }

  private async logOut() {
    const confirmed = await confirmAction({
      title: "Log out?",
      message: "Syncing stops. Your hosts, snippets, rules and saved passwords stay on this device.",
      confirmLabel: "Log out",
      danger: true,
    });
    if (!confirmed) return;
    await api.logOut();
    await this.refresh();
  }
}

function passwordInput(name: string, placeholder: string): HTMLInputElement {
  const input = element("input", "");
  Object.assign(input, { name, type: "password", placeholder, required: true, spellcheck: false });
  input.autocomplete = "off";
  return input;
}

function labelled(label: string, input: HTMLInputElement): HTMLElement {
  const wrapper = element("label", "");
  wrapper.append(label, input);
  return wrapper;
}
