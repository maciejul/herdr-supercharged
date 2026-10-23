//! Derive display labels from existing agent metadata without renaming server IDs.
use super::*;

#[derive(Clone, Debug, Default)]
pub(super) struct OriginalNames {
    tabs: HashMap<String, String>,
    workspaces: HashMap<String, String>,
    retained_tabs: HashMap<String, String>,
    retained_workspaces: HashMap<String, String>,
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

    /// Keep the last visible automatic name while a session is suspended or its
    /// resume metadata is still arriving. Explicit names always remain authoritative.
    pub(super) fn project(
        &mut self,
        snapshot: &mut ClientShellSnapshot,
        previous: Option<&ClientShellSnapshot>,
        preferences: &preferences::CustomizationPreferences,
    ) {
        let originals = Self::capture(snapshot);
        apply(snapshot, preferences);
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
            } else if originals.tabs.get(&tab.tab_id) != Some(&tab.label) {
                // Fresh conversation metadata has supplied a name after resume.
                self.retained_tabs.remove(&tab.tab_id);
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
            } else if originals.workspaces.get(&workspace.workspace_id) != Some(&workspace.label) {
                self.retained_workspaces.remove(&workspace.workspace_id);
            }
            if let Some(name) = self.retained_workspaces.get(&workspace.workspace_id) {
                workspace.label.clone_from(name);
            }
        }
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

fn conversation_name(agent: &crate::protocol::ClientShellAgent) -> Option<String> {
    let kind = agent.agent.as_deref()?;
    if !matches!(kind, "claude" | "codex") {
        return None;
    }
    [
        agent.title.as_deref(),
        agent.terminal_title_stripped.as_deref(),
        agent.name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|name| {
        !name.is_empty()
            && !name.eq_ignore_ascii_case(kind)
            && !matches!(
                name.to_ascii_lowercase().as_str(),
                "claude code" | "openai codex" | "codex cli"
            )
    })
    .map(|name| name.chars().filter(|c| !c.is_control()).take(120).collect())
}

pub(super) fn apply(
    snapshot: &mut ClientShellSnapshot,
    preferences: &preferences::CustomizationPreferences,
) {
    if !preferences.automatic_tab_names && !preferences.automatic_workspace_names {
        return;
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
                workspace.label = name;
            }
        }
    }
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
