'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { prune } = require('../lib/retention');

test('30-day retention keeps all recent backups and prunes stopped sessions too', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'resurrect-retention-'));
  const sessions = path.join(root, 'sessions');
  const now = Date.parse('2026-10-27T12:00:00.000Z');
  try {
    for (const name of ['default', 'stopped-session']) {
      const dir = path.join(sessions, name, 'snapshots');
      fs.mkdirSync(dir, { recursive: true });
      for (const file of ['snapshot-20260927-115959.json', 'snapshot-20260927-120000.json',
        'snapshot-20260927-120000-001-123.json', 'snapshot-20261027-120000.json',
        'snapshot-20261340-120000.json', 'notes.json']) fs.writeFileSync(path.join(dir, file), '{}');
      // More than the upstream 20-file limit: all unexpired snapshots survive.
      for (let i = 0; i < 30; i++) fs.writeFileSync(path.join(dir, `snapshot-20261027-110000-${String(i).padStart(3, '0')}-123.json`), '{}');
      fs.writeFileSync(path.join(sessions, name, 'last.json'), JSON.stringify({
        tool: 'herdr-resurrect', saved_at: name === 'default' ? new Date(now).toISOString() : '2026-09-01T00:00:00Z',
      }));
    }
    fs.mkdirSync(path.join(root, 'spaces'));
    fs.writeFileSync(path.join(root, 'spaces', 'saved-workspace.json'), '{}');
    prune(sessions, now);
    for (const name of ['default', 'stopped-session']) {
      const dir = path.join(sessions, name, 'snapshots');
      assert.equal(fs.existsSync(path.join(dir, 'snapshot-20260927-115959.json')), false);
      assert.equal(fs.readdirSync(dir).length, 35);
      assert.ok(fs.existsSync(path.join(dir, 'snapshot-20260927-120000.json')));
      assert.ok(fs.existsSync(path.join(dir, 'snapshot-20260927-120000-001-123.json')));
    }
    assert.ok(fs.existsSync(path.join(sessions, 'default', 'last.json')));
    assert.equal(fs.existsSync(path.join(sessions, 'stopped-session', 'last.json')), false);
    assert.ok(fs.existsSync(path.join(root, 'spaces', 'saved-workspace.json')));
    prune(sessions, now); // concurrent/already-pruned paths remain harmless
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});
