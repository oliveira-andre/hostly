// Promise-based modal prompts built on the shared #prompt-dialog element.
import { secretField } from "./secret-input";

interface PromptOptions {
  title: string;
  message: string;
  detail?: string;
  confirmLabel?: string;
  danger?: boolean;
  /** Shows an input and resolves to its value. */
  input?: "secret" | "text";
  value?: string;
  /** Informational: only an OK button. */
  notice?: boolean;
  /** Shows a "Save in keychain" checkbox, checked by default. */
  remember?: boolean;
}

const dialog = document.querySelector<HTMLDialogElement>("#prompt-dialog")!;
const field = <T extends Element>(name: string) => dialog.querySelector<T>(`[data-${name}]`)!;
const secretInput = field<HTMLInputElement>("secret");
const textInput = field<HTMLInputElement>("text");
const rememberInput = field<HTMLInputElement>("remember");
const secret = secretField(secretInput);

function open(options: PromptOptions): Promise<string | null> {
  field("title").textContent = options.title;
  field("message").textContent = options.message;
  const detail = field<HTMLElement>("detail");
  detail.textContent = options.detail ?? "";
  detail.hidden = !options.detail;

  secret.conceal();
  secretInput.closest<HTMLElement>(".secret-field")!.hidden = options.input !== "secret";
  textInput.hidden = options.input !== "text";
  field<HTMLElement>("remember-row").hidden = !options.remember;
  rememberInput.checked = true;
  const input = options.input === "text" ? textInput : secretInput;
  input.value = options.value ?? "";

  const confirm = field<HTMLButtonElement>("confirm");
  confirm.textContent = options.confirmLabel ?? "OK";
  confirm.classList.toggle("danger", options.danger ?? false);
  field<HTMLButtonElement>("close").hidden = options.notice ?? false;

  return new Promise((resolve) => {
    dialog.addEventListener(
      "close",
      () => resolve(dialog.returnValue === "ok" ? input.value : null),
      { once: true },
    );
    dialog.returnValue = "";
    dialog.showModal();
    if (options.input) {
      input.focus();
      input.select();
    } else {
      confirm.focus();
    }
  });
}

field("close").addEventListener("click", () => dialog.close("cancel"));

export async function confirmAction(
  options: Omit<PromptOptions, "input" | "value">,
): Promise<boolean> {
  return (await open(options)) !== null;
}

/** Asks for a password or passphrase, and whether to keep it in the keychain. */
export async function promptCredential(
  options: Omit<PromptOptions, "input" | "remember">,
): Promise<{ secret: string; remember: boolean } | null> {
  const secret = await open({ ...options, input: "secret", remember: true });
  return secret === null ? null : { secret, remember: rememberInput.checked };
}

export function promptText(options: Omit<PromptOptions, "input">): Promise<string | null> {
  return open({ ...options, input: "text" });
}

export async function showError(title: string, message: string): Promise<void> {
  await open({ title, message, notice: true });
}
