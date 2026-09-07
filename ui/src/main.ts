/**
 * Entry point for both windows.
 *
 * One bundle serves Settings and Recent Captures; the `?view=` parameter the
 * backend puts in the URL decides which mounts. Shipping one document keeps the
 * WebView payload small and means opening the second window costs no extra
 * parse.
 */

import { mountHistory } from "./history";
import { mountSettings } from "./settings";
import { toast } from "./dom";

const VIEWS = {
  settings: mountSettings,
  history: mountHistory,
} as const;

type ViewName = keyof typeof VIEWS;

function currentView(): ViewName {
  const requested = new URLSearchParams(window.location.search).get("view");
  return requested === "history" ? "history" : "settings";
}

async function main(): Promise<void> {
  const root = document.getElementById("root");
  if (!root) return;

  document.title =
    currentView() === "history"
      ? "Kova Screen — Recent Captures"
      : "Kova Screen — Settings";

  try {
    await VIEWS[currentView()](root);
  } catch (error) {
    // A mount failure would otherwise leave a blank window with no explanation.
    console.error(error);
    toast("This window could not be loaded.", "error");
  }
}

// The context menu is a browser affordance; this is an app window.
window.addEventListener("contextmenu", (event) => event.preventDefault());

void main();
