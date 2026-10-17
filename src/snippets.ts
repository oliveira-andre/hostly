// Sidebar section: saved snippets, run in the active terminal with a click.
import * as api from "./api";
import type { Snippet, SnippetDraft } from "./api";
import { confirmAction, showError } from "./dialogs";
import { element } from "./format";
import { type IconName, icon, iconButton } from "./icons";
import type { SidebarSection } from "./sidebar";

/** Types a script into the active terminal; false when there is none. */
export type RunSnippet = (script: string, run: boolean) => boolean;

export class SnippetList implements SidebarSection {
  readonly name = "Snippets";
  readonly newLabel = "New snippet";
  readonly placeholder = "Search snippets";
  readonly list = document.querySelector<HTMLElement>("#snippet-list")!;
  private snippets: Snippet[] = [];
  private filterText = "";
  private readonly editor = new SnippetEditor((snippet) => this.replace(snippet));

  constructor(private readonly type: RunSnippet) {}

  create() {
    this.editor.open();
  }

  filter(text: string) {
    this.filterText = text.trim().toLowerCase();
    this.render();
  }

  async load() {
    this.snippets = await api.listSnippets();
    this.render();
  }

  private replace(snippet: Snippet) {
    const index = this.snippets.findIndex((s) => s.id === snippet.id);
    if (index >= 0) this.snippets[index] = snippet;
    else this.snippets.push(snippet);
    this.render();
  }

  private async remove(snippet: Snippet) {
    const confirmed = await confirmAction({
      title: "Delete snippet?",
      message: `${snippet.name} will be removed.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!confirmed) return;
    try {
      await api.deleteSnippet(snippet.id);
      this.snippets = this.snippets.filter((s) => s.id !== snippet.id);
      this.render();
    } catch (error) {
      await showError("Couldn't delete snippet", api.asCommandError(error).message);
    }
  }

  private send(snippet: Snippet, run: boolean) {
    if (!this.type(snippet.script, run)) {
      void showError("No terminal to run it in", "Open a connected terminal tab, then run the snippet.");
    }
  }

  private render() {
    const visible = this.snippets
      .filter((s) => [s.name, s.script].some((value) => value.toLowerCase().includes(this.filterText)))
      .sort((a, b) => a.name.localeCompare(b.name));
    if (visible.length === 0) {
      const message = this.snippets.length === 0 ? "No snippets yet. Save commands you run often." : "No matching snippets.";
      this.list.replaceChildren(element("p", "host-list-empty", message));
      return;
    }
    this.list.replaceChildren(...visible.map((snippet) => this.item(snippet)));
  }

  private item(snippet: Snippet): HTMLElement {
    const item = element("div", "host-item");
    item.tabIndex = 0;
    item.title = `${snippet.script}\n\nClick: run in the active terminal`;
    const avatar = element("span", "host-avatar");
    avatar.append(icon("snippet"));
    const text = element("div", "host-text");
    const firstLine = snippet.script.split("\n").find((line) => line.trim()) ?? "";
    text.append(element("span", "host-name", snippet.name), element("span", "host-address", firstLine));

    const button = (name: IconName, label: string, run: () => void) =>
      iconButton(name, label, (event) => {
        event.stopPropagation();
        run();
      }, "icon-button small");
    const actions = element("div", "host-actions");
    actions.append(
      button("paste", "Paste without running", () => this.send(snippet, false)),
      button("pencil", "Edit", () => this.editor.open(snippet)),
      button("trash", "Delete", () => void this.remove(snippet)),
    );
    item.append(avatar, text, actions);
    item.addEventListener("click", () => this.send(snippet, true));
    item.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && event.target === item) this.send(snippet, true);
    });
    return item;
  }
}

class SnippetEditor {
  private readonly dialog = document.querySelector<HTMLDialogElement>("#snippet-dialog")!;
  private readonly form = document.querySelector<HTMLFormElement>("#snippet-form")!;
  private readonly error = document.querySelector<HTMLElement>("#snippet-form-error")!;
  private readonly name = this.form.elements.namedItem("name") as HTMLInputElement;
  private readonly script = this.form.elements.namedItem("script") as HTMLTextAreaElement;
  private editing?: Snippet;

  constructor(private readonly onSaved: (snippet: Snippet) => void) {
    this.form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.save();
    });
    this.form.querySelector("[data-close]")!.addEventListener("click", () => this.dialog.close());
  }

  open(snippet?: Snippet) {
    this.editing = snippet;
    this.form.reset();
    this.error.textContent = "";
    document.querySelector("#snippet-dialog-title")!.textContent = snippet ? "Edit snippet" : "New snippet";
    this.name.value = snippet?.name ?? "";
    this.script.value = snippet?.script ?? "";
    this.dialog.showModal();
    this.name.focus();
  }

  private async save() {
    const draft: SnippetDraft = {
      id: this.editing?.id,
      name: this.name.value.trim(),
      script: this.script.value,
    };
    try {
      this.onSaved(await api.saveSnippet(draft));
      this.dialog.close();
    } catch (error) {
      this.error.textContent = api.asCommandError(error).message;
    }
  }
}
