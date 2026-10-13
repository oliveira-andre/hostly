// A tab's split layout: a tree of panes divided into rows and columns with draggable dividers.
import { displayName, element } from "../format";
import { iconButton } from "../icons";
import type { Pane } from "../panes/pane";

/** "row" places panes side by side (split right); "column" stacks them (split down). */
export type SplitDirection = "row" | "column";
export type SplitPosition = "before" | "after";

type Node =
  | { kind: "leaf"; pane: Pane }
  | { kind: "split"; direction: SplitDirection; children: Node[]; sizes: number[] };

type Split = Extract<Node, { kind: "split" }>;

const MIN_PANE_PX = 120;

/** How a layout reports back to the tab that holds it. */
export interface LayoutEvents {
  close(pane: Pane): void;
  /** A press on a pane's title bar, which may become a drag of the pane. */
  grab(pane: Pane, handle: HTMLElement, down: PointerEvent): void;
}

export class Layout {
  private root: Node;
  private readonly leaves = new Map<Pane, HTMLElement>();

  constructor(
    pane: Pane,
    private readonly container: HTMLElement,
    private readonly events: LayoutEvents,
  ) {
    this.root = { kind: "leaf", pane };
    this.render();
  }

  panes(): Pane[] {
    const panes: Pane[] = [];
    const walk = (node: Node) =>
      node.kind === "leaf" ? panes.push(node.pane) : node.children.forEach(walk);
    walk(this.root);
    return panes;
  }

  /** Places `pane` before (left/above) or after (right/below) `target`, taking half of its space. */
  split(target: Pane, pane: Pane, direction: SplitDirection, position: SplitPosition = "after") {
    this.insert(target, { kind: "leaf", pane }, direction, position);
  }

  /** Moves `pane`, already in this layout, beside `target`. */
  move(pane: Pane, target: Pane, direction: SplitDirection, position: SplitPosition) {
    if (pane === target || !this.detach(pane)) return;
    this.insert(target, { kind: "leaf", pane }, direction, position);
  }

  /** Moves every pane of `source` beside `target`, keeping their arrangement. `source` is left unusable. */
  adopt(target: Pane, source: Layout, direction: SplitDirection, position: SplitPosition) {
    this.insert(target, source.root, direction, position);
  }

  private insert(target: Pane, added: Node, direction: SplitDirection, position: SplitPosition) {
    const leaf = this.leafOf(target);
    const parent = this.parentOf(leaf);
    if (parent?.direction === direction) {
      const index = parent.children.indexOf(leaf);
      const half = parent.sizes[index] / 2;
      parent.sizes.splice(index, 1, half, half);
      parent.children.splice(position === "after" ? index + 1 : index, 0, added);
    } else {
      const children = position === "after" ? [leaf, added] : [added, leaf];
      this.replace(leaf, { kind: "split", direction, children, sizes: [0.5, 0.5] });
    }
    this.root = flatten(this.root);
    this.render();
  }

  /** The pane whose tile contains `target`, if any. */
  paneAt(target: Element): { pane: Pane; tile: HTMLElement } | undefined {
    const tile = target.closest<HTMLElement>(".layout-leaf");
    for (const [pane, leaf] of this.leaves) if (leaf === tile) return { pane, tile };
    return undefined;
  }

  /** Removes `pane` and gives its space to a neighbour. Returns false if it was the last pane. */
  remove(pane: Pane): boolean {
    if (!this.detach(pane)) return false;
    this.render();
    return true;
  }

  private detach(pane: Pane): boolean {
    const leaf = this.leafOf(pane);
    const parent = this.parentOf(leaf);
    if (!parent) return false;
    const index = parent.children.indexOf(leaf);
    const [freed] = parent.sizes.splice(index, 1);
    parent.children.splice(index, 1);
    parent.sizes[Math.max(0, index - 1)] += freed;
    if (parent.children.length === 1) this.replace(parent, parent.children[0]);
    return true;
  }

  setActive(pane: Pane) {
    for (const [p, leaf] of this.leaves) leaf.classList.toggle("active", p === pane);
  }

  private leafOf(pane: Pane): Node {
    const find = (node: Node): Node | undefined =>
      node.kind === "leaf"
        ? node.pane === pane
          ? node
          : undefined
        : node.children.map(find).find(Boolean);
    const leaf = find(this.root);
    if (!leaf) throw new Error("pane is not in this layout");
    return leaf;
  }

  private parentOf(target: Node, node: Node = this.root): Split | undefined {
    if (node.kind === "leaf") return undefined;
    if (node.children.includes(target)) return node;
    return node.children.map((child) => this.parentOf(target, child)).find(Boolean);
  }

  private replace(old: Node, replacement: Node) {
    const parent = this.parentOf(old);
    if (parent) parent.children[parent.children.indexOf(old)] = replacement;
    else this.root = replacement;
  }

  private render() {
    this.leaves.clear();
    this.container.replaceChildren(this.renderNode(this.root));
  }

  private renderNode(node: Node): HTMLElement {
    if (node.kind === "leaf") {
      const leaf = element("div", "layout-leaf");
      const header = element("div", "pane-header");
      const title = element("span", "pane-title", displayName(node.pane.host));
      const close = iconButton("close", "Close pane", () => this.events.close(node.pane), "icon-button small");
      header.append(title, close);
      header.title = "Drag to move this pane, or onto the tab bar to give it its own tab";
      header.addEventListener("pointerdown", (down) => {
        if (down.button === 0 && !(down.target as Element).closest("button")) {
          this.events.grab(node.pane, header, down);
        }
      });
      leaf.append(header, node.pane.element);
      this.leaves.set(node.pane, leaf);
      return leaf;
    }
    const split = element("div", "layout-split");
    split.dataset.direction = node.direction;
    node.children.forEach((child, index) => {
      if (index > 0) split.append(this.divider(node, index - 1, split));
      const rendered = this.renderNode(child);
      rendered.style.flex = `${node.sizes[index] * 100} 1 0`;
      split.append(rendered);
    });
    return split;
  }

  /** A divider between children `index` and `index + 1` that resizes them by dragging. */
  private divider(node: Split, index: number, container: HTMLElement): HTMLElement {
    const divider = element("div", "layout-divider");
    divider.addEventListener("pointerdown", (down) => {
      down.preventDefault();
      divider.setPointerCapture(down.pointerId);
      divider.classList.add("dragging");
      const horizontal = node.direction === "row";
      const total = horizontal ? container.clientWidth : container.clientHeight;
      const start = horizontal ? down.clientX : down.clientY;
      const [before, after] = [node.sizes[index], node.sizes[index + 1]];
      const min = Math.min(MIN_PANE_PX / total, (before + after) / 2);
      const children = [...container.children].filter(
        (child) => !child.classList.contains("layout-divider"),
      ) as HTMLElement[];

      const move = (event: PointerEvent) => {
        const delta = ((horizontal ? event.clientX : event.clientY) - start) / total;
        const size = Math.min(Math.max(before + delta, min), before + after - min);
        node.sizes[index] = size;
        node.sizes[index + 1] = before + after - size;
        children[index].style.flex = `${node.sizes[index] * 100} 1 0`;
        children[index + 1].style.flex = `${node.sizes[index + 1] * 100} 1 0`;
      };
      const end = () => {
        divider.classList.remove("dragging");
        divider.removeEventListener("pointermove", move);
        divider.removeEventListener("pointerup", end);
        divider.removeEventListener("pointercancel", end);
      };
      divider.addEventListener("pointermove", move);
      divider.addEventListener("pointerup", end);
      divider.addEventListener("pointercancel", end);
    });
    return divider;
  }
}

/** Merges splits nested in a split of the same direction, so each divider stays independent. */
function flatten(node: Node): Node {
  if (node.kind === "leaf") return node;
  const children: Node[] = [];
  const sizes: number[] = [];
  node.children.forEach((child, index) => {
    const flat = flatten(child);
    if (flat.kind === "split" && flat.direction === node.direction) {
      children.push(...flat.children);
      sizes.push(...flat.sizes.map((size) => size * node.sizes[index]));
    } else {
      children.push(flat);
      sizes.push(node.sizes[index]);
    }
  });
  return { ...node, children, sizes };
}
