#!/usr/bin/env node
'use strict';
// List saved snapshots, newest last, with a one-line summary each.
const fs = require('fs');
const path = require('path');
const { SNAP_DIR } = require('../lib/paths');
const { snapshotTime } = require('../lib/retention');

function summarize(model) {
  let w = model.workspaces.length, t = 0, p = 0, c = 0, a = 0;
  for (const ws of model.workspaces)
    for (const tab of ws.tabs) {
      t++;
      for (const pane of tab.panes) {
        p++;
        if (pane.agent) a++;
        else if (pane.command && pane.command.restorable) c++;
      }
    }
  return `${w}ws ${t}tab ${p}pane — ${c} cmd, ${a} agent`;
}

try {
  const files = fs.existsSync(SNAP_DIR)
    ? fs.readdirSync(SNAP_DIR).filter(f => snapshotTime(f) !== null)
      .sort((a, b) => snapshotTime(a) - snapshotTime(b) || a.localeCompare(b))
    : [];
  if (!files.length) {
    console.log('herdr-resurrect: no snapshots yet. Run the "save" action to make one.');
    process.exit(0);
  }
  // Mark the newest immutable archive; last.json is only an advisory reference.
  const newest = files[files.length - 1];
  console.log(`herdr-resurrect snapshots (${files.length}) in ${SNAP_DIR}:\n`);
  for (const f of files) {
    const full = path.join(SNAP_DIR, f);
    let line;
    try {
      const model = JSON.parse(fs.readFileSync(full, 'utf8'));
      line = `${new Date(model.saved_at).toLocaleString().padEnd(22)}  ${summarize(model)}`;
    } catch {
      line = '(unreadable)';
    }
    const marker = f === newest ? ' *' : '  ';
    console.log(`${marker} ${f}\n     ${line}`);
  }
  console.log('\n( * = most recent / what "restore" uses )');
} catch (e) {
  console.error(`herdr-resurrect list failed: ${e.message}`);
  process.exit(1);
}
