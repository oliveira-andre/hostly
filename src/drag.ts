// Pointer-event drags of hosts, tabs and panes, and the chip that follows the pointer.
import { element } from "./format";
import { type IconName, icon } from "./icons";

/** How far the pointer moves before a press becomes a drag. */
const DRAG_THRESHOLD_PX = 5;

export interface DragHandlers {
  /** Released before moving far enough to drag. */
  click?(event: PointerEvent): void;
  /** The press became a drag. */
  start?(): void;
  move(event: PointerEvent): void;
  drop(event: PointerEvent): void;
  /** The drag ended without a drop (Escape, or the system took the pointer). */
  cancel(): void;
}

/** Follows the press `down` on `handle` until release. */
export function trackDrag(handle: HTMLElement, down: PointerEvent, handlers: DragHandlers) {
  handle.setPointerCapture(down.pointerId);
  let dragging = false;

  const move = (event: PointerEvent) => {
    if (!dragging) {
      if (Math.hypot(event.clientX - down.clientX, event.clientY - down.clientY) < DRAG_THRESHOLD_PX) return;
      dragging = true;
      document.body.classList.add("dragging");
      handlers.start?.();
    }
    handlers.move(event);
  };
  const up = (event: PointerEvent) => {
    finish();
    if (dragging) handlers.drop(event);
    else handlers.click?.(event);
  };
  const cancel = () => {
    finish();
    if (dragging) handlers.cancel();
  };
  const escape = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    cancel();
  };
  const finish = () => {
    handle.removeEventListener("pointermove", move);
    handle.removeEventListener("pointerup", up);
    handle.removeEventListener("pointercancel", cancel);
    window.removeEventListener("keydown", escape, { capture: true });
    if (handle.hasPointerCapture(down.pointerId)) handle.releasePointerCapture(down.pointerId);
    document.body.classList.remove("dragging");
  };
  handle.addEventListener("pointermove", move);
  handle.addEventListener("pointerup", up);
  handle.addEventListener("pointercancel", cancel);
  window.addEventListener("keydown", escape, { capture: true });
}

/** The chip under the pointer while dragging. */
export class DragGhost {
  private readonly element = element("div", "drag-ghost");

  constructor(iconName: IconName, label: string) {
    this.element.append(icon(iconName, 14), element("span", "", label));
    document.body.append(this.element);
  }

  moveTo(x: number, y: number) {
    this.element.style.transform = `translate(${x + 12}px, ${y + 8}px)`;
  }

  remove() {
    this.element.remove();
  }
}
