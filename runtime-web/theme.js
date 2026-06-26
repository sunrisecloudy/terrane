(function () {
  "use strict";

  // Persisted theme preference. Bumped only on an incompatible shape change.
  const STORAGE_KEY = "terrane.theme.v1";

  // Canonical themeable design tokens. The names mirror the CSS custom
  // properties every bundled example app (and every well-formed generated app)
  // consumes, so overriding them on an app's document root re-skins the app
  // without touching its package. `editable` tokens are surfaced as colour
  // controls in the Engine Room; the remaining tokens ride along through
  // presets so palette-rich apps (calendar, task board) stay coherent.
  const TOKENS = [
    { name: "accent", label: "Accent", hint: "Primary buttons and highlights", editable: true },
    { name: "bg", label: "Background", hint: "App page background", editable: true },
    { name: "panel", label: "Surface", hint: "Cards, panels, inputs", editable: true },
    { name: "text", label: "Text", hint: "Primary text colour", editable: true },
    { name: "muted", label: "Muted text", hint: "Secondary text colour", editable: true },
    { name: "border", label: "Border", hint: "Dividers and outlines", editable: true },
    { name: "danger", label: "Danger", hint: "Destructive actions", editable: true },
  ];

  // Extra tokens used by a subset of apps; carried by presets, not directly
  // edited, so a chosen palette still themes warnings, success, and shadows.
  const EXTRA_TOKEN_NAMES = ["accent-strong", "warn", "good", "rose", "soft", "shadow"];
  const ALL_TOKEN_NAMES = TOKENS.map(function (token) { return token.name; }).concat(EXTRA_TOKEN_NAMES);
  const ALL_TOKEN_SET = new Set(ALL_TOKEN_NAMES);

  const SYSTEM_PRESET_ID = "system";
  const CUSTOM_PRESET_ID = "custom";

  // "system" carries no overrides, so apps fall back to their own `:root`
  // tokens and `prefers-color-scheme` rules. Every other preset is a complete
  // palette so it themes every token an app might read.
  const PRESETS = [
    { id: SYSTEM_PRESET_ID, name: "System default", description: "Use each app's own light/dark palette.", tokens: {} },
    {
      id: "midnight", name: "Midnight", description: "Calm slate dark mode.",
      tokens: { bg: "#0c111d", panel: "#151b2c", text: "#eef2ff", muted: "#9aa4b2", border: "#293247", accent: "#8aa2ff", "accent-strong": "#6f8bff", danger: "#ff8a80", warn: "#f5c451", good: "#5ad19a", rose: "#ff8a80", soft: "#1b2236", shadow: "none" },
    },
    {
      id: "daylight", name: "Daylight", description: "Bright neutral light mode.",
      tokens: { bg: "#f6f7fb", panel: "#ffffff", text: "#121826", muted: "#667085", border: "#dde2eb", accent: "#315efb", "accent-strong": "#1f44d6", danger: "#b42318", warn: "#b25e09", good: "#147d56", rose: "#b42318", soft: "#eef2ff", shadow: "0 16px 50px rgba(16, 24, 40, 0.08)" },
    },
    {
      id: "forest", name: "Forest", description: "Green, grounded, calm.",
      tokens: { bg: "#f3f8f4", panel: "#ffffff", text: "#10261c", muted: "#5b7065", border: "#cfe3d6", accent: "#0f9d58", "accent-strong": "#0b7a44", danger: "#b3261e", warn: "#b25e09", good: "#0f9d58", rose: "#b3261e", soft: "#e3f3e9", shadow: "0 16px 40px rgba(12, 40, 28, 0.10)" },
    },
    {
      id: "sunset", name: "Sunset", description: "Warm amber and rose.",
      tokens: { bg: "#fff6f0", panel: "#ffffff", text: "#2a160d", muted: "#8a6f63", border: "#f3d9c8", accent: "#ef6c2e", "accent-strong": "#cf501a", danger: "#c0341d", warn: "#cf7a12", good: "#1f9d6b", rose: "#e0457a", soft: "#ffe9dc", shadow: "0 16px 44px rgba(70, 30, 10, 0.12)" },
    },
    {
      id: "grape", name: "Grape", description: "Violet with cool neutrals.",
      tokens: { bg: "#f7f4fd", panel: "#ffffff", text: "#1d1330", muted: "#6b6080", border: "#e2d8f3", accent: "#7c3aed", "accent-strong": "#6024c9", danger: "#b3261e", warn: "#a85b09", good: "#0f9d58", rose: "#d6336c", soft: "#efe7fd", shadow: "0 16px 44px rgba(40, 20, 70, 0.12)" },
    },
    {
      id: "mono", name: "Mono", description: "Quiet grayscale, no hue.",
      tokens: { bg: "#f5f5f5", panel: "#ffffff", text: "#1a1a1a", muted: "#6b6b6b", border: "#dcdcdc", accent: "#2b2b2b", "accent-strong": "#000000", danger: "#9b1c1c", warn: "#7a5b00", good: "#1f6b3a", rose: "#9b1c1c", soft: "#ececec", shadow: "0 14px 36px rgba(0, 0, 0, 0.10)" },
    },
    {
      id: "contrast", name: "High contrast", description: "Maximum legibility, dark.",
      tokens: { bg: "#000000", panel: "#0a0a0a", text: "#ffffff", muted: "#d6d6d6", border: "#ffffff", accent: "#ffd400", "accent-strong": "#ffe34d", danger: "#ff5252", warn: "#ffd400", good: "#4dff88", rose: "#ff5252", soft: "#1a1a1a", shadow: "none" },
    },
  ];
  const PRESET_BY_ID = new Map(PRESETS.map(function (preset) { return [preset.id, preset]; }));

  const listeners = new Set();
  let current = load();

  // Defence in depth: theme values flow into `style.setProperty` inside the
  // sandboxed app. The CSSOM already rejects malformed values, but we keep the
  // accepted surface to plain colour/length syntax so a value can never carry a
  // `url(...)` fetch, smuggle extra declarations, or close the rule.
  function isSafeValue(value) {
    if (typeof value !== "string") return false;
    const trimmed = value.trim();
    if (!trimmed || trimmed.length > 80) return false;
    if (/[;{}<>@\\]/.test(trimmed)) return false;
    if (/url\(|expression|image-set|var\(|javascript:|@import/i.test(trimmed)) return false;
    return /^[#0-9a-z.,()%/\s_-]+$/i.test(trimmed);
  }

  function sanitizeTokens(rawTokens) {
    const tokens = {};
    if (!rawTokens || typeof rawTokens !== "object" || Array.isArray(rawTokens)) return tokens;
    for (const name of ALL_TOKEN_NAMES) {
      const value = rawTokens[name];
      if (typeof value === "string" && isSafeValue(value)) {
        tokens[name] = value.trim();
      }
    }
    return tokens;
  }

  function normalize(theme) {
    const tokens = sanitizeTokens(theme && theme.tokens);
    let presetId = theme && typeof theme.presetId === "string" ? theme.presetId : SYSTEM_PRESET_ID;
    const preset = PRESET_BY_ID.get(presetId);
    if (preset && presetId !== CUSTOM_PRESET_ID && !tokensMatchPreset(tokens, preset)) {
      // Stored tokens drifted from the named preset: treat them as custom.
      presetId = Object.keys(tokens).length ? CUSTOM_PRESET_ID : SYSTEM_PRESET_ID;
    }
    if (!preset && presetId !== CUSTOM_PRESET_ID) {
      presetId = Object.keys(tokens).length ? CUSTOM_PRESET_ID : SYSTEM_PRESET_ID;
    }
    return { presetId: presetId, tokens: tokens };
  }

  function tokensMatchPreset(tokens, preset) {
    const presetKeys = Object.keys(preset.tokens);
    const tokenKeys = Object.keys(tokens);
    if (presetKeys.length !== tokenKeys.length) return false;
    return presetKeys.every(function (key) { return tokens[key] === preset.tokens[key]; });
  }

  function load() {
    let raw = null;
    try {
      raw = window.localStorage ? window.localStorage.getItem(STORAGE_KEY) : null;
    } catch (_) {
      raw = null;
    }
    if (!raw) return { presetId: SYSTEM_PRESET_ID, tokens: {} };
    try {
      return normalize(JSON.parse(raw));
    } catch (_) {
      return { presetId: SYSTEM_PRESET_ID, tokens: {} };
    }
  }

  function persist() {
    try {
      if (!window.localStorage) return;
      if (current.presetId === SYSTEM_PRESET_ID && !Object.keys(current.tokens).length) {
        window.localStorage.removeItem(STORAGE_KEY);
        return;
      }
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
    } catch (_) {
      // Persistence is best-effort in embedded/test runtimes.
    }
  }

  function notify() {
    const snapshot = get();
    for (const listener of Array.from(listeners)) {
      try {
        listener(snapshot);
      } catch (_) {
        // A failing listener must not abort theme propagation.
      }
    }
  }

  function cloneTokens(tokens) {
    const copy = {};
    for (const key of Object.keys(tokens)) copy[key] = tokens[key];
    return copy;
  }

  function get() {
    return { presetId: current.presetId, tokens: cloneTokens(current.tokens) };
  }

  // Flat `name -> value` map to apply on a document root. Defaults to the
  // current theme; pass an explicit theme to resolve a candidate.
  function resolveTokens(theme) {
    const source = theme ? normalize(theme) : current;
    return cloneTokens(source.tokens);
  }

  function set(next) {
    current = normalize(next);
    persist();
    notify();
    return get();
  }

  function selectPreset(presetId) {
    const preset = PRESET_BY_ID.get(presetId);
    if (!preset) return get();
    return set({ presetId: preset.id, tokens: cloneTokens(preset.tokens) });
  }

  // Override a single editable token, promoting the theme to "custom" while
  // keeping the rest of the active palette intact.
  function setToken(name, value) {
    if (!ALL_TOKEN_SET.has(name) || !isSafeValue(value)) return get();
    const tokens = cloneTokens(current.tokens);
    tokens[name] = value.trim();
    return set({ presetId: CUSTOM_PRESET_ID, tokens: tokens });
  }

  function reset() {
    return selectPreset(SYSTEM_PRESET_ID);
  }

  function subscribe(listener) {
    if (typeof listener !== "function") return function () {};
    listeners.add(listener);
    return function () { listeners.delete(listener); };
  }

  function presets() {
    return PRESETS.map(function (preset) {
      return { id: preset.id, name: preset.name, description: preset.description, tokens: cloneTokens(preset.tokens) };
    });
  }

  function presetById(presetId) {
    const preset = PRESET_BY_ID.get(presetId);
    return preset ? { id: preset.id, name: preset.name, description: preset.description, tokens: cloneTokens(preset.tokens) } : null;
  }

  function tokens() {
    return TOKENS.map(function (token) {
      return { name: token.name, label: token.label, hint: token.hint, editable: token.editable };
    });
  }

  window.TerraneTheme = {
    SYSTEM_PRESET_ID: SYSTEM_PRESET_ID,
    CUSTOM_PRESET_ID: CUSTOM_PRESET_ID,
    get: get,
    set: set,
    selectPreset: selectPreset,
    setToken: setToken,
    reset: reset,
    resolveTokens: resolveTokens,
    subscribe: subscribe,
    presets: presets,
    presetById: presetById,
    tokens: tokens,
    isSafeValue: isSafeValue,
  };
})();
