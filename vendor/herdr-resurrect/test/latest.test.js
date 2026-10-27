'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'resurrect-latest-'));
process.env.HERDR_PLUGIN_STATE_DIR = root;
process.env.HERDR_PLUGIN_CONFIG_DIR = path.join(root, 'config');
delete process.env.HERDR_SOCKET_PATH;
process.env.HERDR_SESSION = 'test';
const { LAST, SNAP_DIR } = require('../lib/paths');
const { loadModel } = require('../lib/restore');

test('latest restore supports references and legacy snapshots without losing concurrent saves', () => {
  try {
    fs.mkdirSync(SNAP_DIR, { recursive: true });
    const model = label => ({ tool: 'herdr-resurrect', workspaces: [{ label }] });
    fs.writeFileSync(LAST, JSON.stringify(model('legacy')));
    assert.equal(loadModel().workspaces[0].label, 'legacy');
    const old = 'snapshot-20261027-120000.json';
    const fresh = 'snapshot-20261027-120000-900-123.json';
    fs.writeFileSync(path.join(SNAP_DIR, old), JSON.stringify(model('old')));
    fs.writeFileSync(path.join(SNAP_DIR, fresh), JSON.stringify(model('fresh')));
    fs.writeFileSync(LAST, JSON.stringify({ tool: 'herdr-resurrect-reference', snapshot: old }));
    assert.equal(loadModel(LAST).workspaces[0].label, 'old');
    assert.equal(loadModel().workspaces[0].label, 'fresh');
    fs.unlinkSync(LAST);
    assert.equal(loadModel().workspaces[0].label, 'fresh');
    fs.writeFileSync(LAST, JSON.stringify({ tool: 'herdr-resurrect-reference', snapshot: '../outside.json' }));
    assert.throws(() => loadModel(LAST), /invalid snapshot reference/);
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});
