// Hash routes for the full-page views. `#/` (or no hash) shows the sessions.
import { element } from "./format";
import { icon } from "./icons";

export type Route = "sessions" | "account" | "settings";

export function currentRoute(): Route {
  switch (location.hash) {
    case "#/account":
      return "account";
    case "#/settings":
      return "settings";
    default:
      return "sessions";
  }
}

export function navigate(route: Route) {
  location.hash = route === "sessions" ? "#/" : `#/${route}`;
}

/** The top-right buttons open their page, or return to the sessions if it's already open. */
export function toggle(route: Route) {
  navigate(currentRoute() === route ? "sessions" : route);
}

export function backButton(): HTMLElement {
  const back = element("button", "button-ghost back-button");
  back.type = "button";
  back.append(icon("arrowLeft"), element("span", "", "Back to sessions"));
  back.addEventListener("click", () => navigate("sessions"));
  return back;
}
