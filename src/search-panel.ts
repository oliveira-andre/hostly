// The right-hand panel that searches the active terminal's output, scrollback included.
import { element } from "./format";
import { type IconName, iconButton } from "./icons";
import type { Pane } from "./panes/pane";
import {
  HIGHLIGHT_LIMIT,
  type SearchLine,
  type SearchQuery,
  type SearchResults,
  isValidQuery,
} from "./panes/terminal-search";

/** Lines listed at most; the newest are kept, as they sit nearest the prompt. */
const LIST_LIMIT = 500;
/** Characters shown before a line's first match when the line is cut. */
const CONTEXT = 24;
/** Characters of a line rendered at all; the rest is cut off anyway. */
const SNIPPET = 300;

export class SearchPanel {
  private readonly panel = document.querySelector<HTMLElement>("#search-panel")!;
  private readonly hostLabel = element("span", "search-panel-host");
  private readonly input = element("input", "search");
  private readonly caseToggle = toggleButton("caseSensitive", "Match case", () => this.optionsChanged());
  private readonly regexToggle = toggleButton("regex", "Use regular expression", () => this.optionsChanged());
  private readonly count = element("span", "search-count");
  private readonly previousButton = iconButton("chevronUp", "Previous match (Enter)", () => this.step("previous"), "icon-button small");
  private readonly nextButton = iconButton("chevronDown", "Next match (Shift+Enter)", () => this.step("next"), "icon-button small");
  private readonly list = element("div", "search-results");
  private readonly note = element("p", "search-note");
  private pane?: Pane;
  private lines: SearchLine[] = [];
  private rows: HTMLElement[] = [];
  private subscriptions: { dispose(): void }[] = [];

  constructor() {
    const title = element("div", "search-panel-title");
    title.append(element("h2", "", "Find"), this.hostLabel);
    const header = element("header", "search-panel-header");
    header.append(title, iconButton("close", "Close (Esc)", () => this.close(), "icon-button small"));

    this.input.type = "search";
    this.input.placeholder = "Search output";
    this.input.autocomplete = "off";
    this.input.spellcheck = false;
    this.input.setAttribute("aria-label", "Search terminal output");
    this.input.addEventListener("input", () => this.run(false));
    this.input.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" || event.isComposing) return;
      event.preventDefault();
      this.step(event.shiftKey ? "next" : "previous");
    });

    const tools = element("div", "search-tools");
    tools.append(this.caseToggle, this.regexToggle, this.count, this.previousButton, this.nextButton);
    this.note.hidden = true;
    this.panel.append(header, this.input, tools, this.list, this.note);
    this.panel.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      this.close();
    });
  }

  /** Opens the panel, or selects the query if it's already open. */
  open() {
    if (this.panel.hidden) {
      this.panel.hidden = false;
      this.attach();
      this.run(true);
    }
    this.input.focus();
    this.input.select();
  }

  /** Closes the panel, handing focus back to the terminal unless `refocus` is false. */
  close(refocus = true) {
    if (this.panel.hidden) return;
    this.panel.hidden = true;
    this.detach();
    if (refocus) this.pane?.focus();
  }

  toggle() {
    if (this.panel.hidden) this.open();
    else this.close();
  }

  /** Follows the active pane; an open panel searches the new one from its newest output. */
  target(pane: Pane | undefined) {
    if (pane === this.pane) return;
    const open = !this.panel.hidden;
    if (open) this.detach();
    this.pane = pane;
    this.hostLabel.textContent = pane?.search ? pane.title : "";
    if (open) {
      this.attach();
      this.run(true);
    }
  }

  private get search() {
    return this.pane?.search;
  }

  private query(): SearchQuery {
    return { term: this.input.value, caseSensitive: pressed(this.caseToggle), regex: pressed(this.regexToggle) };
  }

  private attach() {
    const search = this.search;
    if (!search) return;
    this.subscriptions = [
      search.onResults((results) => this.showResults(results)),
      search.onChange(() => this.refresh()),
    ];
  }

  private detach() {
    this.search?.clear();
    for (const subscription of this.subscriptions) subscription.dispose();
    this.subscriptions = [];
  }

  private optionsChanged() {
    this.input.focus();
    this.run(false);
  }

  /** Searches again: from the newest output, or keeping the active match where it still matches. */
  private run(fromBottom: boolean) {
    const search = this.search;
    const query = this.query();
    const message = !this.pane
      ? "Open a terminal to search its output."
      : !search
        ? "Find works in terminal panes. Click one to search it."
        : !query.term
          ? "Searches everything in this session, scrollback included."
          : !isValidQuery(query)
            ? "That isn't a valid regular expression."
            : undefined;
    if (message || !search) {
      search?.clear();
      this.showMessage(message ?? "", !!query.term && !!search);
      this.count.textContent = "";
      this.setNavigable(false);
      return;
    }
    this.render(search.lines(query));
    if (fromBottom) search.restart(query);
    else search.step(query, "previous");
    this.markActive(true);
  }

  /** New output arrived: relist the lines without moving the list. */
  private refresh() {
    const search = this.search;
    const query = this.query();
    if (this.panel.hidden || !search || !query.term || !isValidQuery(query)) return;
    const scrollTop = this.list.scrollTop;
    this.render(search.lines(query));
    this.list.scrollTop = scrollTop;
    this.markActive(false);
  }

  private step(direction: "previous" | "next") {
    const query = this.query();
    if (!this.search || !query.term || !isValidQuery(query)) return;
    this.search.step(query, direction);
    this.markActive(true);
  }

  private showResults({ index, count }: SearchResults) {
    const total = count >= HIGHLIGHT_LIMIT ? `${HIGHLIGHT_LIMIT}+` : String(count);
    this.count.textContent =
      count === 0 ? "No results" : index >= 0 ? `${index + 1} of ${total}` : `${total} ${count === 1 ? "result" : "results"}`;
    this.setNavigable(count > 0);
    this.markActive(false);
  }

  private setNavigable(navigable: boolean) {
    this.previousButton.disabled = !navigable;
    this.nextButton.disabled = !navigable;
  }

  private showMessage(text: string, error: boolean) {
    this.lines = [];
    this.rows = [];
    this.note.hidden = true;
    this.list.replaceChildren(element("p", error ? "search-message error" : "search-message", text));
  }

  private render(lines: SearchLine[]) {
    if (lines.length === 0) {
      this.showMessage("No matches.", false);
      return;
    }
    this.lines = lines.slice(-LIST_LIMIT);
    this.rows = this.lines.map((line) => this.row(line));
    this.list.replaceChildren(...this.rows);
    this.note.textContent = `Showing the newest ${LIST_LIMIT} of ${lines.length} matching lines.`;
    this.note.hidden = lines.length <= LIST_LIMIT;
  }

  private row(line: SearchLine): HTMLElement {
    const row = element("button", "search-result");
    row.type = "button";
    row.title = `Line ${line.start + 1}`;
    row.append(element("span", "search-result-line", String(line.start + 1)), snippet(line));
    row.addEventListener("click", () => {
      this.search?.reveal(line, this.query());
      this.markActive(true);
      this.input.focus();
    });
    return row;
  }

  /** Highlights the listed line holding the active match, scrolling to it if `scroll`. */
  private markActive(scroll: boolean) {
    const active = this.search?.activeRow();
    this.lines.forEach((line, index) => {
      const isActive = active !== undefined && line.start <= active && active < line.end;
      const row = this.rows[index];
      row.classList.toggle("active", isActive);
      if (isActive) {
        row.setAttribute("aria-current", "true");
        if (scroll) row.scrollIntoView({ block: "nearest" });
      } else row.removeAttribute("aria-current");
    });
  }
}

function toggleButton(name: IconName, label: string, onToggle: () => void): HTMLButtonElement {
  const button = iconButton(
    name,
    label,
    () => {
      button.setAttribute("aria-pressed", String(!pressed(button)));
      onToggle();
    },
    "icon-button small search-toggle",
  );
  button.setAttribute("aria-pressed", "false");
  return button;
}

function pressed(toggle: HTMLElement): boolean {
  return toggle.getAttribute("aria-pressed") === "true";
}

/** The line's text with its matches marked, starting near the first match so it stays visible. */
function snippet({ text, matches }: SearchLine): HTMLElement {
  const first = matches[0][0];
  let from = Math.min(text.length - text.trimStart().length, first);
  const cut = first - from > CONTEXT;
  if (cut) from = first - CONTEXT;
  const to = Math.min(text.length, Math.max(from + SNIPPET, matches[0][1]));
  const out = element("span", "search-result-text", cut ? "…" : "");
  let at = from;
  for (const [start, end] of matches) {
    if (start >= to) break;
    if (end <= at) continue;
    out.append(text.slice(at, Math.max(at, start)), element("mark", "", text.slice(Math.max(at, start), Math.min(end, to))));
    at = Math.min(end, to);
  }
  out.append(text.slice(at, to));
  return out;
}
