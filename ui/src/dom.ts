/**
 * A very small DOM helper layer.
 *
 * Two windows of settings and a list do not justify a framework, and a
 * framework would add megabytes to a WebView this app tries hard to keep cheap.
 * `h` builds elements, and the rest are the three widgets used repeatedly.
 */

type Child = Node | string | null | undefined | false;

type Attrs = Record<string, string | number | boolean | EventListener | null | undefined>;

/** Creates an element with attributes and children. */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);

  for (const [key, value] of Object.entries(attrs)) {
    if (value === null || value === undefined || value === false) continue;

    if (key.startsWith("on") && typeof value === "function") {
      element.addEventListener(key.slice(2).toLowerCase(), value as EventListener);
    } else if (key === "class") {
      element.className = String(value);
    } else if (key === "value" && element instanceof HTMLInputElement) {
      element.value = String(value);
    } else if (key === "checked" && element instanceof HTMLInputElement) {
      element.checked = Boolean(value);
    } else if (value === true) {
      element.setAttribute(key, "");
    } else {
      element.setAttribute(key, String(value));
    }
  }

  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    element.append(typeof child === "string" ? document.createTextNode(child) : child);
  }

  return element;
}

/** Replaces the children of `parent`. */
export function render(parent: HTMLElement, ...children: Child[]): void {
  parent.replaceChildren();
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    parent.append(typeof child === "string" ? document.createTextNode(child) : child);
  }
}

/** A labelled settings row. */
export function field(label: string, hint: string | null, control: Node): HTMLElement {
  return h(
    "div",
    { class: "field" },
    h(
      "div",
      { class: "field__label" },
      h("span", {}, label),
      hint ? h("span", { class: "field__hint" }, hint) : null,
    ),
    h("div", { class: "field__control" }, control),
  );
}

/** An on/off switch. */
export function toggle(checked: boolean, onChange: (value: boolean) => void): HTMLElement {
  const input = h("input", {
    type: "checkbox",
    checked,
    onChange: (event: Event) => onChange((event.target as HTMLInputElement).checked),
  });
  return h("label", { class: "switch" }, input, h("span", { class: "switch__track" }));
}

/** A select bound to a list of `[value, label]` pairs. */
export function select<T extends string>(
  options: readonly (readonly [T, string])[],
  value: T,
  onChange: (value: T) => void,
): HTMLElement {
  return h(
    "select",
    {
      onChange: (event: Event) => onChange((event.target as HTMLSelectElement).value as T),
    },
    ...options.map(([optionValue, label]) =>
      h("option", { value: optionValue, selected: optionValue === value }, label),
    ),
  );
}

/** A number input constrained to a range. */
export function numberInput(
  value: number,
  min: number,
  max: number,
  onChange: (value: number) => void,
): HTMLElement {
  return h("input", {
    type: "number",
    value: String(value),
    min: String(min),
    max: String(max),
    style: "min-width:96px",
    onChange: (event: Event) => {
      const input = event.target as HTMLInputElement;
      // Clamp here as well as in the backend, so the field shows the value that
      // was actually stored rather than the one the user typed.
      const clamped = Math.min(max, Math.max(min, Number(input.value) || min));
      input.value = String(clamped);
      onChange(clamped);
    },
  });
}

let toastTimer: number | undefined;

/** Shows a transient message at the bottom of the window. */
export function toast(message: string, kind: "info" | "error" = "info"): void {
  const element = document.getElementById("toast");
  if (!element) return;

  element.textContent = message;
  element.className = `toast visible${kind === "error" ? " error" : ""}`;

  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    element.className = "toast";
  }, kind === "error" ? 5200 : 2600);
}

/** Formats a byte count the way the history list shows it. */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

/** Formats a unix timestamp as a short local date and time. */
export function formatDate(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000);
  return date.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

const SVG_NS = "http://www.w3.org/2000/svg";

function svgEl(tag: string, attrs: Record<string, string>): SVGElement {
  const element = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs)) {
    element.setAttribute(key, value);
  }
  return element;
}

/**
 * The Kova mark: the same corner brackets and shutter dot as the app icon,
 * built as nodes rather than parsed from a string so this file never touches
 * `innerHTML`.
 */
export function brandMark(): SVGElement {
  const svg = svgEl("svg", {
    viewBox: "0 0 16 16",
    class: "sidebar__mark",
    "aria-hidden": "true",
  });

  const brackets = svgEl("g", {
    fill: "none",
    stroke: "currentColor",
    "stroke-width": "1.6",
    "stroke-linecap": "round",
  });
  for (const d of [
    "M2 5.2V3.6A1.6 1.6 0 0 1 3.6 2h1.6",
    "M10.8 2h1.6A1.6 1.6 0 0 1 14 3.6v1.6",
    "M14 10.8v1.6a1.6 1.6 0 0 1-1.6 1.6h-1.6",
    "M5.2 14H3.6A1.6 1.6 0 0 1 2 12.4v-1.6",
  ]) {
    brackets.append(svgEl("path", { d }));
  }

  svg.append(
    brackets,
    svgEl("circle", { cx: "8", cy: "8", r: "2.4", fill: "currentColor" }),
  );
  return svg;
}
