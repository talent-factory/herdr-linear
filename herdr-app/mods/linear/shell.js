// The layout node the toggle shows and hides, and the panel's size: what the toggle and the panel share.
import { layout, run } from "herdr";

export const PANEL = "linear-panel";
const DEFAULT_PX = 440;
const WIDE_WEIGHT = 3;

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) { const f = find(child, id); if (f) return f; }
  return null;
}
export const panelNode = () => find(layout(), PANEL);
export const panelShown = () => { const n = panelNode(); return Boolean(n) && !n.hidden; };

export function toggleWide() {
  const n = panelNode();
  if (!n) return;
  if (n.size?.weight != null) run("layout.resize", { id: PANEL, px: Number(localStorage.getItem("px")) || DEFAULT_PX });
  else {
    if (n.size?.px) localStorage.setItem("px", String(Math.round(n.size.px)));
    run("layout.resize", { id: PANEL, weight: WIDE_WEIGHT });
  }
}
