/**
 * The Recent Captures window.
 *
 * A list, not a media manager: thumbnail, name, date, type, size, upload state,
 * and the actions from the product spec. No tags, no albums, no search.
 *
 * Thumbnails are loaded straight from the capture folder through the Tauri
 * asset protocol, so nothing is copied or cached to build this view.
 */

import { api, describeError } from "./ipc";
import type { Capture } from "./ipc";
import { brandMark, formatDate, formatSize, h, render, toast } from "./dom";

let captures: Capture[] = [];
let busy: number | null = null;

export async function mountHistory(root: HTMLElement): Promise<void> {
  await reload(root);
}

async function reload(root: HTMLElement): Promise<void> {
  try {
    captures = await api.getHistory(200);
  } catch (error) {
    render(
      root,
      toolbar(root),
      h("div", { class: "empty" }, describeError(error)),
    );
    return;
  }
  draw(root);
}

function draw(root: HTMLElement): void {
  render(
    root,
    toolbar(root),
    h(
      "main",
      { class: "content" },
      captures.length === 0
        ? h(
            "div",
            { class: "empty" },
            "No captures yet. Press Print Screen to take one.",
          )
        : h("div", { class: "history" }, ...captures.map((c) => row(root, c))),
    ),
  );
}

function toolbar(root: HTMLElement): HTMLElement {
  return h(
    "header",
    { class: "toolbar" },
    brandMark(),
    h("strong", {}, "Recent Captures"),
    h("div", { class: "toolbar__spacer" }),
    h(
      "button",
      { onClick: () => void run(root, () => api.runAction("screenshot_region")) },
      "New capture",
    ),
    h(
      "button",
      { onClick: () => void run(root, () => api.openCaptureFolder()) },
      "Open folder",
    ),
    h(
      "button",
      {
        onClick: async () => {
          const removed = await api.pruneMissing().catch(() => 0);
          toast(removed > 0 ? `Forgot ${removed} missing file(s).` : "Nothing to clean up.");
          await reload(root);
        },
      },
      "Clean up",
    ),
  );
}

function row(root: HTMLElement, capture: Capture): HTMLElement {
  const isImage = capture.kind === "screenshot" || capture.kind === "gif";
  const disabled = busy === capture.id;

  const action = (label: string, run_: () => Promise<unknown>, className?: string) =>
    h(
      "button",
      {
        class: className ?? "",
        disabled,
        onClick: () => void run(root, run_),
      },
      label,
    );

  return h(
    "div",
    { class: "capture" },
    isImage ? thumbnail(capture) : placeholder(capture),
    h(
      "div",
      { class: "capture__meta" },
      h("div", { class: "capture__name" }, capture.file_name),
      h(
        "div",
        { class: "capture__sub" },
        `${formatDate(capture.created_at)} · ${formatSize(capture.size_bytes)}`,
        capture.width > 0 ? ` · ${capture.width}×${capture.height}` : "",
        " ",
        uploadBadge(capture),
      ),
    ),
    h(
      "div",
      { class: "capture__actions" },
      action("Open", () => api.openCapture(capture.id)),
      action("Folder", () => api.revealCapture(capture.id)),
      action("Copy", () => api.copyCaptureFile(capture.id).then(() => toast("File copied."))),
      action("Path", () => api.copyCapturePath(capture.id).then(() => toast("Path copied."))),
      capture.upload_state === "uploaded"
        ? action("URL", () =>
            api.copyCaptureUrl(capture.id).then(() => toast("Link copied.")),
          )
        : action("Upload", async () => {
            busy = capture.id;
            draw(root);
            const url = await api.uploadCapture(capture.id);
            toast(`Uploaded: ${url}`);
          }),
      capture.upload_state === "uploaded"
        ? action(
            "Delete online",
            () =>
              api
                .deleteCaptureUpload(capture.id)
                .then(() => toast("Online copy deleted.")),
            "danger",
          )
        : null,
      action("Delete", () => api.deleteCaptureFile(capture.id), "danger"),
    ),
  );
}

/**
 * Renders a thumbnail from the capture file itself.
 *
 * A capture whose file was moved or deleted outside the app falls back to the
 * placeholder rather than showing a broken image.
 */
function thumbnail(capture: Capture): HTMLElement {
  const img = h("img", {
    class: "capture__thumb",
    alt: "",
    loading: "lazy",
    decoding: "async",
    src: assetUrl(capture.path),
    onError: () => img.replaceWith(placeholder(capture)),
  }) as HTMLImageElement;
  return img;
}

function placeholder(capture: Capture): HTMLElement {
  return h(
    "div",
    { class: "capture__thumb capture__thumb--placeholder" },
    capture.kind === "video" ? "MP4" : capture.kind === "gif" ? "GIF" : "IMG",
  );
}

/** Builds the `asset:` URL the WebView can load a local file from. */
function assetUrl(path: string): string {
  // Tauri exposes local files under a custom scheme; the path has to be encoded
  // because Windows paths contain backslashes, colons and spaces.
  return `http://asset.localhost/${encodeURIComponent(path)}`;
}

function uploadBadge(capture: Capture): HTMLElement | string {
  switch (capture.upload_state) {
    case "uploaded":
      return h("span", { class: "badge uploaded" }, "Uploaded");
    case "failed":
      return h("span", { class: "badge failed" }, "Upload failed");
    default:
      return "";
  }
}

/** Runs an action, reports failures, and refreshes the list. */
async function run(root: HTMLElement, action: () => Promise<unknown>): Promise<void> {
  try {
    await action();
  } catch (error) {
    toast(describeError(error), "error");
  } finally {
    busy = null;
    await reload(root);
  }
}
