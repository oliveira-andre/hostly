// Lucide icons, bundled locally (no CDN).
import {
  ArrowLeft,
  ArrowUp,
  CircleUser,
  Columns2,
  Download,
  Eye,
  EyeOff,
  File,
  FileSymlink,
  Folder,
  FolderOpen,
  FolderPlus,
  HardDrive,
  LogOut,
  Pencil,
  Plus,
  RefreshCw,
  Rows2,
  Settings,
  Search,
  Server,
  SquareTerminal,
  Trash2,
  Upload,
  X,
  createElement,
} from "lucide";

const icons = {
  arrowLeft: ArrowLeft,
  arrowUp: ArrowUp,
  account: CircleUser,
  logOut: LogOut,
  settings: Settings,
  splitRight: Columns2,
  splitDown: Rows2,
  download: Download,
  eye: Eye,
  eyeOff: EyeOff,
  file: File,
  fileSymlink: FileSymlink,
  folder: Folder,
  folderOpen: FolderOpen,
  folderPlus: FolderPlus,
  hardDrive: HardDrive,
  pencil: Pencil,
  plus: Plus,
  refresh: RefreshCw,
  search: Search,
  server: Server,
  terminal: SquareTerminal,
  trash: Trash2,
  upload: Upload,
  close: X,
};

export type IconName = keyof typeof icons;

export function icon(name: IconName, size = 16): SVGElement {
  const svg = createElement(icons[name], { width: size, height: size, "aria-hidden": "true" });
  svg.classList.add("icon");
  return svg;
}

export function iconButton(
  name: IconName,
  label: string,
  onClick: (event: MouseEvent) => void,
  className = "icon-button",
): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = className;
  button.title = label;
  button.setAttribute("aria-label", label);
  button.append(icon(name));
  button.addEventListener("click", onClick);
  return button;
}
