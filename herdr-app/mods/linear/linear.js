// Linear through the `herdr-linear-gui` Rust bridge (built from REPO into ~/.local/bin),
// plus finding Linear issue references in terminal text. Every call runs on this computer:
// Linear is the same from every machine, and the credentials live here.
import { exec, home } from "herdr";

export const REPO = "https://github.com/far-fetched-labs/herdr-linear";
export const INSTALL = `cargo install --force --git ${REPO} --features gui --bin herdr-linear-gui --root ~/.local`;
export const bin = () => `${home()}/.local/bin/herdr-linear-gui`;
const LOGIN_MS = 310000; // the bridge waits 300 s for the browser

const lastLine = (text) => String(text ?? "").split("\n").map((l) => l.trim()).filter(Boolean).pop() ?? "";

// kind: "missing" (bridge not installed), "auth" (connect or reconnect a workspace), "error".
export class BridgeError extends Error {
  constructor(message, kind) { super(message); this.kind = kind; }
}

async function call(args, timeout = 30000) {
  let r;
  try { r = await exec(bin(), args, { timeout }); }
  catch (e) {
    const msg = String(e?.message ?? e);
    throw new BridgeError(msg, /time/i.test(msg) ? "error" : "missing");
  }
  if (r.code === 2) throw new BridgeError(lastLine(r.stderr) || "Connect a Linear workspace.", "auth");
  if (r.code !== 0) throw new BridgeError(lastLine(r.stderr) || lastLine(r.stdout) || `herdr-linear-gui exited ${r.code}`, "error");
  return JSON.parse(r.stdout || "null");
}

const ws = (w) => (w ? ["-w", w] : []);

// [{ urlKey, name, viewer, teams: [{ key, name }], auth, error?, needsLogin? }]
export const workspaces = () => call(["workspaces"]);
export const list = (which, w) => call(["list", which, ...ws(w)]); // rows, each with `workspace`
export const search = (text, w) => call(["search", text, ...ws(w)]);
// `ref`: identifier, issue URL (routes to its workspace) or UUID; `w` breaks team-key ties.
export const issue = (ref, w) => call(["issue", ref, ...ws(w)]);
export const lookup = (refs, w) => (refs.length ? call(["lookup", ...refs, ...ws(w)]) : Promise.resolve([]));
// Opens Linear's consent screen in the browser and resolves once it's approved: the workspace.
export const login = () => call(["login"], LOGIN_MS);

const q = (v) => `'${String(v).replace(/'/g, "'\\''")}'`;
const hold = (cmd) => ["sh", "-c", `${cmd}; printf '\\n[exit %s: press Enter to close] ' "$?"; read _`];
export const keyLoginArgv = () => hold(`${q(bin())} login --key`);
export const installArgv = () => hold(INSTALL);

// --- references in text -------------------------------------------------------------------------

const URL_RE = /linear\.app\/[\w.-]+\/issue\/([A-Za-z][A-Za-z0-9]*-\d+)/g;

// Bare identifiers only for connected workspaces' team keys (so UTF-8 or ISO-9001 never match);
// any case, since branch names carry them lowercased (jesse/con-12-fix-login).
function keyRe(teams) {
  const keys = (teams ?? []).filter((k) => /^[A-Za-z][A-Za-z0-9]*$/.test(k));
  return keys.length ? new RegExp(`(^|[^A-Za-z0-9])(${keys.join("|")})-(\\d+)(?![A-Za-z0-9])`, "gi") : null;
}

// Every distinct issue identifier in `text`, last mention first (the bottom of a terminal is
// what happened most recently). Linear URLs count for any team.
export function findRefs(text, teams) {
  const hits = []; // [index, "CON-12"]
  for (const m of String(text).matchAll(URL_RE)) hits.push([m.index, m[1].toUpperCase()]);
  const re = keyRe(teams);
  if (re) for (const m of String(text).matchAll(re)) {
    if (Number(m[3]) > 0) hits.push([m.index + m[1].length, `${m[2].toUpperCase()}-${Number(m[3])}`]);
  }
  hits.sort((a, b) => b[0] - a[0]);
  return [...new Set(hits.map((h) => h[1]))];
}

// What the search box was given: an identifier or an issue URL (kept whole, so the bridge sends
// it to the URL's workspace), else null for a text search.
export function asRef(text) {
  const s = String(text).trim();
  if (/^[A-Za-z][A-Za-z0-9]*-\d+$/.test(s)) return { id: s.toUpperCase(), q: s.toUpperCase() };
  const m = /linear\.app\/[\w.-]+\/issue\/([A-Za-z][A-Za-z0-9]*-\d+)/.exec(s);
  return m ? { id: m[1].toUpperCase(), q: s } : null;
}
