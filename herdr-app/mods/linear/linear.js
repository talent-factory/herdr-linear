// Linear through the `herdr-linear-gui` Rust bridge (built from REPO into ~/.local/bin). Every call
// runs on this computer: Linear is the same from every machine, and the credentials live here.
// Arguments are argv elements, never a shell line, so text from anywhere can't be read as a command.
// Finding references in text lives in model.js.
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

// [{ urlKey, name, viewer: { id, displayName, email }, teams: [{ id, key, name }], auth, error?, needsLogin? }]
export const workspaces = () => call(["workspaces"]);
export const list = (which, w) => call(["list", which, ...ws(w)]); // rows, each with `workspace`
export const search = (text, w) => call(["search", text, ...ws(w)]);
// `ref`: identifier, issue URL (routes to its workspace) or UUID; `w` breaks team-key ties.
export const issue = (ref, w) => call(["issue", ref, ...ws(w)]);
export const lookup = (refs, w) => (refs.length ? call(["lookup", ...refs, ...ws(w)]) : Promise.resolve([]));
// One page of an "All issues" list: { items: [row], cursor | null }. `scope`: mine|cycle|all;
// `state`: active|backlog|started|done|cancelled|all; `team`: a team key; `after`: a cursor.
export function issues(w, { scope, state, team, search, after } = {}) {
  const opt = (flag, v) => (v ? [flag, String(v)] : []);
  return call(["issues", ...ws(w), ...opt("--scope", scope), ...opt("--state", state), ...opt("--team", team), ...opt("--search", search), ...opt("--after", after)]);
}

// Writes: never retried (a repeat could post twice), so a failure is the caller's to show.
// `issueId` is Linear's issue UUID (the detail's `id`).
export const comment = (issueId, w, body, parent) =>
  call(["comment", issueId, ...ws(w), "--body", body, ...(parent ? ["--parent", parent] : [])]); // { success, comment: { id, url } }
// fields: { stateId, assignee (user id), unassign: true, priority: 0-4 } -> { success, issue }
export function update(issueId, w, { stateId, assignee, unassign, priority } = {}) {
  const args = ["update", issueId, ...ws(w)];
  if (stateId) args.push("--state", stateId);
  if (assignee) args.push("--assignee", assignee);
  if (unassign) args.push("--unassign");
  if (priority != null) args.push("--priority", String(priority));
  return call(args);
}

// Opens Linear's consent screen in the browser and resolves once it's approved: the workspace.
export const login = () => call(["login"], LOGIN_MS);

const q = (v) => `'${String(v).replace(/'/g, "'\\''")}'`;
const hold = (cmd) => ["sh", "-c", `${cmd}; printf '\\n[exit %s: press Enter to close] ' "$?"; read _`];
export const keyLoginArgv = () => hold(`${q(bin())} login --key`);
export const installArgv = () => hold(INSTALL);
