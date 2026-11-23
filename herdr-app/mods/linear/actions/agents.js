// Handing work to the agents: who is on this issue, and what to send them. Click an agent to focus
// it. Send delivers to a running agent, a sub-agent of one (`<name> (spawn)`) or a new agent.
// Adapted from Branch Issue (https://github.com/munza/herdr-mods), Copyright 2026 Tawsif Aqib, Apache-2.0.
import { h_flex } from "gpui-base";
import { agents, hc, knownAgents, panes, run } from "herdr";
import { agentsOnIssue, handoffMessage } from "../model.js";
import { SmallButton, MenuButton } from "../views/ui.js";

const NEW_AGENT = "new:";
const SPAWN = "spawn:";

// An unnamed tab is called by its place ("1", "2"), which says nothing about who is in it: name
// the program instead, with the space and place to tell two of them apart.
export const agentName = (agent) => {
  const tab = agent?.tab_name;
  if (tab && !/^\d+$/.test(tab)) return tab;
  const who = agent?.program?.name || agent?.app || agent?.title || agent?.address || "agent";
  const where = [agent?.space_name, tab && `tab ${tab}`].filter(Boolean).join(" ");
  return where ? `${who} · ${where}` : who;
};

export const agentMethods = {
  issueAgents(issue) {
    if (!issue) return [];
    return agentsOnIssue({ agents: agents(), panes: panes(), issue, worktrees: this.currentWorktrees(issue), space: this.at?.space });
  },

  // Those on the issue first, then the rest of this space, who may well be free.
  agentChoices(issue) {
    const on = this.issueAgents(issue);
    const others = agents().filter((a) => a.space === this.at?.space && !on.some((b) => b.address === a.address));
    return [...on, ...others];
  },

  focusAgent(agent) { run("agent.focus", { terminal: agent.address }); },

  // Installed agents on this space's machine, for starting a new one. Herdr keeps the list cached.
  loadNewAgents(cx) {
    const machine = this.at?.machine ?? null;
    cx.spawn(async (cx) => {
      try {
        const known = await knownAgents({ on: machine });
        if ((this.at?.machine ?? null) === machine) this.newAgents = known.filter((a) => a.installed);
      } catch { this.newAgents = []; }
      cx.notify();
    });
  },

  newAgentChoices() {
    return (this.newAgents ?? []).map((a) => ({ address: `${NEW_AGENT}${a.key}`, isNew: true, run: a.run, tab_name: `new ${a.name}` }));
  },

  // A sub-agent has to be the parent's own kind, so an agent we can't match has none.
  spawnRun(parent) { return (this.newAgents ?? []).find((a) => a.key === parent.program?.key)?.run ?? null; },

  // Who a choice means: someone running, a sub-agent of one, or one to start.
  agentAt(address) {
    if (address?.startsWith(SPAWN)) {
      const parent = agents().find((a) => a.address === address.slice(SPAWN.length));
      const command = parent && this.spawnRun(parent);
      return command ? {
        ...parent, address, parentAddress: parent.address, spawn: true, run: command,
        parentName: agentName(parent), tab_name: `${agentName(parent)} (spawn)`,
      } : null;
    }
    return agents().find((a) => a.address === address) ?? this.newAgentChoices().find((a) => a.address === address) ?? null;
  },

  // Work goes where the checkout is: the one this space is in, else the first there is.
  workFolder(issue) {
    const worktrees = this.currentWorktrees(issue);
    const here = worktrees.find((w) => w.path === this.at?.cwd);
    return (here ?? worktrees[0])?.path ?? this.at?.cwd;
  },

  // What `hc` is asked to do. A sub-agent is started here, as an argv element: no shell sees the
  // message, which can hold other people's text, and a busy parent is no wait.
  hcArgs(agent, message, issue) {
    if (!agent.spawn) return ["send", agent.address, message];
    const machine = this.at?.machine, cwd = this.workFolder(issue);
    return [
      "run", "--bg", "--json", "--parent", agent.parentAddress,
      ...(machine ? ["--on", machine] : []), ...(cwd ? ["--cwd", cwd] : []),
      ...agent.run,
      message.startsWith("-") ? ` ${message}` : message, // never read as a flag by the agent's command line
    ];
  },

  // A new agent gets its own tab, in the work's folder, with the message as its first prompt.
  startAgent(agent, message, issue, cx) {
    run("tab.new", { space: this.at?.space, folder: this.workFolder(issue), argv: agent.run, task: message });
    this.say(`Started ${agentName(agent)} in a new tab.`, cx);
  },

  // A dropdown of who can take this: agents on the issue first, then the rest of this space, then a
  // new one of each installed kind. `send(agent, cx)` does the work. With no agent: says so, sends nothing.
  agentButton({ id, label, icon, issue, send }) {
    const running = this.agentChoices(issue), fresh = this.newAgentChoices();
    if (this.mutating || !(running.length + fresh.length))
      return SmallButton({ id, icon, label: this.mutating ? label : `${label} (no agent)`, disabled: true, onClick: () => {} });
    const chooser = this.chooser({
      id, accessibilityLabel: `${label}: choose an agent`, width: 260,
      trigger: MenuButton({ id: `${id}-trigger`, label }),
      choices: [
        ...running.flatMap((a) => [
          { id: a.address, label: agentName(a), detail: a.status },
          ...(this.spawnRun(a) ? [{ id: `${SPAWN}${a.address}`, label: `${agentName(a)} (spawn)` }] : []),
        ]),
        ...fresh.map((a, i) => ({ id: a.address, label: `Start ${agentName(a)}`, divided: i === 0 && running.length > 0 })),
      ],
      onChoose: (address, cx) => send(this.agentAt(address), cx),
    });
    // A click on the chooser shouldn't also count as a click on the row around it.
    return h_flex().id(`${id}-group`).flex_none().items_center().on_click((_e, cx) => cx.stop_propagation()).child(chooser);
  },

  // Only Send delivers. `hc` reaches a busy agent's session too.
  sendAgent(cx) {
    const agent = this.agentAt(this.target), issue = this.currentIssue();
    if (!agent || !issue || this.mutating) return;
    const draft = this.composerKey, composer = this.composer;
    const message = handoffMessage(issue, composer, this.editor.value());
    const done = () => {
      if (draft) localStorage.removeItem(draft);
      if (this.composer === composer) { this.composer = null; this.composerKey = null; }
    };
    if (agent.isNew) { this.startAgent(agent, message, issue, cx); done(); cx.notify(); return; }
    this.mutating = true; this.error = null; cx.notify();
    cx.spawn(async (cx) => {
      try {
        const result = await hc(this.hcArgs(agent, message, issue));
        if (result.code !== 0) throw new Error(result.stderr || "Agent delivery failed");
        this.say(agent.spawn ? `Started a sub-agent under ${agent.parentName}.` : `Sent to ${agentName(agent)}.`, cx);
        done();
      } catch (e) { this.error = e.message; }
      this.mutating = false;
      cx.notify();
    });
  },
};
