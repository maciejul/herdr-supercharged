'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const herdr = require('../lib/herdr');
const pstree = require('../lib/pstree');
const agentSessions = require('../lib/agent-sessions');
const { build } = require('../lib/snapshot');
const { restore } = require('../lib/restore');

test('suspended snapshot and restore preserve native ownership', () => {
  const session = { source: 'herdr:codex', agent: 'codex', kind: 'id', value: 'exact-session' };
  const ws = { workspace_id: 'w1', number: 1, label: 'Custom space', active_tab_id: 'w1:t1' };
  const tab = { workspace_id: 'w1', tab_id: 'w1:t1', number: 1, label: 'Custom tab (Suspended)' };
  const pane = { workspace_id: 'w1', tab_id: 'w1:t1', pane_id: 'w1:p1', agent: 'codex', agent_session: session, cwd: '/tmp' };
  const live = { workspaces: [ws], tabs: [tab], panes: [pane], focused_workspace_id: 'w1' };
  const originals = { ...herdr };
  const oldQuery = pstree.query;
  const oldResolver = agentSessions.makeResolver;
  const mutations = [];
  try {
    herdr.snapshot = () => live;
    herdr.processInfo = () => ({ pane_id: pane.pane_id, foreground_processes: [] });
    herdr.tabList = () => live.tabs;
    herdr.paneList = () => live.panes;
    for (const key of ['runInPane', 'focusWorkspace', 'splitPane', 'createWorkspace'])
      herdr[key] = (...args) => { mutations.push([key, ...args]); throw new Error(`Unexpected ${key}`); };
    pstree.query = () => null;
    agentSessions.makeResolver = () => () => { throw new Error('Must use exact native session'); };
    const model = build();
    assert.equal(model.workspaces[0].tabs[0].panes[0].suspended, true);
    assert.deepEqual(model.workspaces[0].tabs[0].panes[0].agent.session, session);
    delete pane.agent;
    assert.deepEqual(build().workspaces[0].tabs[0].panes[0].agent.session, session);
    for (const dryRun of [true, false]) {
      const result = restore(model, { dryRun });
      assert.equal(result.actions.some(a => a.action === 'run'), false);
      assert.equal(result.actions.some(a => a.action === 'skip'), true);
    }
    assert.deepEqual(mutations, []);
    // A pre-suspension snapshot must also respect the CURRENT suspended tab.
    delete model.workspaces[0].tabs[0].panes[0].suspended;
    restore(model);
    assert.deepEqual(mutations, []);
    // Missing PTY information is not evidence of an idle shell, even without a label.
    tab.label = 'Custom tab';
    herdr.processInfo = () => null;
    restore(model);
    assert.equal(mutations.some(m => m[0] === 'runInPane'), false);
    mutations.length = 0;
    herdr.focusWorkspace = () => {};
    herdr.runInPane = (...args) => mutations.push(['runInPane', ...args]);
    herdr.processInfo = () => ({ shell_pid: 123, foreground_processes: [{ pid: 123 }] });
    restore(model);
    assert.deepEqual(mutations, [['runInPane', 'w1:p1', 'codex resume exact-session']]);
    mutations.length = 0;
    herdr.processInfo = () => ({ shell_pid: 123, foreground_processes: [{ pid: 456 }] });
    restore(model);
    assert.deepEqual(mutations, []);
    model.workspaces[0].tabs[0].panes[0].suspended = true;
    live.workspaces = [];
    const missing = restore(model);
    assert.equal(missing.actions.some(a => a.action === 'workspace.create'), false);
    assert.match(missing.actions[0].detail, /native Herdr session restore required/);
    assert.deepEqual(mutations, []);
  } finally {
    Object.assign(herdr, originals);
    pstree.query = oldQuery;
    agentSessions.makeResolver = oldResolver;
  }
});
