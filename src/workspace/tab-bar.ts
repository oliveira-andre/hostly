// The tab strip: drag tabs to reorder them, or below the strip onto a pane to
// split with it; the mouse wheel scrolls the strip sideways.
import { trackDrag } from "../drag";

const EDGE_SCROLL_PX = 32;

/** Where a tab dragged below the strip goes. */
export interface TabDropTarget {
  preview(tab: HTMLElement, x: number, y: number): void;
  drop(tab: HTMLElement, x: number, y: number): void;
  /** The tab came back to the strip, or the drag ended without a drop. */
  cancel(): void;
}

export class TabBar {
  constructor(
    private readonly strip: HTMLElement,
    private readonly onReorder: () => void,
    private readonly below: TabDropTarget,
  ) {
    strip.addEventListener(
      "wheel",
      (event) => {
        if (Math.abs(event.deltaY) > Math.abs(event.deltaX)) {
          strip.scrollLeft += event.deltaY;
          event.preventDefault();
        }
      },
      { passive: false },
    );
  }

  /** Adds `tab` at `index` (default: the end). It activates on click, so dragging it leaves the current tab on screen. */
  add(tab: HTMLElement, onActivate: () => void, index = this.strip.children.length) {
    this.strip.insertBefore(tab, this.strip.children[index] ?? null);
    tab.addEventListener("pointerdown", (down) => {
      if (down.button !== 0 || (down.target as Element).closest("button")) return;
      this.trackDrag(tab, down, onActivate);
    });
    tab.scrollIntoView({ inline: "nearest", block: "nearest" });
  }

  /** The strip position a tab dropped at `x` would take. */
  indexAt(x: number): number {
    const tabs = [...this.strip.children];
    const index = tabs.findIndex((tab) => {
      const rect = tab.getBoundingClientRect();
      return x < rect.left + rect.width / 2;
    });
    return index < 0 ? tabs.length : index;
  }

  private trackDrag(tab: HTMLElement, down: PointerEvent, onActivate: () => void) {
    const bar = this.strip.parentElement ?? this.strip;
    let isBelow = false;
    const reset = () => tab.classList.remove("dragging", "dragging-out");

    trackDrag(tab, down, {
      click: onActivate,
      start: () => tab.classList.add("dragging"),
      move: (event) => {
        isBelow = event.clientY > bar.getBoundingClientRect().bottom;
        tab.classList.toggle("dragging-out", isBelow);
        if (isBelow) this.below.preview(tab, event.clientX, event.clientY);
        else {
          this.below.cancel();
          this.reorder(tab, event.clientX);
        }
      },
      drop: (event) => {
        reset();
        if (isBelow) this.below.drop(tab, event.clientX, event.clientY);
        else {
          this.onReorder();
          onActivate();
        }
      },
      cancel: () => {
        reset();
        this.below.cancel();
        this.onReorder();
      },
    });
  }

  private reorder(tab: HTMLElement, x: number) {
    const bounds = this.strip.getBoundingClientRect();
    if (x < bounds.left + EDGE_SCROLL_PX) this.strip.scrollLeft -= 12;
    else if (x > bounds.right - EDGE_SCROLL_PX) this.strip.scrollLeft += 12;

    // Insert before the first tab whose midpoint is right of the pointer.
    const before = [...this.strip.children].find((other) => {
      if (other === tab) return false;
      const rect = other.getBoundingClientRect();
      return x < rect.left + rect.width / 2;
    });
    if (before) {
      if (tab.nextElementSibling !== before) this.strip.insertBefore(tab, before);
    } else if (this.strip.lastElementChild !== tab) {
      this.strip.append(tab);
    }
  }
}
