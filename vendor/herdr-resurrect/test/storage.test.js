'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'resurrect-storage-'));
process.env.HERDR_PLUGIN_STATE_DIR = root;
process.env.HERDR_PLUGIN_CONFIG_DIR = path.join(root, 'config');
delete process.env.HERDR_SOCKET_PATH;
process.env.HERDR_SESSION = 'test';
const herdr = require('../lib/herdr');
const pstree = require('../lib/pstree');
const agentSessions = require('../lib/agent-sessions');
const { save } = require('../lib/snapshot');
const { loadModel } = require('../lib/restore');
const { LAST, SNAP_DIR, SESSIONS_DIR } = require('../lib/paths');

test('save publishes a restorable archive and prunes old snapshots in other sessions', () => {
  try {
    herdr.snapshot = () => ({ workspaces: [], tabs: [], panes: [] });
    pstree.query = () => null;
    agentSessions.makeResolver = () => () => null;
    const stopped = path.join(SESSIONS_DIR, 'stopped', 'snapshots');
    fs.mkdirSync(stopped, { recursive: true });
    const old = path.join(stopped, 'snapshot-20000101-000000.json');
    fs.writeFileSync(old, '{}');
    const result = save();
    assert.equal(fs.existsSync(old), false);
    assert.ok(fs.existsSync(result.file));
    const reference = JSON.parse(fs.readFileSync(LAST));
    assert.equal(reference.tool, 'herdr-resurrect-reference');
    assert.equal(path.join(SNAP_DIR, reference.snapshot), result.file);
    assert.deepEqual(loadModel(), result.model);
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});
