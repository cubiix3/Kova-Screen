/**
 * The Recent Captures window.
 *
 * A list, not a media manager: thumbnail, name, date, type, size, upload state,
 * and the actions from the product spec. No tags, no albums, no search.
 *
 * Thumbnails are loaded straight from the capture folder through the Tauri
 * asset protocol, so nothing is copied or cached to build this view.
 */

import { listen } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api, describeError } from "./ipc";
import type { Capture } from "./ipc";
import { brandMark, formatDate, formatSize, h, render, toast } from "./dom";

let captures: Capture[] = [];
let busy: number | null = null;
let hasMore = false;
let loadingOlder = false;
let generation = 0;
const PAGE_SIZE = 100;

export async function mountHistory(root: HTMLElement): Promise<void> {
  await reload(root);
  await listen("history-changed", () => {
    if (busy === null) void reload(root);
  });
}

async function reload(root: HTMLElement): Promise<void> {
  const current = ++generation;
  try {
    const page = await api.getHistory(PAGE_SIZE + 1, 0);
    if (current !== generation) return;
    captures = page.slice(0, PAGE_SIZE);
    hasMore = page.length > PAGE_SIZE;
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

async function loadOlder(root: HTMLElement): Promise<void> {
  if (loadingOlder || !hasMore) return;
  loadingOlder = true;
  const current = generation;
  try {
    const page = await api.getHistory(PAGE_SIZE + 1, captures.length);
    if (current !== generation) return;
    captures.push(...page.slice(0, PAGE_SIZE));
    hasMore = page.length > PAGE_SIZE;
    draw(root);
  } catch (error) {
    toast(describeError(error), "error");
  } finally {
    loadingOlder = false;
  }
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
            "No captures yet. Press Print Screen, then drag a region or click a window.",
          )
        : h(
            "div",
            { class: "history" },
            ...captures.map((c) => row(root, c)),
            hasMore
              ? h("button", { disabled: loadingOlder, onClick: () => void loadOlder(root) }, "Load older")
              : null,
          ),
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
          await run(root, async () => {
            const removed = await api.pruneMissing();
            toast(removed > 0 ? `Forgot ${removed} missing file(s).` : "Nothing to clean up.");
          });
        },
      },
      "Clean up",
    ),
  );
}

function row(root: HTMLElement, capture: Capture): HTMLElement {
  const isImage = capture.local_exists && (capture.kind === "screenshot" || capture.kind === "gif");
  const disabled = busy !== null;

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
      capture.local_exists ? action("Open", () => api.openCapture(capture.id)) : null,
      capture.local_exists ? action("Folder", () => api.revealCapture(capture.id)) : null,
      capture.local_exists && capture.kind === "screenshot"
        ? action("Image", () =>
            api.copyCaptureImage(capture.id).then(() => toast("Image copied.")),
          )
        : null,
      capture.local_exists
        ? action("File", () => api.copyCaptureFile(capture.id).then(() => toast("File copied.")))
        : null,
      capture.local_exists
        ? action("Path", () => api.copyCapturePath(capture.id).then(() => toast("Path copied.")))
        : null,
      capture.upload_state === "uploaded"
        ? action("URL", () =>
            api.copyCaptureUrl(capture.id).then(() => toast("Link copied.")),
          )
        : !capture.local_exists || capture.kind === "video" || capture.upload_state === "uploading"
          ? null
          : action(busy === capture.id ? "Uploading…" : "Upload", async () => {
              busy = capture.id;
              draw(root);
              const url = await api.uploadCapture(capture.id);
              toast(`Uploaded: ${url}`);
            }),
      capture.can_delete_online
        ? action(
            "Delete online",
            async () => {
              if (!window.confirm(`Delete the online copy of ${capture.file_name}?`)) return;
              busy = capture.id;
              draw(root);
              await api.deleteCaptureUpload(capture.id);
              toast("Online copy deleted.");
            },
            "danger",
          )
        : null,
      capture.local_exists
        ? action(
            "Delete",
            async () => {
              if (!window.confirm(`Delete ${capture.file_name}?`)) return;
              await api.deleteCaptureFile(capture.id);
              toast(capture.upload_state === "uploaded"
                ? "Local file deleted. The online copy remains in Recent Captures."
                : "Deleted.");
            },
            "danger",
          )
        : null,
      !capture.local_exists
        ? action(
            "Forget",
            async () => {
              const warning = capture.upload_state === "uploaded"
                ? "The online copy will remain, and Kova Screen will lose its deletion link."
                : "";
              if (!window.confirm(`Forget ${capture.file_name}? ${warning}`)) return;
              await api.forgetCapture(capture.id);
              toast("Capture forgotten.");
            },
            "danger",
          )
        : null,
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
    src: convertFileSrc(capture.path),
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

function uploadBadge(capture: Capture): HTMLElement | string {
  if (!capture.local_exists) {
    return h("span", { class: "badge" }, capture.upload_state === "uploaded"
      ? "Uploaded · local file removed"
      : "Local file removed");
  }
  switch (capture.upload_state) {
    case "uploading":
      return h("span", { class: "badge" }, "Uploading");
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
