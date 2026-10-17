// The dialog that picks a serial port and speed for a new serial terminal.
import * as api from "./api";
import { element } from "./format";
import { icon } from "./icons";

const dialog = document.querySelector<HTMLDialogElement>("#serial-dialog")!;
const form = document.querySelector<HTMLFormElement>("#serial-form")!;
const error = document.querySelector<HTMLElement>("#serial-form-error")!;
const portSelect = form.elements.namedItem("path") as HTMLSelectElement;
const baudSelect = form.elements.namedItem("baudRate") as HTMLSelectElement;
const refresh = form.querySelector<HTMLButtonElement>("[data-refresh]")!;
refresh.append(icon("refresh"));

/** Remembered per device, so the usual port and speed come preselected. */
const LAST_CHOICE = "hostly.serial";

function remembered(): { path?: string; baudRate?: number } {
  try {
    return JSON.parse(localStorage.getItem(LAST_CHOICE) ?? "{}");
  } catch {
    return {};
  }
}

async function loadPorts() {
  error.textContent = "";
  portSelect.replaceChildren(element("option", "", "Looking for ports…"));
  try {
    const ports = await api.listSerialPorts();
    if (ports.length === 0) {
      const none = element("option", "", "No serial ports found");
      none.value = "";
      portSelect.replaceChildren(none);
      return;
    }
    portSelect.replaceChildren(
      ...ports.map((port) => {
        const option = element("option", "", port.description ? `${port.name} · ${port.description}` : port.name);
        option.value = port.name;
        return option;
      }),
    );
    const last = remembered().path;
    if (last && ports.some((port) => port.name === last)) portSelect.value = last;
  } catch (failure) {
    portSelect.replaceChildren();
    error.textContent = api.asCommandError(failure).message;
  }
}

refresh.addEventListener("click", () => void loadPorts());
form.querySelector("[data-close]")!.addEventListener("click", () => dialog.close());

/** Resolves to the chosen port and speed, or null if cancelled. */
export function chooseSerialPort(): Promise<{ path: string; baudRate: number } | null> {
  form.reset();
  const last = remembered();
  if (last.baudRate) baudSelect.value = String(last.baudRate);
  void loadPorts();
  dialog.showModal();
  baudSelect.focus();
  return new Promise((resolve) => {
    const submit = (event: SubmitEvent) => {
      event.preventDefault();
      if (!portSelect.value) {
        error.textContent = "Connect a device, then look for ports again.";
        return;
      }
      const choice = { path: portSelect.value, baudRate: Number(baudSelect.value) };
      try {
        localStorage.setItem(LAST_CHOICE, JSON.stringify(choice));
      } catch {
        // Only a convenience.
      }
      chosen = choice;
      dialog.close();
    };
    let chosen: { path: string; baudRate: number } | null = null;
    form.addEventListener("submit", submit);
    dialog.addEventListener(
      "close",
      () => {
        form.removeEventListener("submit", submit);
        resolve(chosen);
      },
      { once: true },
    );
  });
}
