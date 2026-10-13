// Password fields with an eye toggle: masked by default, revealed text can be selected and copied.
import { icon } from "./icons";

export interface SecretField {
  /** Masks the value again, e.g. when its dialog reopens. */
  conceal(): void;
}

export function secretField(input: HTMLInputElement): SecretField {
  const wrapper = document.createElement("span");
  wrapper.className = "secret-field";
  input.replaceWith(wrapper);

  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "reveal-button";
  wrapper.append(input, toggle);

  const show = (revealed: boolean) => {
    input.type = revealed ? "text" : "password";
    const label = revealed ? "Hide" : "Show";
    toggle.title = label;
    toggle.setAttribute("aria-label", label);
    toggle.setAttribute("aria-pressed", String(revealed));
    toggle.replaceChildren(icon(revealed ? "eyeOff" : "eye"));
  };
  // Keep the caret in the field so the toggle doesn't interrupt typing.
  toggle.addEventListener("mousedown", (event) => event.preventDefault());
  toggle.addEventListener("click", () => show(input.type === "password"));
  show(false);

  return { conceal: () => show(false) };
}
