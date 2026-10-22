//! Unix background lifecycle hooks share a small collector, independent of session hooks.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(super) const NAME: &str = "herdr-background-activity.py";
const ASSET: &str = include_str!("assets/herdr-background-activity.py");
pub(super) const EVENTS: &[(&str, Option<&str>)] = &[
    ("SessionStart", None),
    ("SubagentStart", None),
    ("SubagentStop", None),
    ("SessionEnd", None),
    ("Stop", None),
    ("PreToolUse", Some("^(.*\\.)?(Bash|exec_command)$")),
    (
        "PostToolUse",
        Some("^(.*\\.)?(Bash|exec_command|write_stdin|TaskOutput|TaskStop)$"),
    ),
];

/// Claude has a separate failure event; Codex reports shell exits via PostToolUse.
pub(super) fn events_for_command(command: &str) -> Vec<(&'static str, Option<&'static str>)> {
    let mut events = EVENTS.to_vec();
    if command.ends_with(" claude") {
        events.push(("PostToolUseFailure", Some("^(.*\\.)?(Bash|exec_command)$")));
        events.push(("StopFailure", None));
    } else {
        events.push(("Interrupt", None));
    }
    events
}

fn companion_path(hook_path: &Path) -> PathBuf {
    hook_path.with_file_name(NAME)
}

pub(super) fn edit(
    content: &str,
    settings_path: &Path,
    hook_path: &Path,
    agent: &str,
    installing: bool,
) -> io::Result<String> {
    let path = companion_path(hook_path);
    let command = format!(
        "python3 {} {}",
        super::command::shell_single_quote(&path.to_string_lossy()),
        agent
    );
    let installing = installing && !disabled_path(hook_path).exists();
    super::claude_settings::edit_background_hooks(content, settings_path, &command, installing)
}

pub(super) fn install_file(hook_path: &Path) -> io::Result<()> {
    fs::write(companion_path(hook_path), ASSET)
}

pub(super) fn remove_file(hook_path: &Path) -> io::Result<bool> {
    super::file_ops::remove_file_if_exists(&companion_path(hook_path))
}

fn disabled_path(hook_path: &Path) -> PathBuf {
    hook_path.with_file_name(format!("{NAME}.disabled"))
}

struct Target {
    hook: PathBuf,
    config: PathBuf,
    agent: &'static str,
}

fn installed_targets() -> io::Result<Vec<Target>> {
    let candidates = [
        (
            super::env::claude_dir()?,
            "hooks",
            super::CLAUDE_HOOK_INSTALL_NAME,
            "settings.json",
            "claude",
        ),
        (
            super::env::codex_dir()?,
            "",
            super::CODEX_HOOK_INSTALL_NAME,
            "hooks.json",
            "codex",
        ),
    ];
    Ok(candidates
        .into_iter()
        .filter_map(|(dir, subdir, name, config, agent)| {
            let hook = dir.join(subdir).join(name);
            hook.is_file().then(|| Target {
                hook,
                config: dir.join(config),
                agent,
            })
        })
        .collect())
}

/// Read at startup/config refresh only; snapshot rendering uses the cached result.
pub(crate) fn tracking_enabled() -> Option<bool> {
    let targets = installed_targets().ok()?;
    if targets.is_empty() {
        return None;
    }
    Some(targets.iter().all(|target| {
        if disabled_path(&target.hook).exists() || !companion_path(&target.hook).is_file() {
            return false;
        }
        let Ok(content) = fs::read_to_string(&target.config) else {
            return false;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
            return false;
        };
        let command = format!(
            "python3 {} {}",
            super::command::shell_single_quote(&companion_path(&target.hook).to_string_lossy()),
            target.agent
        );
        events_for_command(&command).iter().all(|(event, _)| {
            value["hooks"][*event].as_array().is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry["hooks"].as_array().is_some_and(|hooks| {
                        hooks
                            .iter()
                            .any(|hook| hook["command"].as_str() == Some(command.as_str()))
                    })
                })
            })
        })
    }))
}

struct Change {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

fn change(path: PathBuf, after: Option<Vec<u8>>) -> io::Result<Change> {
    super::config_file::check_config_target(&path)?;
    let before = match fs::read(&path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    Ok(Change {
        path,
        before,
        after,
    })
}

fn publish(path: &Path, content: Option<&[u8]>) -> io::Result<()> {
    match content {
        Some(bytes) => super::config_file::write_config(path, bytes),
        None => super::file_ops::remove_file_if_exists(path).map(|_| ()),
    }
}

fn apply_changes(changes: Vec<Change>) -> io::Result<()> {
    for (index, entry) in changes.iter().enumerate() {
        if let Err(error) = publish(&entry.path, entry.after.as_deref()) {
            let mut failures = Vec::new();
            // Include the failed target: a platform writer may have partially written.
            for original in changes[..=index].iter().rev() {
                if let Err(rollback) = publish(&original.path, original.before.as_deref()) {
                    failures.push(format!("{}: {rollback}", original.path.display()));
                }
            }
            return Err(io::Error::other(if failures.is_empty() {
                format!("background tracking update failed and was rolled back: {error}")
            } else {
                format!(
                    "background tracking update failed: {error}; rollback failed: {}",
                    failures.join("; ")
                )
            }));
        }
    }
    Ok(())
}

/// Preflight both integrations before any mutation and restore prior bytes on failure.
pub(crate) fn set_tracking_enabled(enabled: bool) -> io::Result<()> {
    let targets = installed_targets()?;
    if targets.is_empty() {
        return Err(io::Error::other(
            "install a Claude or Codex integration before changing background tracking",
        ));
    }
    let generation = format!("{}-{:?}", std::process::id(), std::time::SystemTime::now());
    let mut changes = Vec::new();
    // Disable workers before touching hooks. Enable only after every hook is ready.
    if !enabled {
        for target in &targets {
            changes.push(change(
                disabled_path(&target.hook),
                Some(b"disabled\n".to_vec()),
            )?);
        }
    }
    for target in &targets {
        let content = fs::read_to_string(&target.config)?;
        let command = format!(
            "python3 {} {}",
            super::command::shell_single_quote(&companion_path(&target.hook).to_string_lossy()),
            target.agent
        );
        let updated = super::claude_settings::edit_background_hooks(
            &content,
            &target.config,
            &command,
            enabled,
        )?;
        changes.push(change(
            companion_path(&target.hook),
            Some(ASSET.as_bytes().to_vec()),
        )?);
        changes.push(change(
            target.hook.with_file_name(format!("{NAME}.generation")),
            Some(generation.as_bytes().to_vec()),
        )?);
        changes.push(change(target.config.clone(), Some(updated.into_bytes()))?);
    }
    if enabled {
        for target in &targets {
            changes.push(change(disabled_path(&target.hook), None)?);
        }
    }
    apply_changes(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_foreign_commands_formatting_and_session_hook_on_install_and_remove() {
        let path = Path::new("/tmp/settings.json");
        let hook = Path::new("/tmp/hooks/herdr-agent-state.sh");
        let foreign = r#"{  "type" : "command", "command" : "echo keep"  }"#;
        let source = format!(
            r#"{{"untouched":{{"n":1e+02}},"hooks":{{"SessionStart":[{{"hooks":[{foreign}]}}]}}}}"#
        );
        let installed = edit(&source, path, hook, "claude", true).unwrap();
        assert!(installed.contains(foreign));
        assert!(installed.contains(r#""untouched":{"n":1e+02}"#));
        assert_eq!(
            installed,
            edit(&installed, path, hook, "claude", true).unwrap()
        );
        let parsed: serde_json::Value = serde_json::from_str(&installed).unwrap();
        assert_eq!(parsed["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        for (event, _) in EVENTS {
            assert!(parsed["hooks"][*event].is_array());
        }
        let removed = edit(&installed, path, hook, "claude", false).unwrap();
        assert!(removed.contains(foreign));
        assert!(!removed.contains(NAME));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&removed).unwrap(),
            serde_json::from_str::<serde_json::Value>(&source).unwrap()
        );
    }

    #[test]
    fn uninstall_keeps_foreign_empty_hooks() {
        let source =
            r#"{"hooks":{"SessionEnd":[],"SubagentStart":[{"matcher":"foreign","hooks":[]}]}}"#;
        let removed = edit(
            source,
            Path::new("/tmp/settings.json"),
            Path::new("/tmp/hook.sh"),
            "claude",
            false,
        )
        .unwrap();
        assert_eq!(source, removed);
    }

    #[test]
    fn failed_publication_restores_previously_changed_files() {
        let dir =
            std::env::temp_dir().join(format!("herdr-tracking-rollback-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first");
        let failing = dir.join("failing");
        fs::write(&first, b"original").unwrap();
        let changes = vec![
            change(first.clone(), Some(b"updated".to_vec())).unwrap(),
            change(failing.clone(), Some(b"new".to_vec())).unwrap(),
        ];
        // Simulate a target becoming unwritable after preflight.
        fs::create_dir(&failing).unwrap();
        assert!(apply_changes(changes).is_err());
        assert_eq!(fs::read(first).unwrap(), b"original");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_event_arrays_are_rejected_without_editing() {
        assert!(edit(
            r#"{"hooks":{"SubagentStart":{}}}"#,
            Path::new("/tmp/settings.json"),
            Path::new("/tmp/hook.sh"),
            "codex",
            true
        )
        .is_err());
    }
}
