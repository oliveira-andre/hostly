// Inter for UI, JetBrains Mono for terminals and machine output (DESIGN.md §3), bundled locally.
import "@fontsource/inter/400.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/700.css";

export const MONO_FONT = '"JetBrains Mono", "Fira Code", ui-monospace, SFMono-Regular, Menlo, monospace';

/** xterm.js measures glyphs when a terminal opens, so terminals wait for the mono font. */
export const fontsReady: Promise<unknown> = Promise.all([
  document.fonts.load('400 13px "JetBrains Mono"'),
  document.fonts.load('700 13px "JetBrains Mono"'),
]).catch(() => undefined);
