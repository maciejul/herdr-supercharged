// Live integration check. Only starts/stops its own disposable Herdr session.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const bin = path.resolve(process.argv[2] || 'target/debug/herdr');
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'herdr-bundled-resurrect-'));
const env = { ...process.env, XDG_CONFIG_HOME: path.join(root, 'config'), XDG_STATE_HOME: path.join(root, 'state') };
for (const key of ['HERDR_SOCKET_PATH', 'HERDR_CLIENT_SOCKET_PATH', 'HERDR_CONFIG_PATH', 'HERDR_SESSION']) delete env[key];
const help = spawnSync(bin, ['--help'], { env, encoding: 'utf8' });
assert.equal(help.status, 0, help.stderr);
const configFile = /Config:\s+([^\n]+)/.exec(help.stdout)[1].trim();
assert.ok(configFile.startsWith(root));
fs.mkdirSync(path.dirname(configFile), { recursive: true });
fs.writeFileSync(configFile, '[terminal]\ndefault_shell = "/bin/sh"\n[session]\nresume_agents_on_restore = false\n');
const name = 'resurrect-smoke';
const sessionDir = path.join(path.dirname(configFile), 'sessions', name);
fs.mkdirSync(sessionDir, { recursive: true });
const exact = '11111111-1111-4111-8111-111111111111';
const savedPane = {
  suspended: true, cwd: root,
  agent_session: { source: 'herdr:codex', agent: 'codex', kind: 'id', value: exact },
  agent_resume: { source: 'old-hook', agent: 'codex', argv: ['codex', 'resume', '--last'] },
};
const workspace = (id, number, suspended) => ({
  id, custom_name: suspended ? 'Saved agent' : 'Control', identity_cwd: root,
  public_pane_numbers: { [number]: 1 }, next_public_pane_number: 2,
  public_tab_numbers: [1], next_public_tab_number: 2, active_tab: 0,
  tabs: [{ custom_name: suspended ? 'Exact conversation' : 'Control',
    layout: { Pane: number }, panes: { [number]: suspended ? savedPane : { suspended: false, cwd: root } },
    zoomed: false, focused: number, root_pane: number }],
});
fs.writeFileSync(path.join(sessionDir, 'session.json'), JSON.stringify({
  version: 3, workspaces: [workspace('w1', 1, true), workspace('w2', 2, false)], active: 1, selected: 1,
}));
const server = spawn(bin, ['--session', name, 'server'], { env, stdio: ['ignore', 'pipe', 'pipe'] });
let serverLog = '';
server.stdout.on('data', b => { serverLog += b; });
server.stderr.on('data', b => { serverLog += b; });
const cliEnv = { ...env, HERDR_SOCKET_PATH: path.join(sessionDir, 'herdr.sock') };
const cli = (...args) => {
  const result = spawnSync(bin, args, { env: cliEnv, encoding: 'utf8', timeout: 15000 });
  assert.equal(result.status, 0, `${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim() ? JSON.parse(result.stdout).result : null;
};
const assertNoRuntime = paneId => {
  const result = spawnSync(bin, ['pane', 'process-info', '--pane', paneId], { env: cliEnv, encoding: 'utf8', timeout: 15000 });
  if (result.status === 0) {
    assert.ok(!JSON.parse(result.stdout).result.process_info.shell_pid);
  } else {
    // Pane existence is checked through snapshot separately; the process API
    // reports pane_not_found when the retained pane has no runtime.
    assert.equal(JSON.parse(result.stderr).error.code, 'pane_not_found');
  }
};
const waitFor = async (fn, label) => {
  for (let i = 0; i < 100; i++) {
    if (fn()) return;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`${label} timed out: ${serverLog}`);
};
try {
  await waitFor(() => fs.existsSync(cliEnv.HERDR_SOCKET_PATH), 'server startup');
  const before = cli('api', 'snapshot').snapshot;
  assert.equal(before.tabs.find(t => t.tab_id === 'w1:t1').label, 'Exact conversation (Suspended)');
  assert.equal(before.panes.find(p => p.pane_id === 'w1:p1').agent_session.value, exact);
  assertNoRuntime('w1:p1');
  const invoke = async action => {
    const { log } = cli('plugin', 'action', 'invoke', `ntindle.herdr-resurrect.${action}`);
    await waitFor(() => {
      const item = cli('plugin', 'log', '--plugin', 'ntindle.herdr-resurrect', '--limit', '100').logs.find(l => l.log_id === log.log_id);
      if (!item || item.status === 'running') return false;
      assert.equal(item.status, 'succeeded', JSON.stringify(item));
      return true;
    }, action);
  };
  const appDir = path.basename(path.dirname(configFile));
  const pluginState = path.join(env.XDG_STATE_HOME, appDir, 'plugins', 'ntindle.herdr-resurrect');
  const store = path.join(pluginState, 'sessions', name);
  const archiveDir = path.join(store, 'snapshots');
  const stoppedDir = path.join(pluginState, 'sessions', 'stopped', 'snapshots');
  fs.mkdirSync(archiveDir, { recursive: true });
  fs.mkdirSync(stoppedDir, { recursive: true });
  const oldArchive = path.join(stoppedDir, 'snapshot-20000101-000000.json');
  fs.writeFileSync(oldArchive, '{}');
  fs.writeFileSync(path.join(path.dirname(stoppedDir), 'last.json'), JSON.stringify({ tool: 'herdr-resurrect', saved_at: '2000-01-01T00:00:00Z' }));
  for (let i = 1; i <= 30; i++) {
    const stamp = new Date(Date.now() - i * 60000).toISOString().replace(/[-:]/g, '').replace('T', '-').replace(/\..*$/, '');
    fs.writeFileSync(path.join(archiveDir, `snapshot-${stamp}.json`), '{}');
  }
  await invoke('save');
  assert.equal(fs.existsSync(oldArchive), false, 'expired stopped-session snapshot is pruned');
  assert.equal(fs.existsSync(path.join(path.dirname(stoppedDir), 'last.json')), false);
  assert.ok(fs.readdirSync(archiveDir).length > 30, 'recent history is not truncated to 20 files');
  await invoke('restore-preview');
  await invoke('restore');
  const controlPid = cli('pane', 'process-info', '--pane', 'w2:p1').process_info.shell_pid;
  assert.ok(controlPid);
  cli('server', 'live-handoff', '--import-exe', bin);
  assert.equal(cli('pane', 'process-info', '--pane', 'w2:p1').process_info.shell_pid, controlPid,
    'Live handoff must preserve the running control shell');
  const after = cli('api', 'snapshot').snapshot;
  assert.equal(after.focused_tab_id, before.focused_tab_id);
  assert.equal(after.workspaces.length, before.workspaces.length);
  assert.equal(after.tabs.find(t => t.tab_id === 'w1:t1').label, 'Exact conversation (Suspended)');
  assert.equal(after.panes.find(p => p.pane_id === 'w1:p1').agent_session.value, exact);
  assertNoRuntime('w1:p1');
  const latest = JSON.parse(fs.readFileSync(path.join(store, 'last.json')));
  const snap = latest.tool === 'herdr-resurrect-reference'
    ? JSON.parse(fs.readFileSync(path.join(store, 'snapshots', latest.snapshot))) : latest;
  const agent = snap.workspaces.flatMap(w => w.tabs.flatMap(t => t.panes)).find(p => p.pane_id === 'w1:p1');
  assert.equal(agent.suspended, true);
  assert.equal(agent.agent.session.value, exact);
  console.log('PASS: bundled offline install, native deferred restore, plugin save/preview/restore and live handoff preserve exact ID, suspension, names, topology, focus and running shell.');
} finally {
  if (fs.existsSync(cliEnv.HERDR_SOCKET_PATH)) cli('server', 'stop');
  if (server.exitCode === null) await new Promise(resolve => server.once('exit', resolve));
  fs.rmSync(root, { recursive: true, force: true });
}
