/**
 * The typed edge of the IPC boundary.
 *
 * Every shape here mirrors a Rust type. Notably absent: anything that returns
 * the vgy.me user key. The backend has no command for it by design, so this
 * file cannot offer one either -- the key is written and its presence queried,
 * never read back into the WebView.
 */

import { invoke } from "@tauri-apps/api/core";

export type ImageFormat = "png" | "jpeg" | "webp";
export type UrlKind = "direct-image" | "page-url";
export type Theme = "dark" | "light" | "system";
export type RecordingQuality = "low" | "medium" | "high";
export type CaptureKind = "screenshot" | "gif" | "video";
export type UploadState = "none" | "uploaded" | "failed";

export interface GeneralSettings {
  launch_with_windows: boolean;
  start_minimized: boolean;
  notifications: boolean;
  theme: Theme;
}

export interface CaptureSettings {
  format: ImageFormat;
  quality: number;
  copy_to_clipboard: boolean;
  include_cursor: boolean;
  delay_ms: number;
  play_sound: boolean;
}

export interface RecordingSettings {
  mp4_fps: number;
  gif_fps: number;
  quality: RecordingQuality;
  include_cursor: boolean;
  hardware_encoding: boolean;
  max_duration_secs: number;
  gif_max_size_mb: number;
}

export interface UploadSettings {
  enabled: boolean;
  auto_upload_screenshots: boolean;
  auto_upload_gifs: boolean;
  auto_upload_recordings: boolean;
  url_kind: UrlKind;
  copy_url: boolean;
  copy_recording_path: boolean;
  max_upload_mb: number;
}

export interface StorageSettings {
  capture_dir: string | null;
  filename_template: string;
  history_limit: number;
}

export interface HotkeySettings {
  region_screenshot: string;
  fullscreen_screenshot: string;
  window_screenshot: string;
  record_mp4: string;
  record_gif: string;
  stop_recording: string;
}

export interface Settings {
  general: GeneralSettings;
  capture: CaptureSettings;
  recording: RecordingSettings;
  upload: UploadSettings;
  storage: StorageSettings;
  hotkeys: HotkeySettings;
}

export interface HotkeyFailure {
  action: string;
  binding: string;
  reason: string;
  taken_by_another_app: boolean;
}

export interface SettingsView {
  settings: Settings;
  /** Whether a key is stored. Never the key itself. */
  has_user_key: boolean;
  hotkey_failures: HotkeyFailure[];
  capture_dir: string;
  version: string;
  webp_is_lossless: boolean;
}

export interface Capture {
  id: number;
  path: string;
  file_name: string;
  kind: CaptureKind;
  created_at: number;
  size_bytes: number;
  width: number;
  height: number;
  upload_state: UploadState;
  page_url: string | null;
  direct_url: string | null;
  /* `delete_url` is deliberately not serialised by the backend. */
}

export type ActionId =
  | "screenshot_region"
  | "screenshot_fullscreen"
  | "screenshot_all_monitors"
  | "screenshot_window"
  | "record_mp4"
  | "record_gif"
  | "stop_recording";

export const api = {
  getSettings: () => invoke<SettingsView>("get_settings"),
  saveSettings: (settings: Settings) =>
    invoke<SettingsView>("save_settings", { settings }),
  setUserKey: (key: string) => invoke<boolean>("set_user_key", { key }),
  testUserKey: () => invoke<string>("test_user_key"),
  setCaptureDir: (dir: string) => invoke<string>("set_capture_dir", { dir }),

  getHistory: (limit?: number) => invoke<Capture[]>("get_history", { limit }),
  openCapture: (id: number) => invoke<void>("open_capture", { id }),
  revealCapture: (id: number) => invoke<void>("reveal_capture", { id }),
  openCaptureFolder: () => invoke<void>("open_capture_folder"),
  copyCaptureFile: (id: number) => invoke<void>("copy_capture_file", { id }),
  copyCapturePath: (id: number) => invoke<void>("copy_capture_path", { id }),
  copyCaptureUrl: (id: number) => invoke<void>("copy_capture_url", { id }),
  uploadCapture: (id: number) => invoke<string>("upload_capture", { id }),
  deleteCaptureFile: (id: number) => invoke<void>("delete_capture_file", { id }),
  deleteCaptureUpload: (id: number) =>
    invoke<void>("delete_capture_upload", { id }),
  pruneMissing: () => invoke<number>("prune_missing"),

  runAction: (action: ActionId) => invoke<void>("run_action", { action }),
  recordingStatus: () => invoke<boolean>("recording_status"),
};

/**
 * Turns a rejected IPC call into a readable message.
 *
 * Backend commands reject with a plain string that is already user-facing, so
 * the common case is a pass-through; the fallbacks only cover a genuine
 * transport failure.
 */
export function describeError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Something went wrong.";
}
