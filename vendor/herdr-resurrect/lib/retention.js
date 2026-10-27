'use strict';
const fs = require('fs');
const path = require('path');

const RETENTION_DAYS = 30;

function snapshotTime(name) {
  const match = /^snapshot-(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})(\d{2})(?:-(\d{3})-\d+)?\.json$/.exec(name);
  if (!match) return null;
  const iso = `${match[1]}-${match[2]}-${match[3]}T${match[4]}:${match[5]}:${match[6]}.${match[7] || '000'}Z`;
  const time = Date.parse(iso);
  return Number.isFinite(time) && new Date(time).toISOString() === iso ? time : null;
}

function entries(directory) {
  try { return fs.readdirSync(directory, { withFileTypes: true }); }
  catch (error) { if (error.code === 'ENOENT') return []; throw error; }
}

function remove(file) {
  try { fs.unlinkSync(file); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
}

// Prune only machine-generated snapshots, across named sessions including ones
// that are currently stopped. Named saved spaces and unknown files are retained.
function prune(sessionsDir, now = Date.now()) {
  const cutoff = now - RETENTION_DAYS * 24 * 60 * 60 * 1000;
  for (const session of entries(sessionsDir)) {
    if (!session.isDirectory()) continue; // never follow a session symlink
    const dir = path.join(sessionsDir, session.name);
    const snapshots = path.join(dir, 'snapshots');
    let stat;
    try { stat = fs.lstatSync(snapshots); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    if (stat?.isDirectory()) {
      for (const file of entries(snapshots)) {
        const time = snapshotTime(file.name);
        if (file.isFile() && time !== null && time < cutoff) remove(path.join(snapshots, file.name));
      }
    }
    const last = path.join(dir, 'last.json');
    try {
      if (!fs.lstatSync(last).isFile()) continue;
      const model = JSON.parse(fs.readFileSync(last, 'utf8'));
      const time = Date.parse(model.saved_at);
      if (['herdr-resurrect', 'herdr-resurrect-reference'].includes(model.tool) && Number.isFinite(time) && time < cutoff) remove(last);
    } catch (error) {
      if (error.code !== 'ENOENT' && !(error instanceof SyntaxError)) throw error;
    }
  }
}

module.exports = { prune, RETENTION_DAYS, snapshotTime };
