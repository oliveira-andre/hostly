import type { Host } from "./api";

export function displayName(host: Host): string {
  return host.label.trim() || host.hostname;
}

export function address(host: Host): string {
  const port = host.port === 22 ? "" : `:${host.port}`;
  return `${host.username}@${host.hostname}${port}`;
}

/** Strips control characters so user-entered text can't inject terminal escape sequences. */
export function plain(text: string): string {
  return text.replace(/[\x00-\x1f\x7f]/g, "");
}

const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatSize(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  return unit === 0 ? `${value} B` : `${value.toFixed(value < 10 ? 1 : 0)} ${UNITS[unit]}`;
}

export function formatDate(unixSeconds: number | null): string {
  if (unixSeconds === null) return "";
  const date = new Date(unixSeconds * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
