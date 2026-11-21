// A mod's preferences from the `settings` key of its installed mod.json, merged over the defaults
// its code ships with. Herdr reloads the mod when that file is saved, so an edit applies at once.
// Unknown keys and values of the wrong type or range are reported, never fatal, and ignored.
// Adapted from Branch Issue (https://github.com/munza/herdr-mods), Copyright 2026 Tawsif Aqib, Apache-2.0.
import { SCOPES, STATES } from "./model.js";

export const DEFAULT_SETTINGS = {
  refresh: { every: "60s", search_debounce: "450ms" },
  branch: { max_identifiers: 4 },
  list: { default: { scope: "mine", state: "active", team: "" } },
  // root "": Herdr's own worktree directory (settings.json), else ~/.herdr/worktrees.
  worktrees: { root: "", open_with: "terminal", move_to_started: false },
};

const UNITS = { ms: 1, s: 1000, m: 60000, h: 3600000 };
// "450ms", "60s", "5m", or a plain number of milliseconds. Zero falls back, since these pace timers.
export function duration(value, fallback) {
  if (typeof value === "number") return value > 0 ? value : fallback;
  const match = /^\s*(\d+(?:\.\d+)?)\s*(ms|s|m|h)\s*$/.exec(String(value ?? ""));
  return (match && Math.round(Number(match[1]) * UNITS[match[2]])) || fallback;
}

const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

// Deep-merge `given` over `defaults`. The defaults define the keys and their types; a duration
// default is a string, and a number is accepted for it. Anything else is skipped and named in
// `problems`.
export function mergeSettings(defaults, given, problems = [], path = "") {
  const result = { ...defaults };
  if (!isObject(given)) {
    if (given !== undefined) problems.push(`${path || "settings"} must be an object`);
    return result;
  }
  for (const [key, value] of Object.entries(given)) {
    const name = path ? `${path}.${key}` : key;
    if (!(key in defaults)) { problems.push(`${name} is not a setting`); continue; }
    const current = defaults[key];
    if (isObject(current)) result[key] = mergeSettings(current, value, problems, name);
    else if (typeof value === typeof current || (typeof current === "string" && typeof value === "number")) result[key] = value;
    else problems.push(`${name} should be a ${typeof current}`);
  }
  return result;
}

// Values of the right type that still make no sense go back to their default.
function checkRanges(settings, problems) {
  const out = JSON.parse(JSON.stringify(settings));
  const D = DEFAULT_SETTINGS;
  for (const key of ["every", "search_debounce"]) {
    if (duration(out.refresh[key], 0)) continue;
    problems.push(`refresh.${key} should be a duration such as 60s`);
    out.refresh[key] = D.refresh[key];
  }
  const max = out.branch.max_identifiers;
  if (!Number.isInteger(max) || max < 1) {
    problems.push("branch.max_identifiers should be a whole number, at least 1");
    out.branch.max_identifiers = D.branch.max_identifiers;
  }
  for (const [key, choices] of [["scope", SCOPES], ["state", STATES]]) {
    if (choices.some((c) => c.id === out.list.default[key])) continue;
    problems.push(`list.default.${key} should be one of ${choices.map((c) => c.id).join(", ")}`);
    out.list.default[key] = D.list.default[key];
  }
  return out;
}

// The merged, checked settings for `given` (mod.json's `settings`) plus problems to show the person.
export function resolveSettings(given, defaults = DEFAULT_SETTINGS) {
  const problems = [];
  const merged = mergeSettings(defaults, given, problems);
  return { settings: checkRanges(merged, problems), problems };
}

// Reads `<path>/mod.json` with `readText` (Herdr's readTextFile, injected so this stays testable).
export async function loadSettings(readText, path, defaults = DEFAULT_SETTINGS) {
  let given = {};
  const pre = [];
  try {
    given = JSON.parse((await readText(`${path}/mod.json`)) ?? "{}").settings ?? {};
  } catch (e) {
    pre.push(`mod.json could not be read: ${e.message}`);
  }
  const { settings, problems } = resolveSettings(given, defaults);
  return { settings, problems: [...pre, ...problems] };
}

// Where new worktrees go: the mod's setting, else Herdr's own, else ~/.herdr/worktrees.
export const worktreeRoot = (settings, herdrSettings) =>
  settings.worktrees.root || herdrSettings?.worktrees?.directory || "~/.herdr/worktrees";
