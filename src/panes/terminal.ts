// A terminal pane: one xterm.js instance bound to one session: an SSH (or
// Mosh) connection to a saved host, a shell on this computer, or a serial port.
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import * as api from "../api";
import type { Host, SessionCallbacks, SessionStatus } from "../api";
import { connectWithRecovery } from "../connect";
import { MONO_FONT, fontsReady } from "../fonts";
import { displayName, plain } from "../format";
import type { IconName } from "../icons";
import { isMac } from "../platform";
import type { Pane, PaneEvents, PaneState } from "./pane";
import { TerminalSearch } from "./terminal-search";

const ENTER = 13;
const dim = (text: string) => `\x1b[2m${text}\x1b[0m`;
const red = (text: string) => `\x1b[31m${text}\x1b[0m`;

// DESIGN.md palette; bright variants are lighter steps of the same hues.
const theme = {
  background: "#22212C",
  foreground: "#F8F8F2",
  cursor: "#9580FF",
  cursorAccent: "#22212C",
  selectionBackground: "#454158",
  scrollbarSliderBackground: "rgba(255, 128, 191, 0.3)",
  scrollbarSliderHoverBackground: "rgba(255, 128, 191, 0.6)",
  scrollbarSliderActiveBackground: "rgba(255, 128, 191, 0.75)",
  black: "#1B1A23",
  red: "#FF9580",
  green: "#8AFF80",
  yellow: "#FFFF80",
  blue: "#9580FF",
  magenta: "#FF80BF",
  cyan: "#80FFEA",
  white: "#F8F8F2",
  brightBlack: "#7970A9",
  brightRed: "#FFAA99",
  brightGreen: "#A2FF99",
  brightYellow: "#FFFF99",
  brightBlue: "#AA99FF",
  brightMagenta: "#FF99CC",
  brightCyan: "#99FFEE",
  brightWhite: "#F8F8F2",
};

/** What a terminal connects to. */
export type TerminalTarget =
  | { type: "ssh"; host: Host }
  | { type: "local" }
  | { type: "serial"; path: string; baudRate: number };

/** Resolves to `promise`'s value, or reports its failure and resolves to null. */
async function attempt<T>(promise: Promise<T>, report: (message: string) => void): Promise<T | null> {
  try {
    return await promise;
  } catch (error) {
    report(api.asCommandError(error).message);
    return null;
  }
}

export class TerminalPane implements Pane {
  readonly kind = "terminal";
  readonly element = document.createElement("div");
  state: PaneState = "connecting";
  private readonly terminal = new Terminal({
    fontFamily: MONO_FONT,
    fontSize: 13,
    cursorBlink: true,
    scrollback: 10_000,
    theme,
    // Search highlights are decorations, which xterm still marks as proposed API.
    allowProposedApi: true,
  });
  readonly search = new TerminalSearch(this.terminal);
  private readonly fitAddon = new FitAddon();
  private readonly resizeObserver = new ResizeObserver(() => this.fit());
  private readonly encoder = new TextEncoder();
  private sessionId?: string;
  private opened = false;
  private disposed = false;

  constructor(
    readonly target: TerminalTarget,
    private readonly events: PaneEvents,
  ) {
    this.element.className = "terminal-pane";
    this.element.addEventListener("focusin", () => events.focused(this));
    this.terminal.loadAddon(this.fitAddon);
    this.terminal.attachCustomKeyEventHandler((event) => this.clipboardKey(event));
    this.terminal.onData((data) => this.input(this.encoder.encode(data)));
    this.terminal.onBinary((data) => this.input(Uint8Array.from(data, (c) => c.charCodeAt(0))));
    this.terminal.onResize(({ cols, rows }) => {
      if (this.state === "connected" && this.sessionId) {
        api.resize(this.sessionId, cols, rows).catch(() => {});
      }
    });
  }

  async start() {
    await fontsReady;
    if (this.disposed) return;
    this.terminal.open(this.element);
    this.opened = true;
    this.resizeObserver.observe(this.element);
    this.fit();
    this.focus();
    await this.connect();
  }

  get host(): Host | undefined {
    return this.target.type === "ssh" ? this.target.host : undefined;
  }

  get title(): string {
    switch (this.target.type) {
      case "ssh":
        return displayName(this.target.host);
      case "local":
        return "Local terminal";
      case "serial":
        return this.target.path.split(/[\\/]/).pop() || this.target.path;
    }
  }

  get icon(): IconName {
    switch (this.target.type) {
      case "ssh":
        return "terminal";
      case "local":
        return "laptop";
      case "serial":
        return "cable";
    }
  }

  focus() {
    if (this.opened) this.terminal.focus();
  }

  /**
   * Types a snippet into the session, as a paste so a multi-line script isn't
   * cut short by shells with bracketed paste. `run` presses Enter after it.
   * Returns false when there is no live session to type into.
   */
  typeSnippet(script: string, run: boolean): boolean {
    if (this.state !== "connected" || !this.opened) return false;
    this.terminal.paste(script.replace(/\s+$/, ""));
    if (run) this.input(this.encoder.encode("\r"));
    this.focus();
    return true;
  }

  /**
   * Off macOS, Ctrl+C and Ctrl+V belong to the shell, so copy and paste are
   * Ctrl+Shift+C / Ctrl+Shift+V (macOS uses ⌘C / ⌘V through the Edit menu).
   * Returns false for keys the terminal must not handle.
   */
  private clipboardKey(event: KeyboardEvent): boolean {
    if (isMac || !event.ctrlKey || !event.shiftKey || event.altKey) return true;
    if (event.code !== "KeyC" && event.code !== "KeyV") return true;
    // Also keeps the webview from handling them (Ctrl+Shift+C opens the inspector).
    event.preventDefault();
    if (event.type !== "keydown") return false;
    if (event.code === "KeyC") {
      const selection = this.terminal.getSelection();
      if (selection) void writeText(selection);
    } else {
      void readText()
        .then((text) => {
          if (text && !this.disposed) this.terminal.paste(text);
        })
        .catch(() => {});
    }
    return false;
  }

  /** Hidden panes have no size, so they are fitted when they become visible. */
  private fit() {
    if (this.opened && !this.disposed && this.element.offsetParent !== null) this.fitAddon.fit();
  }

  private async connect() {
    this.setState("connecting");
    this.terminal.writeln(dim(this.connectingText()));
    const id = await this.open({
      onData: (bytes) => {
        if (!this.disposed) this.terminal.write(bytes);
      },
      onStatus: (status) => this.closed(status),
    });
    if (this.disposed) {
      if (id) void api.disconnect(id);
      return;
    }
    if (id === null) {
      this.endWithReconnectHint();
      return;
    }
    this.sessionId = id;
    // The session can end before connect resolves (e.g. a forced command).
    if (this.state === "connecting") {
      this.setState("connected");
      // The pane may have been resized while connecting.
      api.resize(id, this.terminal.cols, this.terminal.rows).catch(() => {});
    }
  }

  private connectingText(): string {
    switch (this.target.type) {
      case "ssh": {
        const { username, hostname, port, mosh } = this.target.host;
        return `Connecting to ${plain(`${username}@${hostname}:${port}`)}${mosh ? " with Mosh" : ""}…`;
      }
      case "local":
        return "Starting your shell…";
      case "serial":
        return `Opening ${plain(this.target.path)} at ${this.target.baudRate} baud…`;
    }
  }

  /** Starts the session and resolves to its id, or to null after reporting why it failed. */
  private open(callbacks: SessionCallbacks): Promise<string | null> {
    const size = { cols: this.terminal.cols, rows: this.terminal.rows };
    const report = (message: string) => {
      if (!this.disposed) this.terminal.writeln(red(message));
    };
    const target = this.target;
    switch (target.type) {
      case "ssh":
        return connectWithRecovery(
          target.host,
          (secret) => api.connect(target.host.id, size, secret, callbacks),
          report,
        );
      case "local":
        return attempt(api.openLocal(size, callbacks), report);
      case "serial":
        return attempt(api.openSerial(target.path, target.baudRate, callbacks), report);
    }
  }

  private input(bytes: Uint8Array) {
    if (this.state === "connected" && this.sessionId) {
      api.write(this.sessionId, bytes).catch(() => {});
    } else if (this.state === "closed" && bytes.length === 1 && bytes[0] === ENTER) {
      this.terminal.writeln("");
      void this.connect();
    }
  }

  private closed(status: SessionStatus) {
    if (this.disposed) return;
    this.sessionId = undefined;
    // Logging out (exit, Ctrl+D) closes the pane; a dropped connection stays
    // open to show why, with the option to reconnect.
    if (!status.error) {
      this.state = "closed";
      this.events.closeRequested(this);
      return;
    }
    this.terminal.writeln("");
    this.terminal.writeln(red(`Connection lost: ${status.error}`));
    this.endWithReconnectHint();
  }

  private endWithReconnectHint() {
    this.setState("closed");
    this.terminal.writeln(dim("Press Enter to reconnect."));
  }

  private setState(state: PaneState) {
    this.state = state;
    this.events.stateChanged(this);
  }

  dispose() {
    this.disposed = true;
    this.search.dispose();
    if (this.sessionId) void api.disconnect(this.sessionId);
    this.resizeObserver.disconnect();
    this.terminal.dispose();
    this.element.remove();
  }
}
