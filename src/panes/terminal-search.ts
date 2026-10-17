// Search over one terminal's whole buffer, scrollback included. The search
// addon highlights the matches and steps through them; `lines` lists them.
import { type ISearchOptions, SearchAddon } from "@xterm/addon-search";
import type { IDisposable, Terminal } from "@xterm/xterm";

/** Matches past this many aren't highlighted or counted (the addon's default). */
export const HIGHLIGHT_LIMIT = 1000;

export interface SearchQuery {
  term: string;
  caseSensitive: boolean;
  regex: boolean;
}

/** A line of output (wrapped rows joined) holding at least one match. */
export interface SearchLine {
  /** Buffer rows the line spans: [start, end). */
  start: number;
  end: number;
  text: string;
  /** [from, to) offsets of each match in `text`. */
  matches: [number, number][];
}

export interface SearchResults {
  /** The active match; -1 when there is none or it's past HIGHLIGHT_LIMIT. */
  index: number;
  /** Matches found, up to HIGHLIGHT_LIMIT. */
  count: number;
}

// Yellow is DESIGN.md's attention highlight. Backgrounds must be #RRGGBB, so
// these are #FFFF80 blended 20% and 40% over the terminal background.
const decorations = {
  matchBackground: "#4E4D3D",
  matchOverviewRuler: "#FFFF80",
  activeMatchBackground: "#7A7A4E",
  activeMatchBorder: "#FFFF80",
  activeMatchColorOverviewRuler: "#FFFF80",
};

/** False for a regex that doesn't compile. */
export function isValidQuery(query: SearchQuery): boolean {
  try {
    matcher(query);
    return true;
  } catch {
    return false;
  }
}

function matcher({ term, caseSensitive, regex }: SearchQuery): RegExp {
  const source = regex ? term : term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(source, caseSensitive ? "g" : "gi");
}

function searchOptions({ caseSensitive, regex }: SearchQuery): ISearchOptions {
  return { caseSensitive, regex, decorations };
}

export class TerminalSearch {
  private readonly addon = new SearchAddon({ highlightLimit: HIGHLIGHT_LIMIT });
  private lastQuery?: SearchQuery;
  private disposed = false;

  constructor(private readonly terminal: Terminal) {
    terminal.loadAddon(this.addon);
  }

  /** Fires after every step, and when new output moves the matches. */
  onResults(listener: (results: SearchResults) => void): IDisposable {
    return this.addon.onDidChangeResults(({ resultIndex, resultCount }) =>
      listener({ index: resultIndex, count: resultCount }),
    );
  }

  /** Fires once output or a resize has settled, as lines may have changed. */
  onChange(listener: () => void): IDisposable {
    let timer: number | undefined;
    const later = () => {
      clearTimeout(timer);
      timer = window.setTimeout(listener, 200);
    };
    const subscriptions = [this.terminal.onWriteParsed(later), this.terminal.onResize(later)];
    return {
      dispose: () => {
        clearTimeout(timer);
        for (const subscription of subscriptions) subscription.dispose();
      },
    };
  }

  /** Starts over from the newest output: the bottom-most match becomes active. */
  restart(query: SearchQuery) {
    if (this.disposed) return;
    this.terminal.clearSelection();
    this.step(query, "previous");
  }

  /**
   * Activates the previous (older) or next match, wrapping around. After the
   * query changes, the active match stays put if it still matches.
   */
  step(query: SearchQuery, direction: "previous" | "next") {
    if (this.disposed) return;
    // The addon stores new options before comparing them with the old, so it
    // only re-highlights when the term changes. Clearing forces it to.
    const last = this.lastQuery;
    if (last && (last.caseSensitive !== query.caseSensitive || last.regex !== query.regex)) {
      this.addon.clearDecorations();
    }
    this.lastQuery = query;
    const options = searchOptions(query);
    if (direction === "previous") this.addon.findPrevious(query.term, options);
    else this.addon.findNext(query.term, options);
  }

  /** Activates the first match on `line`, scrolling it into view. */
  reveal(line: SearchLine, query: SearchQuery) {
    if (this.disposed) return;
    // findNext continues after the selection, so select the cell just before the line.
    if (line.start > 0) this.terminal.select(this.terminal.cols - 1, line.start - 1, 1);
    else this.terminal.clearSelection();
    this.step(query, "next");
  }

  /** The buffer row of the active match, which the addon keeps selected. */
  activeRow(): number | undefined {
    return this.disposed ? undefined : this.terminal.getSelectionPosition()?.start.y;
  }

  /** Removes the highlights. The active match stays selected, ready to copy. */
  clear() {
    if (!this.disposed) this.addon.clearDecorations();
  }

  /** Every line with a match, oldest first. The query must be valid. */
  lines(query: SearchQuery): SearchLine[] {
    if (this.disposed) return [];
    const pattern = matcher(query);
    const buffer = this.terminal.buffer.active;
    const lines: SearchLine[] = [];
    let start = 0;
    while (start < buffer.length) {
      // A line too long for the terminal continues on rows marked isWrapped.
      let end = start + 1;
      while (buffer.getLine(end)?.isWrapped) end++;
      let text = "";
      for (let row = start; row < end; row++) {
        text += buffer.getLine(row)?.translateToString(row === end - 1) ?? "";
      }
      const matches: [number, number][] = [];
      for (const match of text.matchAll(pattern)) {
        if (match[0].length > 0) matches.push([match.index, match.index + match[0].length]);
      }
      if (matches.length > 0) lines.push({ start, end, text, matches });
      start = end;
    }
    return lines;
  }

  dispose() {
    this.disposed = true;
  }
}
