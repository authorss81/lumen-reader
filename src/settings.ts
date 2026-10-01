import { api, el } from "./api";
import type { ReaderSettings } from "./reader";

export interface AppSettings extends ReaderSettings {
  theme: "light" | "sepia" | "dark" | "black";
  /** Follow the Windows light/dark preference instead of a fixed theme. */
  followSystemTheme: boolean;
}

export const DEFAULT_SETTINGS: AppSettings = {
  theme: "light",
  followSystemTheme: false,
  fontSize: 19,
  lineHeight: 1.62,
  letterSpacing: 0,
  marginX: 72,
  marginY: 48,
  fontFamily: "serif",
  justify: false,
  scrollMode: false,
};

const THEMES: { id: AppSettings["theme"]; label: string; preview: [string, string] }[] = [
  { id: "light", label: "Light", preview: ["#fbfaf7", "#ffffff"] },
  { id: "sepia", label: "Sepia", preview: ["#f5ecd8", "#e8dfcc"] },
  { id: "dark", label: "Dark", preview: ["#1a1c20", "#2a2e34"] },
  { id: "black", label: "Black", preview: ["#000000", "#16181c"] },
];

export async function loadSettings(): Promise<AppSettings> {
  try {
    const stored = (await api.getSettings()) as Partial<AppSettings>;
    return { ...DEFAULT_SETTINGS, ...stored };
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

export function applyTheme(theme: string) {
  document.documentElement.dataset.theme = theme;
}

/** Resolve the effective theme, optionally following the Windows preference. */
export function effectiveTheme(settings: AppSettings): string {
  if (!settings.followSystemTheme) return settings.theme;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/** Keep following the OS when the user has asked us to. */
export function watchSystemTheme(settings: AppSettings, onChange: (theme: string) => void) {
  if (!settings.followSystemTheme || !window.matchMedia) return;
  const query = window.matchMedia("(prefers-color-scheme: dark)");
  const handler = () => onChange(effectiveTheme(settings));
  query.addEventListener("change", handler);
}

/**
 * Renders the settings drawer. Kept as a plain function so it can be opened
 * from both the library and the reader.
 */
export function openSettingsSheet(
  settings: AppSettings,
  onChange: (next: AppSettings) => void,
) {
  document.querySelector(".sheet")?.remove();
  const sheet = el("aside", "sheet");
  sheet.setAttribute("aria-label", "Reading settings");

  const head = el("div", "panel-head");
  const title = el("div", "topbar-book", "Reading settings");
  title.style.flex = "1";
  const close = el("button", "icon-btn");
  close.innerHTML =
    '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>';
  close.addEventListener("click", () => sheet.remove());
  head.append(title, close);

  const body = el("div", "sheet-body");
  sheet.append(head, body);

  const persist = () => {
    applyTheme(effectiveTheme(settings));
    for (const key of Object.keys(DEFAULT_SETTINGS) as (keyof AppSettings)[]) {
      void api.setSetting(key, settings[key]);
    }
    onChange({ ...settings });
  };

  // ---- layout: paged vs scrolling
  body.appendChild(
    field("Reading style", () =>
      segment(
        [
          { id: "paged", label: "Paginated" },
          { id: "scroll", label: "Continuous scroll" },
        ],
        settings.scrollMode ? "scroll" : "paged",
        (id) => {
          settings.scrollMode = id === "scroll";
          persist();
        },
        true,
      ),
    ),
  );

  body.appendChild(
    field("Theme source", () =>
      segment(
        [
          { id: "manual", label: "Pick a theme" },
          { id: "system", label: "Match Windows" },
        ],
        settings.followSystemTheme ? "system" : "manual",
        (id) => {
          settings.followSystemTheme = id === "system";
          persist();
          sheet.remove();
          openSettingsSheet(settings, onChange);
        },
        true,
      ),
    ),
  );

  // ---- theme
  body.appendChild(
    field("Page theme", () => {
      const grid = el("div", "seg");
      for (const theme of THEMES) {
        const btn = el("button", "theme-swatch");
        btn.type = "button";
        if (settings.theme === theme.id) btn.classList.add("active");
        btn.style.background = theme.preview[0];
        btn.style.color = theme.preview[0] === "#ffffff" ? "#1b1a17" : "#e7e9ec";
        const label = el("span", undefined, theme.label);
        btn.appendChild(label);
        btn.addEventListener("click", () => {
          settings.theme = theme.id;
          grid.querySelectorAll(".theme-swatch").forEach((n) => n.classList.remove("active"));
          btn.classList.add("active");
          persist();
        });
        grid.appendChild(btn);
      }
      return grid;
    }),
  );

  // ---- font family
  body.appendChild(
    field("Typeface", () =>
      segment(
        [
          { id: "serif", label: "Serif" },
          { id: "sans", label: "Sans" },
          { id: "mono", label: "Mono" },
        ],
        settings.fontFamily,
        (id) => {
          settings.fontFamily = id as AppSettings["fontFamily"];
          persist();
        },
        true,
      ),
    ),
  );

  // ---- font size
  body.appendChild(
    field("Font size", `${settings.fontSize}px`, () =>
      slider(12, 34, 1, settings.fontSize, (v) => {
        settings.fontSize = v;
        persist();
      }, (v) => `${v}px`),
    ),
  );

  // ---- line height
  body.appendChild(
    field("Line height", settings.lineHeight.toFixed(2), () =>
      slider(1.2, 2.4, 0.02, settings.lineHeight, (v) => {
        settings.lineHeight = v;
        persist();
      }, (v) => v.toFixed(2)),
    ),
  );

  // ---- letter spacing
  body.appendChild(
    field("Letter spacing", `${settings.letterSpacing.toFixed(2)}em`, () =>
      slider(-0.02, 0.15, 0.01, settings.letterSpacing, (v) => {
        settings.letterSpacing = v;
        persist();
      }, (v) => `${v.toFixed(2)}em`),
    ),
  );

  // ---- margins
  body.appendChild(
    field("Side margins", `${settings.marginX}px`, () =>
      slider(0, 220, 4, settings.marginX, (v) => {
        settings.marginX = v;
        persist();
      }),
    ),
  );
  body.appendChild(
    field("Top & bottom margins", `${settings.marginY}px`, () =>
      slider(0, 160, 4, settings.marginY, (v) => {
        settings.marginY = v;
        persist();
      }),
    ),
  );

  // ---- justify
  body.appendChild(
    field("Alignment", () =>
      segment(
        [
          { id: "left", label: "Ragged right" },
          { id: "justify", label: "Justified" },
        ],
        settings.justify ? "justify" : "left",
        (id) => {
          settings.justify = id === "justify";
          persist();
        },
        true,
      ),
    ),
  );

const reset = el("button", "btn", "Reset to defaults");
  reset.type = "button";
  reset.style.width = "100%";
  reset.style.marginTop = "6px";
  reset.addEventListener("click", () => {
    Object.assign(settings, DEFAULT_SETTINGS);
    sheet.remove();
    document.querySelector(".scrim")?.remove();
    applyTheme(effectiveTheme(settings));
    for (const key of Object.keys(DEFAULT_SETTINGS) as (keyof AppSettings)[]) {
      void api.setSetting(key, settings[key]);
    }
    onChange({ ...settings });
  });
  body.appendChild(reset);

  const scrim = el("div", "scrim");
  scrim.addEventListener("click", () => {
    sheet.remove();
    scrim.remove();
  });
  document.body.append(scrim, sheet);
  (sheet.querySelector("button") as HTMLButtonElement)?.focus();
}

function field(
  label: string,
  valueOrBuilder: string | (() => HTMLElement),
  builder?: () => HTMLElement,
) {
  const wrap = el("div", "field");
  const head = el("div", "field-label");
  head.appendChild(el("span", undefined, label));
  const control = builder ?? (typeof valueOrBuilder === "function" ? valueOrBuilder : null);
  const readout = typeof valueOrBuilder === "string" ? valueOrBuilder : null;
  if (readout) head.appendChild(el("span", "field-value", readout));
  wrap.appendChild(head);
  if (control) wrap.appendChild(control());
  return wrap;
}

function segment(
  options: { id: string; label: string }[],
  active: string,
  onPick: (id: string) => void,
  twoColumns = false,
) {
  const grid = el("div", `seg${twoColumns ? " seg-cols-2" : ""}`);
  for (const option of options) {
    const btn = el("button", "seg-btn", option.label);
    btn.type = "button";
    if (option.id === active) btn.classList.add("active");
    btn.addEventListener("click", () => {
      grid.querySelectorAll(".seg-btn").forEach((n) => n.classList.remove("active"));
      btn.classList.add("active");
      onPick(option.id);
    });
    grid.appendChild(btn);
  }
  return grid;
}

function slider(
  min: number,
  max: number,
  step: number,
  value: number,
  onInput: (value: number) => void,
  format: (value: number) => string = (v) => `${v}px`,
) {
  const wrap = el("div");
  const input = document.createElement("input");
  input.type = "range";
  input.className = "slider";
  input.min = String(min);
  input.max = String(max);
  input.step = String(step);
  input.value = String(value);
  const readout = el("div", "field-value", format(value));
  readout.style.textAlign = "right";
  input.addEventListener("input", () => {
    const next = Number(input.value);
    readout.textContent = format(next);
    onInput(next);
  });
  wrap.append(input, readout);
  return wrap;
}