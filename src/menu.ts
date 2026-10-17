// A small popover menu under a button. Closes on a choice, Escape, or a click elsewhere.
import { element } from "./format";
import { type IconName, icon } from "./icons";

export interface MenuItem {
  label: string;
  icon: IconName;
  /** Keys shown on the right, e.g. "⌘T". */
  hint?: string;
  run(): void;
}

let close: (() => void) | undefined;

export function showMenu(anchor: HTMLElement, items: MenuItem[]) {
  close?.();
  const menu = element("div", "menu");
  menu.setAttribute("role", "menu");
  const buttons = items.map((item) => {
    const button = element("button", "menu-item");
    button.type = "button";
    button.setAttribute("role", "menuitem");
    button.append(icon(item.icon), element("span", "menu-label", item.label));
    if (item.hint) button.append(element("span", "menu-hint", item.hint));
    button.addEventListener("click", () => {
      dismiss();
      item.run();
    });
    return button;
  });
  menu.append(...buttons);
  document.body.append(menu);

  const rect = anchor.getBoundingClientRect();
  const width = menu.offsetWidth;
  menu.style.top = `${rect.bottom + 6}px`;
  menu.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - width - 8))}px`;
  anchor.setAttribute("aria-expanded", "true");

  const onPointer = (event: PointerEvent) => {
    if (!menu.contains(event.target as Node) && !anchor.contains(event.target as Node)) dismiss();
  };
  const onKey = (event: KeyboardEvent) => {
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      dismiss();
      anchor.focus();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      buttons[(index + step + buttons.length) % buttons.length].focus();
    }
  };
  function dismiss() {
    menu.remove();
    anchor.setAttribute("aria-expanded", "false");
    window.removeEventListener("pointerdown", onPointer, true);
    window.removeEventListener("keydown", onKey, true);
    close = undefined;
  }
  window.addEventListener("pointerdown", onPointer, true);
  window.addEventListener("keydown", onKey, true);
  close = dismiss;
  buttons[0]?.focus();
}
