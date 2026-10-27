# Bundled private Herdr integration

Upstream: https://github.com/ntindle/herdr-resurrect
Base commit: 109a9376549e29646c77fdfb7ed8ba456a9d6617 (v0.2.0).
License: MIT; original LICENSE and attribution are retained.

This variant is embedded in the private Herdr binary. At server startup it is
extracted to a content-addressed directory under the Herdr managed plugins
folder and registered automatically. No GitHub access or separate plugin checkout
is needed. Node.js 16+ must be available on PATH; there are no npm dependencies.
Saved-space overlays additionally need bash and fzf on Linux/macOS.

The built-in plugin preserves its disabled/enabled setting across upgrades.
Explicit local/GitHub installations are preserved; switching an existing custom
installation requires linking the extracted bundled directory explicitly.
Snapshots and user settings remain in the existing plugin state/config directories.
Auto-restore remains off by default.

## Automatic snapshots and retention

The private Herdr server invokes a full-session snapshot once per hour while it
is running. No autosave pane is needed. Each named session schedules its own
snapshots. Event-driven and manual saves remain available; suspension continues
to use native session persistence and does not wait for an extra plugin save.
Disabled plugins are respected. Scheduled saves do not overlap; after system
sleep there is one due save, not a burst of missed hourly runs. Errors appear in
plugin logs (or the server log if the action cannot be started).

Every successful save removes recognized snapshot archives older than 30 days
across all named sessions, including stopped ones. Expired latest references and
legacy latest copies are also cleaned. The former 20-snapshot count limit and
HERDR_RESURRECT_KEEP override no longer apply. Named saved spaces are not pruned.
Retention runs while Herdr is running and saving; it cannot clean files while
the machine is off.

New last.json files contain a small reference rather than a duplicate session
snapshot. Restore supports both old full-model files and new references, and can
discover the newest archive if the pointer is missing or stale. Older versions
of the plugin do not understand reference files; for rollback, load an explicit
timestamped archive with the older restore script's --file option.

## Suspension behavior

- Snapshots preserve exact native agent IDs and mark suspended panes.
- Restore skips live suspended tabs, including when using older snapshots.
- Missing process data is not treated as an idle shell.
- Restore does not focus a suspended tab and accidentally resume its agent.
- Busy panes stay untouched; normal idle shells can restore commands.
- A missing workspace containing suspended panes is skipped with a message;
  use native Herdr session recovery to preserve deferred exact-session resume.

The private Herdr core pins exact session identity before suspension, protects it
from late shutdown hooks, and recovers identity from a validated native pending
resume plan. Native snapshots and the API share that identity source. Suspended
tabs always prefer the exact ID over an old generic continue command, including
when automatic startup resume is off.

The plugin currently uses the private tab-label suffix plus missing shell PID to
identify suspension because the frozen public API has no suspension field.

## Validation

From the Herdr repository: node vendor/herdr-resurrect/test/suspension.test.js
Native regression coverage lives in src/app/unread_suspend.rs and
src/agent_resume.rs; bundle lifecycle coverage lives in src/bundled_resurrect.rs.
Tests are kept in the repository and are not embedded in the runtime bundle.
