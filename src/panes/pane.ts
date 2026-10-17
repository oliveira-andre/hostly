import type { Host } from "../api";
import type { IconName } from "../icons";
import type { TerminalSearch } from "./terminal-search";

export type PaneState = "connecting" | "connected" | "closed";

/** How a pane reports back to the workspace that hosts it. */
export interface PaneEvents {
  stateChanged(pane: Pane): void;
  focused(pane: Pane): void;
  closeRequested(pane: Pane): void;
}

/** A tile in a tab's split layout: a terminal or an SFTP browser. */
export interface Pane {
  readonly kind: "terminal" | "sftp";
  /** Shown on its tab and title bar. */
  readonly title: string;
  readonly icon: IconName;
  /** The saved host it connects to; local and serial terminals have none. */
  readonly host?: Host;
  readonly element: HTMLElement;
  readonly state: PaneState;
  /** Searches the pane's output; only terminals have one. */
  readonly search?: TerminalSearch;
  /** Called once the pane is in the document and visible. */
  start(): void;
  focus(): void;
  dispose(): void;
}
