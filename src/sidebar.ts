// The left sidebar: hosts, snippets and port forwarding, one section at a
// time. The section picks what "+" creates and what the search box filters.
import { element } from "./format";

export interface SidebarSection {
  /** The section's tab label. */
  readonly name: string;
  readonly list: HTMLElement;
  /** The "+" button's label, e.g. "New host". */
  readonly newLabel: string;
  readonly placeholder: string;
  create(): void;
  filter(text: string): void;
}

export class Sidebar {
  private readonly tabs = document.querySelector<HTMLElement>("#sidebar-tabs")!;
  private readonly add = document.querySelector<HTMLButtonElement>("#new-item")!;
  private readonly search = document.querySelector<HTMLInputElement>("#sidebar-filter")!;
  private readonly buttons: HTMLButtonElement[];
  /** Each section keeps its own search text. */
  private readonly filters: string[];
  private current = 0;

  constructor(private readonly sections: SidebarSection[]) {
    this.filters = sections.map(() => "");
    this.tabs.style.gridTemplateColumns = `repeat(${sections.length}, 1fr)`;
    this.buttons = sections.map((section, index) => {
      const button = element("button", "", section.name);
      button.type = "button";
      button.setAttribute("role", "tab");
      button.addEventListener("click", () => this.show(index));
      return button;
    });
    this.tabs.replaceChildren(...this.buttons);
    this.add.addEventListener("click", () => this.sections[this.current].create());
    this.search.addEventListener("input", () => {
      this.filters[this.current] = this.search.value;
      this.sections[this.current].filter(this.search.value);
    });
    this.show(0);
  }

  show(index: number) {
    this.current = index;
    this.sections.forEach((section, i) => {
      const active = i === index;
      section.list.hidden = !active;
      this.buttons[i].classList.toggle("active", active);
      this.buttons[i].setAttribute("aria-selected", String(active));
    });
    const section = this.sections[index];
    this.add.title = section.newLabel;
    this.add.setAttribute("aria-label", section.newLabel);
    this.search.placeholder = section.placeholder;
    this.search.value = this.filters[index];
  }
}
