//! Derive display labels from existing agent metadata without renaming server IDs.
use super::*;

type AgentOwners = HashSet<(String, Option<String>)>;

#[derive(Clone, Debug, Default)]
pub(super) struct OriginalNames {
    tabs: HashMap<String, String>,
    workspaces: HashMap<String, String>,
    retained_tabs: HashMap<String, String>,
    retained_workspaces: HashMap<String, String>,
    tab_owners: HashMap<String, AgentOwners>,
    workspace_owners: HashMap<String, AgentOwners>,
}

impl OriginalNames {
    pub(super) fn capture(snapshot: &ClientShellSnapshot) -> Self {
        Self {
            tabs: snapshot
                .tabs
                .iter()
                .map(|tab| (tab.tab_id.clone(), tab.label.clone()))
                .collect(),
            workspaces: snapshot
                .workspaces
                .iter()
                .map(|workspace| (workspace.workspace_id.clone(), workspace.label.clone()))
                .collect(),
            ..Default::default()
        }
    }

    /// Keep meaningful names across transient terminal activity titles, missing
    /// metadata, and suspension. Explicit names always remain authoritative.
    pub(super) fn project(
        &mut self,
        snapshot: &mut ClientShellSnapshot,
        previous: Option<&ClientShellSnapshot>,
        preferences: &preferences::CustomizationPreferences,
    ) {
        observe_agent_owners(
            &mut self.retained_tabs,
            &mut self.tab_owners,
            snapshot,
            true,
        );
        observe_agent_owners(
            &mut self.retained_workspaces,
            &mut self.workspace_owners,
            snapshot,
            false,
        );
        let originals = Self::capture(snapshot);
        let (named_tabs, named_workspaces) = apply(snapshot, preferences);
        self.retained_tabs
            .retain(|id, _| originals.tabs.contains_key(id));
        self.retained_workspaces
            .retain(|id, _| originals.workspaces.contains_key(id));
        let mut suspended_workspaces = HashSet::new();
        for tab in &mut snapshot.tabs {
            let suspended = originals
                .tabs
                .get(&tab.tab_id)
                .is_some_and(|label| label.ends_with(" (Suspended)"));
            if suspended {
                suspended_workspaces.insert(tab.workspace_id.clone());
            }
            if tab.custom_label {
                self.retained_tabs.remove(&tab.tab_id);
                continue;
            }
            if !preferences.automatic_tab_names {
                continue;
            }
            if suspended {
                let previous_name = previous
                    .and_then(|s| s.tabs.iter().find(|t| t.tab_id == tab.tab_id))
                    .map(|t| t.label.as_str())
                    .unwrap_or(&tab.label);
                self.retained_tabs
                    .entry(tab.tab_id.clone())
                    .or_insert_with(|| {
                        previous_name
                            .strip_suffix(" (Suspended)")
                            .unwrap_or(previous_name)
                            .to_owned()
                    });
            } else if named_tabs.contains(&tab.tab_id) {
                self.retained_tabs
                    .insert(tab.tab_id.clone(), tab.label.clone());
            }
            if let Some(name) = self.retained_tabs.get(&tab.tab_id) {
                tab.label = if suspended {
                    format!("{name} (Suspended)")
                } else {
                    name.clone()
                };
            }
        }
        for workspace in &mut snapshot.workspaces {
            if workspace.custom_label {
                self.retained_workspaces.remove(&workspace.workspace_id);
                continue;
            }
            if !preferences.automatic_workspace_names {
                continue;
            }
            if suspended_workspaces.contains(&workspace.workspace_id) {
                let previous_name = previous
                    .and_then(|s| {
                        s.workspaces
                            .iter()
                            .find(|w| w.workspace_id == workspace.workspace_id)
                    })
                    .map(|w| &w.label)
                    .unwrap_or(&workspace.label);
                self.retained_workspaces
                    .entry(workspace.workspace_id.clone())
                    .or_insert_with(|| previous_name.clone());
            } else if named_workspaces.contains(&workspace.workspace_id) {
                self.retained_workspaces
                    .insert(workspace.workspace_id.clone(), workspace.label.clone());
            }
            if let Some(name) = self.retained_workspaces.get(&workspace.workspace_id) {
                workspace.label.clone_from(name);
            }
        }
        self.tab_owners
            .retain(|id, _| self.retained_tabs.contains_key(id));
        self.workspace_owners
            .retain(|id, _| self.retained_workspaces.contains_key(id));
        self.tabs = originals.tabs;
        self.workspaces = originals.workspaces;
    }

    pub(super) fn restore(&self, snapshot: &mut ClientShellSnapshot) {
        for tab in &mut snapshot.tabs {
            if let Some(label) = self.tabs.get(&tab.tab_id) {
                tab.label.clone_from(label);
            }
        }
        for workspace in &mut snapshot.workspaces {
            if let Some(label) = self.workspaces.get(&workspace.workspace_id) {
                workspace.label.clone_from(label);
            }
        }
    }
}

/// Absence can be a temporary metadata gap. Positive ownership replacement is
/// different: an old task label must not migrate to a new agent in the same Space.
fn observe_agent_owners(
    retained: &mut HashMap<String, String>,
    known: &mut HashMap<String, AgentOwners>,
    snapshot: &ClientShellSnapshot,
    tabs: bool,
) {
    let mut current: HashMap<&str, HashSet<(&str, Option<&str>)>> = HashMap::new();
    for agent in &snapshot.agents {
        let id = if tabs {
            agent.tab_id.as_str()
        } else {
            agent.workspace_id.as_str()
        };
        current
            .entry(id)
            .or_default()
            .insert((agent.pane_id.as_str(), agent.agent.as_deref()));
    }
    for (id, owners) in current {
        let previous = known.get(id);
        let unchanged = previous.is_some_and(|previous| {
            previous.len() == owners.len()
                && previous
                    .iter()
                    .all(|(pane, kind)| owners.contains(&(pane.as_str(), kind.as_deref())))
        });
        if unchanged {
            continue;
        }
        if previous.is_some() {
            retained.remove(id);
        }
        known.insert(
            id.to_owned(),
            owners
                .into_iter()
                .map(|(pane, kind)| (pane.to_owned(), kind.map(str::to_owned)))
                .collect(),
        );
    }
}

fn conversation_name(agent: &crate::protocol::ClientShellAgent) -> Option<String> {
    let kind = agent.agent.as_deref()?;
    if !matches!(kind, "claude" | "codex") {
        return None;
    }
    [
        (agent.title.as_deref(), false),
        (agent.terminal_title_stripped.as_deref(), true),
        (agent.name.as_deref(), false),
    ]
    .into_iter()
    .filter_map(|(name, terminal_title)| name.map(|name| (name.trim(), terminal_title)))
    .find(|(name, terminal_title)| {
        !name.is_empty()
            && (!terminal_title || !is_transient_terminal_title(name))
            && !name.eq_ignore_ascii_case(kind)
            && !matches!(
                name.to_ascii_lowercase().as_str(),
                "claude code" | "openai codex" | "codex cli"
            )
    })
    .map(|(name, _)| name.chars().filter(|c| !c.is_control()).take(120).collect())
}

/// OSC titles can carry agent activity or a provisional thread ID rather than
/// a conversation name. Only reject whole placeholders, never words in a task
/// title or an authoritative metadata title/name.
fn is_transient_terminal_title(name: &str) -> bool {
    let status = name.trim_end_matches(['.', '…']).trim_end();
    [
        "working",
        "ready",
        "idle",
        "thinking",
        "waiting",
        "busy",
        "running",
        "starting",
        "loading",
        "processing",
        "connecting",
        "compacting",
    ]
    .iter()
    .any(|candidate| status.eq_ignore_ascii_case(candidate))
        || (name.len() == 36
            && name.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            }))
}

pub(super) fn apply(
    snapshot: &mut ClientShellSnapshot,
    preferences: &preferences::CustomizationPreferences,
) -> (HashSet<String>, HashSet<String>) {
    let mut named_tabs = HashSet::new();
    let mut named_workspaces = HashSet::new();
    if !preferences.automatic_tab_names && !preferences.automatic_workspace_names {
        return (named_tabs, named_workspaces);
    }
    let names = snapshot
        .agents
        .iter()
        .filter_map(|agent| conversation_name(agent).map(|name| (agent, name)))
        .collect::<Vec<_>>();
    for tab in snapshot
        .tabs
        .iter_mut()
        .filter(|tab| preferences.automatic_tab_names && !tab.custom_label)
    {
        let candidates = names
            .iter()
            .filter(|(agent, _)| agent.tab_id == tab.tab_id)
            .collect::<Vec<_>>();
        let selected = candidates
            .iter()
            .find(|(agent, _)| agent.focused)
            .copied()
            .or_else(|| (candidates.len() == 1).then(|| candidates[0]));
        if let Some((_, name)) = selected {
            named_tabs.insert(tab.tab_id.clone());
            let suspended = tab.label.ends_with(" (Suspended)");
            tab.label = name.clone();
            if suspended {
                tab.label.push_str(" (Suspended)");
            }
        }
    }
    for workspace in snapshot
        .workspaces
        .iter_mut()
        .filter(|workspace| preferences.automatic_workspace_names && !workspace.custom_label)
    {
        let mut agents = snapshot
            .agents
            .iter()
            .filter(|agent| agent.workspace_id == workspace.workspace_id);
        if let Some(agent) = agents.next().filter(|_| agents.next().is_none()) {
            if let Some(name) = conversation_name(agent) {
                named_workspaces.insert(workspace.workspace_id.clone());
                workspace.label = name;
            }
        }
    }
    (named_tabs, named_workspaces)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::AgentStatus;

    #[test]
    fn suspension_preserves_custom_workspace_and_tab_names_in_composed_sidebar() {
        let mut snapshot = super::super::tests::snapshot();
        snapshot.workspaces[0].label = "My workspace".into();
        snapshot.workspaces[0].custom_label = true;
        snapshot.tabs[0].label = "My task (Suspended)".into();
        snapshot.tabs[0].custom_label = true;
        let mut config = ClientShellConfig::from_config(&Config::default());
        config
            .preferences
            .customization
            .selected_tab_name_as_workspace = true;
        config.preferences.customization.show_selected_tab_name = true;
        let mut client = ClientShellState::new(config);
        client.set_snapshot(Box::new(snapshot));
        client.set_pane_surface(super::super::tests::surface());
        let frame = client.compose(120, 40).unwrap();
        let text = frame
            .cells
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>();
        assert!(
            text.contains("My workspace"),
            "custom workspace name must not become the suspended tab label"
        );
        assert!(text.contains("My task (Suspended)"));
    }

    #[test]
    fn suspension_preserves_conversation_names_when_metadata_disappears() {
        for kind in ["claude", "codex"] {
            let mut snapshot = super::super::tests::snapshot();
            snapshot.agents.push(crate::protocol::ClientShellAgent {
                pane_id: "pane_1".into(),
                workspace_id: "ws_1".into(),
                tab_id: "tab_1".into(),
                name: None,
                display_agent: None,
                agent: Some(kind.into()),
                title: Some("Conversation name".into()),
                terminal_title: None,
                terminal_title_stripped: None,
                agent_status: AgentStatus::Idle,
                state_change_seq: 1,
                state_labels: vec![],
                tokens: vec![],
                focused: true,
            });
            let mut client =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            client.set_snapshot(Box::new(snapshot.clone()));
            let mut agent = snapshot.agents[0].clone();
            snapshot.agents.clear();
            snapshot.tabs[0].label = "1 (Suspended)".into();
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot.clone()));
            for _ in 0..2 {
                let shown = client.snapshot.as_ref().unwrap();
                assert_eq!(shown.tabs[0].label, "Conversation name (Suspended)");
                assert_eq!(shown.workspaces[0].label, "Conversation name");
                client.refresh_automatic_names();
            }
            snapshot.tabs[0].label = "1".into();
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot.clone()));
            assert_eq!(
                client.snapshot.as_ref().unwrap().tabs[0].label,
                "Conversation name"
            );
            agent.title = Some("New conversation title".into());
            snapshot.agents.push(agent);
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot.clone()));
            assert_eq!(
                client.snapshot.as_ref().unwrap().tabs[0].label,
                "New conversation title"
            );
            assert_eq!(
                client.snapshot.as_ref().unwrap().workspaces[0].label,
                "New conversation title"
            );
            snapshot.tabs[0].label = "1 (Suspended)".into();
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot.clone()));
            snapshot.tabs[0].label = "Explicit rename (Suspended)".into();
            snapshot.tabs[0].custom_label = true;
            snapshot.workspaces[0].label = "Explicit space".into();
            snapshot.workspaces[0].custom_label = true;
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot));
            assert_eq!(
                client.snapshot.as_ref().unwrap().tabs[0].label,
                "Explicit rename (Suspended)"
            );
            assert_eq!(
                client.snapshot.as_ref().unwrap().workspaces[0].label,
                "Explicit space"
            );
        }
    }

    fn snapshot_with_terminal_name(kind: &str, title: &str) -> ClientShellSnapshot {
        let mut snapshot = super::super::tests::snapshot();
        snapshot.tabs[0].label = "Default tab".into();
        snapshot.workspaces[0].label = "Default space".into();
        snapshot.agents.push(crate::protocol::ClientShellAgent {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: None,
            display_agent: None,
            agent: Some(kind.into()),
            title: None,
            terminal_title: Some(format!("⠋ {title}")),
            terminal_title_stripped: Some(title.into()),
            agent_status: AgentStatus::Working,
            state_change_seq: 1,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: true,
        });
        snapshot
    }

    fn assert_composed_names(client: &mut ClientShellState, tab: &str, workspace: &str) {
        let shown = client.snapshot.as_ref().expect("projected snapshot");
        assert_eq!(shown.tabs[0].label, tab);
        assert_eq!(shown.workspaces[0].label, workspace);
        let mut surface = super::super::tests::surface();
        surface.boot_id.clone_from(&shown.boot_id);
        surface.projection_revision = shown.revision;
        client.set_pane_surface(surface);
        let frame = client.compose(120, 40).expect("named client frame");
        let text = frame
            .cells
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>();
        assert!(text.contains(tab), "tab label missing from frame: {tab}");
        assert!(
            text.contains(workspace),
            "workspace label missing from frame: {workspace}"
        );
    }

    #[test]
    fn automatic_names_ignore_initial_working_terminal_title() {
        for kind in ["claude", "codex"] {
            let snapshot = snapshot_with_terminal_name(kind, "Working");
            let expected_tab = snapshot.tabs[0].label.clone();
            let expected_workspace = snapshot.workspaces[0].label.clone();
            let mut client =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            client.set_snapshot(Box::new(snapshot));
            assert_composed_names(&mut client, &expected_tab, &expected_workspace);
        }
    }

    #[test]
    fn automatic_names_filter_only_placeholder_terminal_titles() {
        for kind in ["claude", "codex"] {
            for title in [
                "Working",
                " ready ",
                "Thinking...",
                "Waiting…",
                "IDLE",
                "0199f2e4-1234-7000-8000-0123456789ab",
            ] {
                let mut snapshot = snapshot_with_terminal_name(kind, title);
                let agent = &mut snapshot.agents[0];
                assert_eq!(conversation_name(agent), None, "{kind}: {title}");
                agent.name = Some("Fallback task".into());
                assert_eq!(conversation_name(agent).as_deref(), Some("Fallback task"));
                agent.title = Some("Working".into());
                assert_eq!(conversation_name(agent).as_deref(), Some("Working"));
                agent.title = None;
                agent.name = Some("Ready".into());
                assert_eq!(conversation_name(agent).as_deref(), Some("Ready"));
            }
            for title in ["Working on login", "Ready for release", "Waiting queue bug"] {
                let snapshot = snapshot_with_terminal_name(kind, title);
                assert_eq!(
                    conversation_name(&snapshot.agents[0]).as_deref(),
                    Some(title)
                );
            }
        }
    }

    #[test]
    fn automatic_names_retain_missing_metadata_and_respect_preferences_and_manual_names() {
        for kind in ["claude", "codex"] {
            let mut snapshot = snapshot_with_terminal_name(kind, "Fix login");
            let original_tab = snapshot.tabs[0].label.clone();
            let original_workspace = snapshot.workspaces[0].label.clone();
            let mut client =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            client.set_snapshot(Box::new(snapshot.clone()));
            snapshot.agents.clear();
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot.clone()));
            assert_composed_names(&mut client, "Fix login", "Fix login");
            client.config.preferences.customization.automatic_tab_names = false;
            client
                .config
                .preferences
                .customization
                .automatic_workspace_names = false;
            client.refresh_automatic_names();
            assert_composed_names(&mut client, &original_tab, &original_workspace);
            client.config.preferences.customization.automatic_tab_names = true;
            client
                .config
                .preferences
                .customization
                .automatic_workspace_names = true;
            client.refresh_automatic_names();
            assert_composed_names(&mut client, "Fix login", "Fix login");
            snapshot.tabs[0].custom_label = true;
            snapshot.tabs[0].label = "Manual task".into();
            snapshot.workspaces[0].custom_label = true;
            snapshot.workspaces[0].label = "Manual space".into();
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot.clone()));
            assert_composed_names(&mut client, "Manual task", "Manual space");
            // Clearing manual names must not resurrect an obsolete retained name.
            snapshot.tabs[0].custom_label = false;
            snapshot.tabs[0].label.clone_from(&original_tab);
            snapshot.workspaces[0].custom_label = false;
            snapshot.workspaces[0].label.clone_from(&original_workspace);
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot));
            assert_composed_names(&mut client, &original_tab, &original_workspace);
        }
    }

    #[test]
    fn automatic_names_preserve_boot_changes_but_clear_positive_agent_replacement() {
        for replacement in ["boot", "pane", "kind"] {
            let mut snapshot = snapshot_with_terminal_name("codex", "Fix login");
            let original_tab = snapshot.tabs[0].label.clone();
            let original_workspace = snapshot.workspaces[0].label.clone();
            let mut client =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            client.set_snapshot(Box::new(snapshot.clone()));
            snapshot.agents[0].terminal_title = Some("Ready".into());
            snapshot.agents[0].terminal_title_stripped = Some("Ready".into());
            match replacement {
                "boot" => snapshot.boot_id = "new-boot".into(),
                "pane" => snapshot.agents[0].pane_id = "new-agent-pane".into(),
                "kind" => snapshot.agents[0].agent = Some("claude".into()),
                _ => unreachable!(),
            }
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot));
            if replacement == "boot" {
                assert_composed_names(&mut client, "Fix login", "Fix login");
            } else {
                assert_composed_names(&mut client, &original_tab, &original_workspace);
            }
        }
    }

    #[test]
    fn automatic_names_compare_last_known_owners_across_metadata_gaps() {
        for replacement in [false, true] {
            let mut snapshot = snapshot_with_terminal_name("codex", "Fix login");
            let original_tab = snapshot.tabs[0].label.clone();
            let original_workspace = snapshot.workspaces[0].label.clone();
            let mut agent = snapshot.agents[0].clone();
            let mut client =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            client.set_snapshot(Box::new(snapshot.clone()));
            snapshot.agents.clear();
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot.clone()));
            assert_composed_names(&mut client, "Fix login", "Fix login");
            agent.terminal_title = Some("Ready".into());
            agent.terminal_title_stripped = Some("Ready".into());
            if replacement {
                agent.pane_id = "replacement-pane".into();
                agent.agent = Some("claude".into());
            }
            snapshot.agents.push(agent);
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot));
            if replacement {
                assert_composed_names(&mut client, &original_tab, &original_workspace);
            } else {
                assert_composed_names(&mut client, "Fix login", "Fix login");
            }
        }
    }

    #[test]
    fn automatic_names_accept_fresh_titles_equal_to_original_labels() {
        for target in ["tab", "workspace"] {
            let mut snapshot = snapshot_with_terminal_name("codex", "Fix login");
            snapshot.tabs[0].label = "1".into();
            let fresh_title = if target == "tab" {
                snapshot.tabs[0].label.clone()
            } else {
                snapshot.workspaces[0].label.clone()
            };
            let mut client =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            client.set_snapshot(Box::new(snapshot.clone()));
            snapshot.agents[0].terminal_title_stripped = Some(fresh_title.clone());
            snapshot.revision += 1;
            client.set_snapshot(Box::new(snapshot));
            let shown = client.snapshot.as_ref().expect("fresh title snapshot");
            assert_eq!(shown.tabs[0].label, fresh_title);
            assert_eq!(shown.workspaces[0].label, fresh_title);
        }
    }

    #[test]
    fn automatic_names_retain_task_during_working_and_accept_next_task() {
        for kind in ["claude", "codex"] {
            let mut snapshot = snapshot_with_terminal_name(kind, "Fix login");
            let mut client =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            client.set_snapshot(Box::new(snapshot.clone()));
            assert_composed_names(&mut client, "Fix login", "Fix login");
            for (title, expected) in [("Working", "Fix login"), ("Add tests", "Add tests")] {
                snapshot.agents[0].terminal_title = Some(format!("⠋ {title}"));
                snapshot.agents[0].terminal_title_stripped = Some(title.into());
                snapshot.revision += 1;
                client.set_snapshot(Box::new(snapshot.clone()));
                assert_composed_names(&mut client, expected, expected);
                client.refresh_automatic_names();
                assert_composed_names(&mut client, expected, expected);
            }
        }
    }

    #[test]
    fn automatic_names_follow_conversation_titles_and_preserve_manual_labels() {
        for kind in ["claude", "codex"] {
            let mut snapshot = super::super::tests::snapshot();
            snapshot.agents.push(crate::protocol::ClientShellAgent {
                pane_id: "pane_1".into(),
                workspace_id: "ws_1".into(),
                tab_id: "tab_1".into(),
                name: None,
                display_agent: None,
                agent: Some(kind.into()),
                title: None,
                terminal_title: Some("⠋ Fix login".into()),
                terminal_title_stripped: Some("Fix login".into()),
                agent_status: AgentStatus::Working,
                state_change_seq: 1,
                state_labels: Vec::new(),
                tokens: Vec::new(),
                focused: true,
            });
            let original = snapshot.clone();
            let mut client =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            client.set_snapshot(Box::new(original.clone()));
            for (tabs, spaces) in [(false, true), (true, false), (false, false), (true, true)] {
                client.config.preferences.customization.automatic_tab_names = tabs;
                client
                    .config
                    .preferences
                    .customization
                    .automatic_workspace_names = spaces;
                client.refresh_automatic_names();
                let shown = client.snapshot.as_ref().expect("active snapshot");
                assert_eq!(
                    shown.tabs[0].label,
                    if tabs {
                        "Fix login"
                    } else {
                        &original.tabs[0].label
                    }
                );
                assert_eq!(
                    shown.workspaces[0].label,
                    if spaces {
                        "Fix login"
                    } else {
                        &original.workspaces[0].label
                    }
                );
            }
            apply(
                &mut snapshot,
                &preferences::CustomizationPreferences::default(),
            );
            assert_eq!(snapshot.tabs[0].label, "Fix login");
            assert_eq!(snapshot.workspaces[0].label, "Fix login");
            let mut client =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            client.set_snapshot(Box::new(snapshot.clone()));
            client.set_pane_surface(super::super::tests::surface());
            let frame = client.compose(120, 40).expect("named client frame");
            let text = frame
                .cells
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>();
            assert!(text.contains("Fix login"));
            snapshot.tabs[0].label = "1 (Suspended)".into();
            apply(
                &mut snapshot,
                &preferences::CustomizationPreferences::default(),
            );
            assert_eq!(snapshot.tabs[0].label, "Fix login (Suspended)");
            snapshot.tabs[0].custom_label = true;
            snapshot.tabs[0].label = "My tab".into();
            snapshot.workspaces[0].custom_label = true;
            snapshot.workspaces[0].label = "My space".into();
            snapshot.agents[0].terminal_title_stripped = Some("New task".into());
            apply(
                &mut snapshot,
                &preferences::CustomizationPreferences::default(),
            );
            assert_eq!(snapshot.tabs[0].label, "My tab");
            assert_eq!(snapshot.workspaces[0].label, "My space");
            snapshot.tabs[0].custom_label = false;
            snapshot.tabs[0].label = "1".into();
            snapshot.workspaces[0].custom_label = false;
            snapshot.workspaces[0].label = "project".into();
            snapshot.agents[0].terminal_title_stripped = Some(kind.into());
            apply(
                &mut snapshot,
                &preferences::CustomizationPreferences::default(),
            );
            assert_eq!(snapshot.tabs[0].label, "1");
            assert_eq!(snapshot.workspaces[0].label, "project");
        }
    }
}
