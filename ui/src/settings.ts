/**
 * The settings window.
 *
 * Seven sections, one screen each, matching the product spec. Changes are saved
 * on edit rather than behind a Save button: every field maps to exactly one
 * setting, so there is nothing to batch and nothing to discard.
 *
 * The vgy.me key is the one exception. It is write-only across the IPC
 * boundary, so the field shows a placeholder when a key is stored and only
 * sends a value when the user types a new one.
 */

import { api, describeError } from "./ipc";
import type {
  ImageFormat,
  RecordingQuality,
  Settings,
  SettingsView,
  Theme,
  UrlKind,
} from "./ipc";
import {
  brandMark,
  field,
  h,
  numberInput,
  render,
  select,
  toast,
  toggle,
} from "./dom";

type Section = "general" | "capture" | "recording" | "upload" | "hotkeys" | "storage" | "about";

const SECTIONS: readonly (readonly [Section, string])[] = [
  ["general", "General"],
  ["capture", "Capture"],
  ["recording", "Recording"],
  ["upload", "Upload"],
  ["hotkeys", "Hotkeys"],
  ["storage", "Storage"],
  ["about", "About"],
];

let view: SettingsView;
let section: Section = "general";

export async function mountSettings(root: HTMLElement): Promise<void> {
  try {
    view = await api.getSettings();
  } catch (error) {
    render(root, h("div", { class: "empty" }, describeError(error)));
    return;
  }
  draw(root);
}

function draw(root: HTMLElement): void {
  const sidebar = h(
    "nav",
    { class: "sidebar" },
    h("div", { class: "sidebar__brand" }, brandMark(), "Kova Screen"),
    ...SECTIONS.map(([id, label]) =>
      h(
        "button",
        {
          class: "nav-item",
          role: "tab",
          "aria-selected": String(id === section),
          onClick: () => {
            section = id;
            draw(root);
          },
        },
        label,
      ),
    ),
    h("div", { class: "sidebar__version" }, `v${view.version}`),
  );

  const content = h("main", { class: "content" }, ...body(root));
  render(root, h("div", { class: "shell" }, sidebar, content));
}

/** Persists a mutated settings object and refreshes the view. */
async function commit(root: HTMLElement, mutate: (settings: Settings) => void): Promise<void> {
  const next = structuredClone(view.settings);
  mutate(next);
  try {
    view = await api.saveSettings(next);
  } catch (error) {
    toast(describeError(error), "error");
    // Re-read, so the UI shows what was actually stored rather than the value
    // the user tried to set.
    view = await api.getSettings();
  }
  draw(root);
}

function body(root: HTMLElement): HTMLElement[] {
  const s = view.settings;

  switch (section) {
    case "general":
      return [
        heading("General", "How Kova Screen behaves on your desktop."),
        field(
          "Launch with Windows",
          "Starts in the tray when you sign in.",
          toggle(s.general.launch_with_windows, (v) =>
            void commit(root, (n) => { n.general.launch_with_windows = v; }),
          ),
        ),
        field(
          "Start minimized",
          "Skip this window and go straight to the tray.",
          toggle(s.general.start_minimized, (v) =>
            void commit(root, (n) => { n.general.start_minimized = v; }),
          ),
        ),
        field(
          "Notifications",
          "Show a toast after each capture.",
          toggle(s.general.notifications, (v) =>
            void commit(root, (n) => { n.general.notifications = v; }),
          ),
        ),
        field(
          "Theme",
          null,
          select<Theme>(
            [["dark", "Dark"], ["light", "Light"], ["system", "System"]],
            s.general.theme,
            (v) => void commit(root, (n) => { n.general.theme = v; }),
          ),
        ),
      ];

    case "capture":
      return [
        heading("Capture", "What a screenshot looks like and what happens to it."),
        field(
          "Format",
          null,
          select<ImageFormat>(
            [["png", "PNG"], ["jpeg", "JPEG"], ["webp", "WebP"]],
            s.capture.format,
            (v) => void commit(root, (n) => { n.capture.format = v; }),
          ),
        ),
        s.capture.format === "jpeg"
          ? field(
              "JPEG quality",
              `${s.capture.quality}`,
              h("input", {
                type: "range",
                min: "1",
                max: "100",
                value: String(s.capture.quality),
                onChange: (e: Event) =>
                  void commit(root, (n) => {
                    n.capture.quality = Number((e.target as HTMLInputElement).value);
                  }),
              }),
            )
          : null,
        s.capture.format === "webp" && view.webp_is_lossless
          ? note(
              "WebP is encoded losslessly, so there is no quality setting. " +
                "Lossless WebP is usually smaller than PNG for a screenshot.",
            )
          : null,
        field(
          "Copy to clipboard",
          "Put the image on the clipboard as well as saving it.",
          toggle(s.capture.copy_to_clipboard, (v) =>
            void commit(root, (n) => { n.capture.copy_to_clipboard = v; }),
          ),
        ),
        field(
          "Include cursor",
          "Draw the mouse pointer into the screenshot.",
          toggle(s.capture.include_cursor, (v) =>
            void commit(root, (n) => { n.capture.include_cursor = v; }),
          ),
        ),
        field(
          "Capture delay",
          "Milliseconds to wait before the shutter fires.",
          numberInput(s.capture.delay_ms, 0, 10000, (v) =>
            void commit(root, (n) => { n.capture.delay_ms = v; }),
          ),
        ),
        h("div", { style: "height:18px" }),
        h(
          "div",
          { class: "field__control" },
          h("button", { class: "primary", onClick: () => void trigger("screenshot_region") }, "Test region capture"),
          h("button", { onClick: () => void trigger("screenshot_fullscreen") }, "Test fullscreen"),
        ),
      ].filter(Boolean) as HTMLElement[];

    case "recording":
      return [
        heading("Recording", "MP4 and GIF capture settings."),
        field(
          "MP4 frame rate",
          null,
          select(
            [["30", "30 FPS"], ["60", "60 FPS"]] as const,
            String(s.recording.mp4_fps) as "30" | "60",
            (v) => void commit(root, (n) => { n.recording.mp4_fps = Number(v); }),
          ),
        ),
        field(
          "GIF frame rate",
          null,
          select(
            [["10", "10 FPS"], ["15", "15 FPS"], ["20", "20 FPS"], ["30", "30 FPS"]] as const,
            String(s.recording.gif_fps) as "10" | "15" | "20" | "30",
            (v) => void commit(root, (n) => { n.recording.gif_fps = Number(v); }),
          ),
        ),
        field(
          "Quality",
          "Sets the video bitrate, scaled to the recorded area.",
          select<RecordingQuality>(
            [["low", "Low"], ["medium", "Medium"], ["high", "High"]],
            s.recording.quality,
            (v) => void commit(root, (n) => { n.recording.quality = v; }),
          ),
        ),
        field(
          "Include cursor",
          null,
          toggle(s.recording.include_cursor, (v) =>
            void commit(root, (n) => { n.recording.include_cursor = v; }),
          ),
        ),
        field(
          "Hardware encoding",
          "Experimental. Some GPU drivers retain resources between recordings. Leave off for stable long sessions.",
          toggle(s.recording.hardware_encoding, (v) =>
            void commit(root, (n) => { n.recording.hardware_encoding = v; }),
          ),
        ),
        field(
          "Maximum length",
          "Recorded seconds before stopping automatically; pauses do not count. 0 disables the limit.",
          numberInput(s.recording.max_duration_secs, 0, 21600, (v) =>
            void commit(root, (n) => { n.recording.max_duration_secs = v; }),
          ),
        ),
        field(
          "GIF size limit",
          "Megabytes. A GIF that reaches this is cut short rather than growing.",
          numberInput(s.recording.gif_max_size_mb, 1, 512, (v) =>
            void commit(root, (n) => { n.recording.gif_max_size_mb = v; }),
          ),
        ),
        note("Audio is not recorded in v0.1."),
      ];

    case "upload":
      return uploadSection(root, s);

    case "hotkeys":
      return hotkeysSection(root, s);

    case "storage":
      return [
        heading("Storage", "Where captures are written."),
        field(
          "Capture folder",
          view.capture_dir,
          h(
            "div",
            { class: "field__control" },
            h("button", { onClick: () => void chooseFolder(root) }, "Change"),
            h("button", { onClick: () => void api.openCaptureFolder().catch(showError) }, "Open"),
          ),
        ),
        field(
          "Filename template",
          "Tokens: {date} {time} {year} {month} {day} {hour} {minute} {second}",
          h("input", {
            type: "text",
            value: s.storage.filename_template,
            style: "min-width:230px",
            onChange: (e: Event) =>
              void commit(root, (n) => {
                n.storage.filename_template = (e.target as HTMLInputElement).value;
              }),
          }),
        ),
        field(
          "History entries",
          "Older entries are forgotten. Files are never deleted.",
          numberInput(s.storage.history_limit, 0, 100000, (v) =>
            void commit(root, (n) => { n.storage.history_limit = v; }),
          ),
        ),
      ];

    case "about":
      return [
        heading("About", "Kova Screen"),
        h(
          "p",
          { style: "color:var(--secondary);max-width:56ch" },
          "Fast, minimal screen capture for Windows. Screenshots, GIF and MP4 " +
            "recording, clipboard integration and optional vgy.me upload.",
        ),
        note(
          "Kova Screen works entirely offline. Nothing leaves your PC unless " +
            "you turn on uploads.",
        ),
        field("Version", null, h("span", { class: "capture__sub" }, view.version)),
        field(
          "License",
          null,
          h("span", { class: "capture__sub" }, "MIT OR Apache-2.0"),
        ),
      ];
  }
}

function uploadSection(root: HTMLElement, s: Settings): HTMLElement[] {
  const keyInput = h("input", {
    type: "password",
    placeholder: view.has_user_key ? "••••• saved" : "Optional",
    style: "min-width:200px",
    autocomplete: "off",
  }) as HTMLInputElement;

  return [
    heading("Upload", "Optional. Off by default."),
    field(
      "Enable uploads",
      "Nothing is ever sent unless this is on.",
      toggle(s.upload.enabled, (v) =>
        void commit(root, (n) => { n.upload.enabled = v; }),
      ),
    ),
    h("div", { class: "note" }, h("strong", {}, "vgy.me"), " is the only provider in v0.1."),
    field(
      "User key",
      "Stored in Windows Credential Manager, never in a config file.",
      h(
        "div",
        { class: "field__control" },
        keyInput,
        h(
          "button",
          {
            onClick: async () => {
              try {
                const saved = await api.setUserKey(keyInput.value);
                keyInput.value = "";
                view = await api.getSettings();
                toast(saved ? "Key saved." : "Key cleared.");
                draw(root);
              } catch (error) {
                showError(error);
              }
            },
          },
          "Save",
        ),
        h(
          "button",
          {
            onClick: async () => {
              try {
                toast(await api.testUserKey());
              } catch (error) {
                showError(error);
              }
            },
          },
          "Test",
        ),
      ),
    ),
    field(
      "Auto-upload screenshots",
      null,
      toggle(s.upload.auto_upload_screenshots, (v) =>
        void commit(root, (n) => { n.upload.auto_upload_screenshots = v; }),
      ),
    ),
    field(
      "Auto-upload GIFs",
      null,
      toggle(s.upload.auto_upload_gifs, (v) =>
        void commit(root, (n) => { n.upload.auto_upload_gifs = v; }),
      ),
    ),
    field(
      "Auto-upload recordings",
      "vgy.me does not accept MP4; these are kept locally.",
      toggle(s.upload.auto_upload_recordings, (v) =>
        void commit(root, (n) => { n.upload.auto_upload_recordings = v; }),
      ),
    ),
    field(
      "Copy",
      null,
      select<UrlKind>(
        [["direct-image", "Direct image URL"], ["page-url", "vgy.me page URL"]],
        s.upload.url_kind,
        (v) => void commit(root, (n) => { n.upload.url_kind = v; }),
      ),
    ),
    field(
      "Copy URL after upload",
      null,
      toggle(s.upload.copy_url, (v) =>
        void commit(root, (n) => { n.upload.copy_url = v; }),
      ),
    ),
    field(
      "Copy recording path",
      "Put the file path on the clipboard when a recording is saved.",
      toggle(s.upload.copy_recording_path, (v) =>
        void commit(root, (n) => { n.upload.copy_recording_path = v; }),
      ),
    ),
    field(
      "Upload size limit",
      "Megabytes.",
      numberInput(s.upload.max_upload_mb, 1, 1024, (v) =>
        void commit(root, (n) => { n.upload.max_upload_mb = v; }),
      ),
    ),
  ];
}

const HOTKEY_FIELDS: readonly (readonly [keyof Settings["hotkeys"], string])[] = [
  ["region_screenshot", "Region screenshot"],
  ["fullscreen_screenshot", "Fullscreen screenshot"],
  ["window_screenshot", "Window screenshot"],
  ["record_mp4", "Record MP4"],
  ["record_gif", "Record GIF"],
  ["stop_recording", "Stop recording"],
];

function hotkeysSection(root: HTMLElement, s: Settings): HTMLElement[] {
  const failures = view.hotkey_failures;

  return [
    heading("Hotkeys", "Click a field, then press the combination you want."),
    ...failures.map((failure) =>
      h(
        "div",
        { class: "note warn" },
        h("strong", {}, failure.binding || "A hotkey"),
        ` — ${failure.reason}`,
      ),
    ),
    ...HOTKEY_FIELDS.map(([key, label]) =>
      field(label, null, hotkeyInput(root, s.hotkeys[key], key)),
    ),
    note("Press Escape while recording a hotkey to clear it."),
  ];
}

/**
 * A field that captures the next key combination the user presses.
 *
 * Modifier-only presses are ignored while the user is still reaching for the
 * final key, so holding Ctrl and then pressing R records `Ctrl+R` rather than
 * committing `Ctrl` on the first keydown.
 */
function hotkeyInput(
  root: HTMLElement,
  value: string,
  key: keyof Settings["hotkeys"],
): HTMLElement {
  const input = h("input", {
    type: "text",
    class: "hotkey-input",
    value,
    readonly: true,
    onFocus: () => {
      input.classList.add("recording");
      input.value = "Press a combination…";
    },
    onBlur: () => {
      input.classList.remove("recording");
      input.value = view.settings.hotkeys[key];
    },
    onKeydown: (event: Event) => {
      const e = event as KeyboardEvent;
      e.preventDefault();
      // Holding a key must not send overlapping save/re-registration requests.
      if (e.repeat) return;

      if (e.key === "Escape") {
        void commit(root, (n) => { n.hotkeys[key] = ""; });
        return;
      }
      if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;

      const parts: string[] = [];
      if (e.ctrlKey) parts.push("Ctrl");
      if (e.shiftKey) parts.push("Shift");
      if (e.altKey) parts.push("Alt");
      if (e.metaKey) parts.push("Win");
      parts.push(normaliseKey(e));

      void commit(root, (n) => { n.hotkeys[key] = parts.join("+"); });
    },
  }) as HTMLInputElement;

  return input;
}

/** Maps a `KeyboardEvent` onto the spelling the Rust parser accepts. */
function normaliseKey(event: KeyboardEvent): string {
  const key = event.key;

  const named: Record<string, string> = {
    PrintScreen: "PrintScreen",
    Insert: "Insert",
    Delete: "Delete",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    " ": "Space",
    Enter: "Enter",
    Tab: "Tab",
    Backspace: "Backspace",
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
    Pause: "Pause",
  };
  return named[key] ?? (key.length === 1 ? key.toUpperCase() : key);
}

async function chooseFolder(root: HTMLElement): Promise<void> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const chosen = await open({ directory: true, multiple: false, title: "Capture folder" });
  if (typeof chosen !== "string") return;

  try {
    await api.setCaptureDir(chosen);
    view = await api.getSettings();
    draw(root);
    toast("Capture folder updated.");
  } catch (error) {
    showError(error);
  }
}

async function trigger(action: Parameters<typeof api.runAction>[0]): Promise<void> {
  try {
    await api.runAction(action);
  } catch (error) {
    showError(error);
  }
}

function heading(title: string, lede: string): HTMLElement {
  return h("header", {}, h("h1", {}, title), h("p", { class: "lede" }, lede));
}

function note(text: string): HTMLElement {
  return h("div", { class: "note" }, text);
}

function showError(error: unknown): void {
  toast(describeError(error), "error");
}
