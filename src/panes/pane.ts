import type { Host } from "../api";

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
  readonly host: Host;
  readonly element: HTMLElement;
  readonly state: PaneState;
  /** Called once the pane is in the document and visible. */
  start(): void;
  focus(): void;
  dispose(): void;
}
