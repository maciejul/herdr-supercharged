//! Hourly snapshots use the normal asynchronous plugin runner, without a pane.
use std::time::{Duration, Instant};

use super::App;
use crate::api::schema::PluginCommandStatus;

pub(super) const INTERVAL: Duration = Duration::from_secs(3600);
const PLUGIN: &str = "ntindle.herdr-resurrect";

impl App {
    pub(crate) fn run_periodic_resurrect_snapshot(&mut self, now: Instant) {
        if self
            .next_resurrect_snapshot_at
            .is_none_or(|deadline| now < deadline)
        {
            return;
        }
        // Do not duplicate an in-flight save or start a burst after system sleep.
        if self.pending_resurrect_snapshot_log_id.is_some()
            || self.state.plugin_command_logs.iter().any(|log| {
                log.plugin_id == PLUGIN
                    && log.action_id.as_deref() == Some("save")
                    && log.status == PluginCommandStatus::Running
            })
        {
            self.next_resurrect_snapshot_at = Some(now + Duration::from_secs(60));
            return;
        }
        self.next_resurrect_snapshot_at = Some(now + INTERVAL);
        match self.invoke_plugin_action_from_source(
            format!("{PLUGIN}.save"),
            None,
            "hourly_snapshot",
        ) {
            Ok(log) => self.pending_resurrect_snapshot_log_id = Some(log.log_id),
            Err(error) => tracing::warn!(%error, "hourly resurrect snapshot could not start"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_save_action(enabled: bool) -> (App, std::path::PathBuf) {
        let (_, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            super::super::AppPolicy::TEST,
            None,
            rx,
            crate::api::EventHub::default(),
        );
        let root =
            std::env::temp_dir().join(format!("herdr-hourly-{}-{enabled}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let program =
            serde_json::to_string(&std::env::current_exe().unwrap().display().to_string()).unwrap();
        std::fs::write(root.join("herdr-plugin.toml"), format!(
            "id = \"{PLUGIN}\"\nname = \"Test\"\nversion = \"0.1.0\"\nmin_herdr_version = \"0.7.0\"\n[[actions]]\nid = \"save\"\ntitle = \"Save\"\ncommand = [{program}, \"--help\"]\n"
        )).unwrap();
        let plugin = crate::app::load_plugin_manifest(&root.to_string_lossy(), enabled).unwrap();
        app.state.installed_plugins.insert(PLUGIN.into(), plugin);
        (app, root)
    }

    #[tokio::test]
    async fn hourly_snapshot_waits_for_deadline_runs_once_and_avoids_overlap() {
        let (mut app, root) = app_with_save_action(true);
        let start = Instant::now();
        app.next_resurrect_snapshot_at = Some(start + INTERVAL);
        app.run_periodic_resurrect_snapshot(start + INTERVAL - Duration::from_secs(1));
        assert!(app.state.plugin_command_logs.is_empty());
        app.run_periodic_resurrect_snapshot(start + INTERVAL);
        assert_eq!(app.state.plugin_command_logs.len(), 1);
        assert_eq!(
            app.state.plugin_command_logs[0].action_id.as_deref(),
            Some("save")
        );
        assert_eq!(app.next_resurrect_snapshot_at, Some(start + INTERVAL * 2));
        app.state.plugin_command_logs.clear(); // event storms may evict a running log
        app.run_periodic_resurrect_snapshot(start + INTERVAL * 2);
        assert_eq!(
            app.state.plugin_command_logs.len(),
            0,
            "do not overlap a running save"
        );
        assert_eq!(
            app.next_resurrect_snapshot_at,
            Some(start + INTERVAL * 2 + Duration::from_secs(60))
        );
        for _ in 0..100 {
            app.drain_all_internal_events();
            if app.state.plugin_commands_in_flight == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            app.pending_resurrect_snapshot_log_id.is_none(),
            "completion must release an evicted log token"
        );
        app.run_periodic_resurrect_snapshot(start + INTERVAL * 2 + Duration::from_secs(60));
        assert_eq!(app.state.plugin_command_logs.len(), 1);
        for _ in 0..100 {
            app.drain_all_internal_events();
            if app.state.plugin_commands_in_flight == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            app.state.plugin_command_logs[0].status,
            PluginCommandStatus::Succeeded
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn hourly_snapshot_respects_disabled_plugin_and_rearms_after_sleep() {
        let (mut app, root) = app_with_save_action(false);
        let start = Instant::now();
        app.next_resurrect_snapshot_at = Some(start);
        let later = start + INTERVAL * 24;
        app.run_periodic_resurrect_snapshot(later);
        assert!(app.state.plugin_command_logs.is_empty());
        assert_eq!(app.next_resurrect_snapshot_at, Some(later + INTERVAL));
        std::fs::remove_dir_all(root).unwrap();
    }
}
