//! Inactive, completed sessions are retained as panes and resumed on selection.
use std::time::{Duration, Instant};

use super::App;
use crate::agent_resume::AgentResumePlan;
use crate::detect::AgentState;
use crate::events::AppEvent;
use crate::terminal::TerminalState;

pub(super) const DEFAULT_DELAY_SECONDS: u64 = 24 * 60 * 60;
const DELAY_SECONDS: [u64; 6] = [0, 3600, 4 * 3600, 8 * 3600, 24 * 3600, 48 * 3600];
#[cfg(test)]
const UNREAD_TIMEOUT: Duration = Duration::from_secs(DEFAULT_DELAY_SECONDS);

fn setting_path() -> std::path::PathBuf {
    crate::config::state_dir().join("unread-suspension.json")
}

fn load_delay_at(path: &std::path::Path) -> u64 {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<u64>(&bytes).ok())
        .filter(|seconds| DELAY_SECONDS.contains(seconds))
        .unwrap_or(DEFAULT_DELAY_SECONDS)
}

pub(super) fn load_delay_seconds() -> u64 {
    load_delay_at(&setting_path())
}

fn store_delay_at(path: &std::path::Path, seconds: u64) -> std::io::Result<()> {
    if !DELAY_SECONDS.contains(&seconds) {
        return Err(std::io::Error::other("unsupported unread suspension delay"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("invalid unread suspension settings path"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".unread-suspension-{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(&seconds)?)?;
    let result = crate::platform::replace_file(&temporary, path);
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

const CHECK_INTERVAL: Duration = Duration::from_secs(60);

fn resume_plan(terminal: &TerminalState) -> Option<AgentResumePlan> {
    let label = terminal.effective_agent_label()?;
    if !matches!(label, "claude" | "codex") || terminal.state != AgentState::Idle {
        return None;
    }
    // Use an exact session identity, never a directory-wide "continue" command.
    if let Some(authority) = &terminal.hook_authority {
        if authority.agent_label == label {
            if let Some(session) = &authority.session_ref {
                return crate::agent_resume::plan(&authority.source, label, session);
            }
        }
    }
    let session = terminal.persisted_agent_session.as_ref()?;
    (session.agent == label)
        .then(|| crate::agent_resume::plan(&session.source, label, &session.session_ref))
        .flatten()
}

fn has_background_work(terminal: &TerminalState) -> bool {
    let tokens = terminal.metadata_tokens.values();
    tokens
        .get("herdr_background_activity")
        .is_some_and(|value| {
            // Malformed evidence must not be interpreted as an empty task list.
            serde_json::from_str::<serde_json::Value>(value)
                .ok()
                .and_then(|value| {
                    value
                        .get("items")
                        .and_then(|v| v.as_array())
                        .map(Vec::is_empty)
                })
                != Some(true)
        })
}

impl App {
    pub(crate) fn unread_suspend_delay_command(&self) -> String {
        format!("unread-suspend.delay.{}", self.unread_suspend_delay_seconds)
    }

    pub(crate) fn cycle_unread_suspend_delay(&mut self) -> std::io::Result<()> {
        self.cycle_unread_suspend_delay_at(&setting_path())
    }

    fn cycle_unread_suspend_delay_at(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let current = DELAY_SECONDS
            .iter()
            .position(|seconds| *seconds == self.unread_suspend_delay_seconds)
            .unwrap_or(4);
        let next = DELAY_SECONDS[(current + 1) % DELAY_SECONDS.len()];
        store_delay_at(path, next)?;
        self.unread_suspend_delay_seconds = next;
        self.unread_suspend_check_at = None;
        Ok(())
    }

    pub(crate) fn defer_failed_suspended_resume(&mut self, pane_id: crate::layout::PaneId) -> bool {
        for ws in &mut self.state.workspaces {
            if let Some(pane) = ws.pane_state_mut(pane_id) {
                if pane.suspended {
                    // Retry on the next explicit selection, not every event-loop tick.
                    pane.seen = false;
                }
                return pane.suspended;
            }
        }
        false
    }

    pub(crate) fn suspend_unread_agents(&mut self, now: Instant) -> bool {
        if self.unread_suspend_check_at.is_some_and(|next| now < next) {
            return false;
        }
        let timeout = Duration::from_secs(self.unread_suspend_delay_seconds);
        let interval = if timeout.is_zero() {
            Duration::from_secs(1)
        } else {
            CHECK_INTERVAL
        };
        self.unread_suspend_check_at = Some(now + interval);
        // Bounded maintenance cadence, independent of render/client frequency.
        let mut candidates = Vec::new();
        for ws in &self.state.workspaces {
            for tab in &ws.tabs {
                let expired = tab.panes.values().any(|pane| {
                    !pane.seen
                        && !pane.suspended
                        && pane
                            .unread_since
                            .is_some_and(|since| now.saturating_duration_since(since) >= timeout)
                });
                if !expired
                    || tab.panes.values().any(|pane| {
                        self.state
                            .terminals
                            .get(&pane.attached_terminal_id)
                            .is_some_and(|terminal| {
                                (terminal.effective_agent_label().is_some()
                                    && terminal.state != AgentState::Idle)
                                    || has_background_work(terminal)
                            })
                    })
                {
                    continue;
                }
                // A split tab is suspended only if every running agent can be resumed.
                let mut plans = Vec::new();
                let mut safe = true;
                for (&pane_id, pane) in &tab.panes {
                    if pane.suspended
                        || self
                            .terminal_runtimes
                            .get(&pane.attached_terminal_id)
                            .is_none()
                    {
                        continue;
                    }
                    let Some(terminal) = self.state.terminals.get(&pane.attached_terminal_id)
                    else {
                        safe = false;
                        break;
                    };
                    if terminal.effective_agent_label().is_none() {
                        continue;
                    }
                    let Some(plan) = resume_plan(terminal) else {
                        safe = false;
                        break;
                    };
                    plans.push((pane_id, pane.attached_terminal_id.clone(), plan));
                }
                if safe {
                    candidates.extend(plans);
                }
            }
        }
        let changed = !candidates.is_empty();
        for (pane_id, terminal_id, plan) in candidates {
            if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
                terminal.pending_agent_resume_plan = Some(plan);
            }
            for ws in &mut self.state.workspaces {
                if let Some(pane) = ws.pane_state_mut(pane_id) {
                    pane.suspended = true;
                    pane.seen = false;
                    pane.suspension_exit_pending = true;
                    break;
                }
            }
            self.shutdown_terminal_runtime(terminal_id);
        }
        if changed {
            self.schedule_session_save();
        }
        changed
    }

    pub(crate) fn consume_suspended_agent_event(&mut self, event: &AppEvent) -> bool {
        let pane_id = match event {
            AppEvent::PaneDied { pane_id, .. }
            | AppEvent::StateChanged { pane_id, .. }
            | AppEvent::AgentProcessDetected { pane_id, .. }
            | AppEvent::HookStateReported { pane_id, .. }
            | AppEvent::AgentSessionReported { pane_id, .. }
            | AppEvent::ReportedAgentShellReturned { pane_id, .. } => *pane_id,
            _ => return false,
        };
        for ws in &mut self.state.workspaces {
            if let Some(pane) = ws.pane_state_mut(pane_id) {
                if !pane.suspended {
                    return false;
                }
                if matches!(event, AppEvent::PaneDied { .. }) {
                    pane.suspension_exit_pending = false;
                }
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_resume::{AgentSessionRef, PersistedAgentSession};
    use crate::detect::Agent;
    use crate::layout::PaneId;
    use crate::terminal::{TerminalId, TerminalRuntime};

    fn setup(now: Instant, agent: Agent) -> (App, PaneId, TerminalId) {
        let (_, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            super::super::AppPolicy::TEST,
            None,
            rx,
            crate::api::EventHub::default(),
        );
        app.state = super::super::state::AppState::test_with_adversarial_identity_state();
        let ws = &mut app.state.workspaces[0];
        let pane_id = ws.tabs[0].root_pane;
        let pane = ws.tabs[0].panes.get_mut(&pane_id).unwrap();
        pane.seen = false;
        pane.unread_since = Some(now);
        let terminal_id = pane.attached_terminal_id.clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state_with_screen_signals_at(
            Some(agent),
            AgentState::Idle,
            false,
            false,
            false,
            false,
            now,
        );
        let label = crate::detect::agent_label(agent);
        terminal.set_persisted_agent_session(PersistedAgentSession {
            source: format!("herdr:{label}"),
            agent: label.into(),
            session_ref: AgentSessionRef::id("suspension-test-session").unwrap(),
        });
        app.terminal_runtimes.insert(
            terminal_id.clone(),
            TerminalRuntime::test_with_screen_bytes(80, 24, b"done"),
        );
        (app, pane_id, terminal_id)
    }

    #[tokio::test]
    async fn suspension_delay_presets_apply_at_the_selected_threshold() {
        for seconds in DELAY_SECONDS {
            let now = Instant::now();
            let (mut app, pane_id, terminal_id) = setup(now, Agent::Claude);
            app.unread_suspend_delay_seconds = seconds;
            if seconds > 0 {
                assert!(
                    !app.suspend_unread_agents(now + Duration::from_secs(seconds) - CHECK_INTERVAL)
                );
            }
            assert!(
                app.suspend_unread_agents(now + Duration::from_secs(seconds)),
                "delay {seconds}"
            );
            assert!(
                app.state.workspaces[0]
                    .pane_state(pane_id)
                    .unwrap()
                    .suspended
            );
            assert!(app.terminal_runtimes.get(&terminal_id).is_none());
        }
    }

    #[tokio::test]
    async fn delay_changes_persist_and_reset_sweep_without_overwriting_on_failure() {
        let path =
            std::env::temp_dir().join(format!("herdr-unread-delay-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(load_delay_at(&path), DEFAULT_DELAY_SECONDS);
        let (mut app, _, _) = setup(Instant::now(), Agent::Claude);
        for expected in [172800, 0, 3600, 14400, 28800, 86400] {
            app.unread_suspend_check_at = Some(Instant::now() + CHECK_INTERVAL);
            app.cycle_unread_suspend_delay_at(&path).unwrap();
            assert_eq!(app.unread_suspend_delay_seconds, expected);
            assert_eq!(load_delay_at(&path), expected);
            assert!(app.unread_suspend_check_at.is_none());
        }
        let invalid = path.join("not-a-directory");
        assert!(app.cycle_unread_suspend_delay_at(&invalid).is_err());
        assert_eq!(app.unread_suspend_delay_seconds, DEFAULT_DELAY_SECONDS);
        std::fs::write(&path, "123").unwrap();
        assert_eq!(load_delay_at(&path), DEFAULT_DELAY_SECONDS);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn unread_suspend_waits_24_hours_and_retains_identity_and_tab() {
        for agent in [Agent::Claude, Agent::Codex] {
            let now = Instant::now();
            let (mut app, pane_id, terminal_id) = setup(now, agent);
            assert!(!app.suspend_unread_agents(now + UNREAD_TIMEOUT - CHECK_INTERVAL));
            assert!(app.suspend_unread_agents(now + UNREAD_TIMEOUT));
            assert!(app.terminal_runtimes.get(&terminal_id).is_none());
            let pane = app.state.workspaces[0].pane_state(pane_id).unwrap();
            assert!(pane.suspended);
            assert!(pane.suspension_exit_pending);
            assert!(app.state.workspaces[0]
                .tab_display_name(0)
                .unwrap()
                .ends_with(" (Suspended)"));
            assert!(app.state.terminals[&terminal_id]
                .pending_agent_resume_plan
                .as_ref()
                .unwrap()
                .argv
                .iter()
                .any(|arg| arg == "suspension-test-session"));
            let snapshot = crate::persist::capture(
                &app.state.workspaces,
                &app.state.terminals,
                &app.terminal_runtimes,
                app.state.active,
                app.state.selected,
            );
            let serialized = serde_json::to_string(&snapshot).unwrap();
            let snapshot: crate::persist::SessionSnapshot =
                serde_json::from_str(&serialized).unwrap();
            let (tx, _) = tokio::sync::mpsc::channel(32);
            let (workspaces, terminals, runtimes) = crate::persist::restore(
                &snapshot,
                None,
                24,
                80,
                0,
                &app.state.default_shell,
                app.state.shell_mode,
                true,
                tx,
                std::sync::Arc::new(tokio::sync::Notify::new()),
                std::sync::Arc::new(crate::render_signal::RenderSignal::new()),
            );
            assert!(workspaces[0].tabs[0]
                .panes
                .values()
                .any(|pane| pane.suspended && !pane.seen));
            assert!(terminals
                .values()
                .any(|terminal| terminal.pending_agent_resume_plan.is_some()));
            drop(runtimes);
            app.handle_internal_event(AppEvent::PaneDied {
                pane_id,
                exit_reason: crate::platform::ChildExitReason::Exited,
            });
            assert!(app.state.workspaces[0].pane_state(pane_id).is_some());
            assert!(!app.start_pending_agent_resume_for_terminal(&terminal_id, 24, 80, true));
            app.state.assert_invariants_for_test();
        }
    }

    #[tokio::test]
    async fn unread_suspend_skips_read_working_and_unresumable_sessions() {
        for scenario in ["read", "working", "missing-session", "background"] {
            let now = Instant::now();
            let (mut app, pane_id, terminal_id) = setup(now, Agent::Claude);
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
            match scenario {
                "read" => {
                    app.state.workspaces[0]
                        .pane_state_mut(pane_id)
                        .unwrap()
                        .seen = true
                }
                "working" => terminal.state = AgentState::Working,
                "missing-session" => terminal.persisted_agent_session = None,
                "background" => {
                    terminal.metadata_tokens.patch(
                        std::collections::HashMap::from([(
                            "herdr_background_activity".into(),
                            Some(r#"{"items":[{"id":"task"}]}"#.into()),
                        )]),
                        None,
                        now,
                    );
                }
                _ => unreachable!(),
            }
            assert!(
                !app.suspend_unread_agents(now + UNREAD_TIMEOUT),
                "{scenario}"
            );
            assert!(app.terminal_runtimes.get(&terminal_id).is_some());
        }
    }

    #[tokio::test]
    async fn unread_suspend_opening_tab_resumes_after_exit_and_restores_title() {
        let now = Instant::now();
        let (mut app, pane_id, terminal_id) = setup(now, Agent::Codex);
        assert!(app.suspend_unread_agents(now + UNREAD_TIMEOUT));
        app.state.workspaces[0].switch_tab(0);
        assert!(!app.start_pending_agent_resume_for_terminal(&terminal_id, 24, 80, true));
        app.handle_internal_event(AppEvent::PaneDied {
            pane_id,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });
        // Exercise shell startup without launching an actual coding agent.
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .pending_agent_resume_plan
            .as_mut()
            .unwrap()
            .argv = vec!["echo".into(), "resumed".into()];
        assert!(app.start_pending_agent_resume_for_terminal(&terminal_id, 24, 80, true));
        assert!(app.terminal_runtimes.get(&terminal_id).is_some());
        assert!(
            !app.state.workspaces[0]
                .pane_state(pane_id)
                .unwrap()
                .suspended
        );
        assert!(!app.state.workspaces[0]
            .tab_display_name(0)
            .unwrap()
            .ends_with(" (Suspended)"));
        app.state.assert_invariants_for_test();
    }
    #[tokio::test]
    async fn unread_suspend_failed_resume_retains_plan_for_next_selection() {
        let now = Instant::now();
        let (mut app, pane_id, terminal_id) = setup(now, Agent::Codex);
        assert!(app.suspend_unread_agents(now + UNREAD_TIMEOUT));
        app.handle_internal_event(AppEvent::PaneDied {
            pane_id,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });
        app.state.workspaces[0].switch_tab(0);
        app.state.terminals.get_mut(&terminal_id).unwrap().cwd = std::env::current_dir()
            .unwrap()
            .join("__missing_suspend_resume_directory__");
        assert!(app.start_pending_agent_resume_for_terminal(&terminal_id, 24, 80, true));
        assert!(app.state.terminals[&terminal_id]
            .pending_agent_resume_plan
            .is_some());
        let pane = app.state.workspaces[0].pane_state(pane_id).unwrap();
        assert!(pane.suspended && !pane.seen);
        assert!(!app.start_pending_agent_resume_for_terminal(&terminal_id, 24, 80, true));
    }
}
