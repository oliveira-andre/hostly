export const isMac = navigator.userAgent.includes("Mac");

export type ShortcutName = "splitRight" | "splitDown" | "nextTab" | "previousTab";

interface Shortcut {
  label: string;
  /** Matches the combo built from a keydown in main.ts, e.g. "Meta+Shift+KeyG". */
  combo: string;
  /** Keys as shown to the user, one chip each. */
  keys: string[];
}

// Not ⌘D: it sits next to Ctrl+D (end of input), which ends the shell. ⌘V is Paste.
// Off macOS, plain Ctrl+letter belongs to the shell, so these also need Shift.
export const shortcuts: Record<ShortcutName, Shortcut> = isMac
  ? {
      splitRight: { label: "Split right", combo: "Meta+KeyG", keys: ["⌘", "G"] },
      splitDown: { label: "Split down", combo: "Meta+Shift+KeyG", keys: ["⇧", "⌘", "G"] },
      nextTab: { label: "Next tab", combo: "Meta+Shift+BracketRight", keys: ["⇧", "⌘", "]"] },
      previousTab: { label: "Previous tab", combo: "Meta+Shift+BracketLeft", keys: ["⇧", "⌘", "["] },
    }
  : {
      splitRight: { label: "Split right", combo: "Ctrl+Shift+KeyG", keys: ["Ctrl", "Shift", "G"] },
      splitDown: { label: "Split down", combo: "Ctrl+Shift+KeyH", keys: ["Ctrl", "Shift", "H"] },
      nextTab: { label: "Next tab", combo: "Ctrl+PageDown", keys: ["Ctrl", "PgDn"] },
      previousTab: { label: "Previous tab", combo: "Ctrl+PageUp", keys: ["Ctrl", "PgUp"] },
    };

/** Tooltip text, e.g. "Split down (⇧ ⌘ G)". */
export function shortcutHint(name: ShortcutName): string {
  const { label, keys } = shortcuts[name];
  return `${label} (${keys.join(" ")})`;
}
