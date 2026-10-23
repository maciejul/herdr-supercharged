//! Personal Space organization, persisted with client chrome rather than server state.
use super::*;

pub(super) fn endpoint_key(endpoint: &ClientEndpointId) -> String {
    match endpoint {
        ClientEndpointId::Local => "local".into(),
        ClientEndpointId::Ssh(profile) => format!("ssh:{profile}"),
    }
}

pub(super) enum GroupedSpaceRow {
    Group(usize),
    Ungrouped,
    Workspace(WorkspaceEntry),
}

pub(super) fn grouped_rows(
    snapshot: &ClientShellSnapshot,
    entries: Vec<WorkspaceEntry>,
    endpoint: &ClientEndpointId,
    groups: &[preferences::NamedSpaceGroup],
) -> Vec<GroupedSpaceRow> {
    let key = endpoint_key(endpoint);
    let applicable = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.endpoint == key)
        .collect::<Vec<_>>();
    if applicable.is_empty() {
        return entries
            .into_iter()
            .map(GroupedSpaceRow::Workspace)
            .collect();
    }
    let mut emitted = HashSet::new();
    let mut rows = Vec::new();
    for (index, group) in applicable {
        rows.push(GroupedSpaceRow::Group(index));
        for entry in &entries {
            let workspace = &snapshot.workspaces[entry.index];
            if group.workspace_ids.contains(&workspace.workspace_id) && emitted.insert(entry.index)
            {
                // Keep the active Space reachable even inside a collapsed group.
                if !group.collapsed || workspace.focused {
                    rows.push(GroupedSpaceRow::Workspace(*entry));
                }
            }
        }
    }
    let ungrouped = entries
        .into_iter()
        .filter(|entry| !emitted.contains(&entry.index))
        .collect::<Vec<_>>();
    if !ungrouped.is_empty() {
        rows.push(GroupedSpaceRow::Ungrouped);
        rows.extend(ungrouped.into_iter().map(GroupedSpaceRow::Workspace));
    }
    rows
}

pub(super) fn render_create_button(
    buffer: &mut Buffer,
    area: Rect,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    if !config.preferences.customization.space_groups_enabled || area.width < 18 || area.height == 0
    {
        return;
    }
    let rect = Rect::new(area.right().saturating_sub(9), area.y, 8, 1);
    render::put_text(
        buffer,
        rect.x,
        rect.y,
        rect.width,
        "+ group",
        Style::default().fg(config.palette.accent),
    );
    hits.new_space_group = rect;
}

impl ClientShellState {
    pub(super) fn space_group_drop_target_at(
        &self,
        endpoint: &ClientEndpointId,
        point: (u16, u16),
    ) -> Option<usize> {
        if !self.config.preferences.customization.space_groups_enabled {
            return None;
        }
        let key = endpoint_key(endpoint);
        self.hits.space_groups.iter().find_map(|(rect, index)| {
            (contains(*rect, point)
                && self
                    .config
                    .preferences
                    .space_groups
                    .get(*index)
                    .is_some_and(|group| group.endpoint == key))
            .then_some(*index)
        })
    }

    pub(super) fn assign_workspace_to_group(
        &mut self,
        endpoint: &ClientEndpointId,
        workspace: &str,
        index: usize,
        outcome: &mut ClientShellInput,
    ) {
        if !self.config.preferences.customization.space_groups_enabled {
            return;
        }
        let snapshot = if endpoint == &self.active_endpoint_id {
            self.snapshot.as_deref()
        } else {
            self.endpoints
                .iter()
                .find(|candidate| &candidate.endpoint_id == endpoint)
                .and_then(|candidate| candidate.snapshot.as_deref())
        };
        if !snapshot.is_some_and(|snapshot| {
            snapshot
                .workspaces
                .iter()
                .any(|candidate| candidate.workspace_id == workspace)
        }) {
            return;
        }
        let key = endpoint_key(endpoint);
        if self
            .config
            .preferences
            .space_groups
            .get(index)
            .is_none_or(|group| group.endpoint != key)
        {
            return;
        }
        for group in self
            .config
            .preferences
            .space_groups
            .iter_mut()
            .filter(|group| group.endpoint == key)
        {
            group.workspace_ids.retain(|member| member != workspace);
        }
        let group = &mut self.config.preferences.space_groups[index];
        group.workspace_ids.push(workspace.to_owned());
        group.collapsed = false;
        self.persist_chrome_preferences(outcome);
        outcome.repaint = true;
    }

    pub(super) fn highlight_space_group_drop(&self, frame: &mut FrameData) {
        if self.overlay.is_some() {
            return;
        }
        let Some(ClientChromeDrag::SpaceGroup {
            target: Some(index),
            ..
        }) = &self.chrome_drag
        else {
            return;
        };
        let Some((rect, _)) = self
            .hits
            .space_groups
            .iter()
            .find(|(_, group)| group == index)
        else {
            return;
        };
        let color = crate::protocol::color_to_u32(self.config.palette.surface1);
        for y in rect.y..rect.bottom().min(frame.height) {
            for x in rect.x..rect.right().min(frame.width) {
                frame.cells[usize::from(y) * usize::from(frame.width) + usize::from(x)].bg = color;
            }
        }
    }

    pub(super) fn new_workspace_in_group(&mut self, index: usize, outcome: &mut ClientShellInput) {
        let Some(group) = self.config.preferences.space_groups.get(index).cloned() else {
            return;
        };
        if group.endpoint != endpoint_key(&self.active_endpoint_id) {
            self.set_endpoint_error(
                "Select a Space on this group's machine before creating a workspace.",
            );
            return;
        }
        let source = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| {
                snapshot
                    .workspaces
                    .iter()
                    .filter(|workspace| group.workspace_ids.contains(&workspace.workspace_id))
                    .max_by_key(|workspace| workspace.focused)
                    .map(|workspace| workspace.workspace_id.clone())
            })
            .or_else(|| self.workspace_action_id());
        if self.config.prompt_new_workspace_name {
            self.open_new_workspace_overlay_from(source);
            if let Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                target: ClientRenameTarget::NewWorkspace { space_group, .. },
                ..
            })) = self.overlay.as_mut()
            {
                *space_group = Some((group.endpoint, group.name));
            }
        } else {
            let method = crate::api::schema::Method::WorkspaceCreate(
                crate::api::schema::WorkspaceCreateParams {
                    source_workspace_id: source,
                    cwd: None,
                    focus: true,
                    label: None,
                    env: Default::default(),
                },
            );
            self.submit_group_workspace(group.endpoint, group.name, method, outcome);
        }
    }

    pub(super) fn submit_group_workspace(
        &mut self,
        endpoint: String,
        group_name: String,
        method: crate::api::schema::Method,
        outcome: &mut ClientShellInput,
    ) {
        if endpoint != endpoint_key(&self.active_endpoint_id)
            || !self
                .config
                .preferences
                .space_groups
                .iter()
                .any(|group| group.endpoint == endpoint && group.name == group_name)
        {
            self.set_endpoint_error("The target group or active machine changed. Create the workspace again from its group.");
            outcome.repaint = true;
            return;
        }
        self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::WorkspaceCreateInGroup {
                endpoint,
                group_name,
            },
            outcome,
        );
    }

    pub(super) fn assign_created_group_workspace(
        &mut self,
        endpoint: &str,
        group_name: &str,
        workspace_id: String,
        outcome: &mut ClientShellInput,
    ) {
        let Some(index) = self
            .config
            .preferences
            .space_groups
            .iter()
            .position(|group| group.endpoint == endpoint && group.name == group_name)
        else {
            self.set_endpoint_error("Workspace created, but its group was renamed or removed. Use Move to group to assign it.");
            return;
        };
        for group in self
            .config
            .preferences
            .space_groups
            .iter_mut()
            .filter(|group| group.endpoint == endpoint)
        {
            group.workspace_ids.retain(|id| id != &workspace_id);
        }
        let group = &mut self.config.preferences.space_groups[index];
        group.workspace_ids.push(workspace_id);
        group.collapsed = false;
        self.reveal_focused_workspace = true;
        self.persist_chrome_preferences(outcome);
        outcome.repaint = true;
    }

    pub(super) fn open_space_group_editor(
        &mut self,
        workspace_id: Option<String>,
        group_index: Option<usize>,
    ) {
        let key = endpoint_key(&self.active_endpoint_id);
        let existing = group_index
            .and_then(|index| self.config.preferences.space_groups.get(index))
            .or_else(|| {
                workspace_id.as_ref().and_then(|id| {
                    self.config
                        .preferences
                        .space_groups
                        .iter()
                        .find(|group| group.endpoint == key && group.workspace_ids.contains(id))
                })
            });
        let title = if group_index.is_some() {
            "rename group"
        } else if workspace_id.is_some() {
            "group name (blank = ungroup)"
        } else {
            "new space group"
        };
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title,
            input: TextEditor::new(existing.map_or("", |group| group.name.as_str()), false),
            target: ClientRenameTarget::SpaceGroup {
                choices: if workspace_id.is_some() && group_index.is_none() {
                    self.config
                        .preferences
                        .space_groups
                        .iter()
                        .filter(|group| group.endpoint == key)
                        .map(|group| group.name.clone())
                        .collect()
                } else {
                    Vec::new()
                },
                endpoint: key,
                workspace_id,
                group_index,
            },
        }));
    }

    pub(super) fn save_space_group(
        &mut self,
        workspace_id: Option<String>,
        group_index: Option<usize>,
        name: &str,
        outcome: &mut ClientShellInput,
    ) {
        let name = name.trim();
        if name.chars().count() > 60 || name.chars().any(char::is_control) {
            self.set_endpoint_error(
                "Group names must be at most 60 characters without control characters.",
            );
            outcome.repaint = true;
            return;
        }
        let key = endpoint_key(&self.active_endpoint_id);
        let groups = &mut self.config.preferences.space_groups;
        if let Some(index) = group_index {
            let Some(group) = groups.get(index) else {
                return;
            };
            if name.is_empty() {
                return;
            }
            if groups.iter().enumerate().any(|(other, candidate)| {
                other != index && candidate.endpoint == group.endpoint && candidate.name == name
            }) {
                self.set_endpoint_error("A group with that name already exists.");
                outcome.repaint = true;
                return;
            }
            groups[index].name = name.to_owned();
        } else {
            if let Some(id) = &workspace_id {
                for group in groups.iter_mut().filter(|group| group.endpoint == key) {
                    group.workspace_ids.retain(|member| member != id);
                }
            }
            if !name.is_empty() {
                let index = groups
                    .iter()
                    .position(|group| group.endpoint == key && group.name == name)
                    .unwrap_or_else(|| {
                        groups.push(preferences::NamedSpaceGroup {
                            endpoint: key,
                            name: name.to_owned(),
                            ..Default::default()
                        });
                        groups.len() - 1
                    });
                if let Some(id) = workspace_id {
                    groups[index].workspace_ids.push(id);
                }
            }
        }
        self.workspace_scroll = 0;
        self.persist_chrome_preferences(outcome);
        outcome.repaint = true;
    }

    pub(super) fn handle_space_group_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.config.preferences.customization.space_groups_enabled {
            return false;
        }
        use crossterm::event::{MouseButton, MouseEventKind};
        let point = (mouse.column, mouse.row);
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && contains(self.hits.new_space_group, point)
        {
            self.open_space_group_editor(None, None);
            outcome.repaint = true;
            return true;
        }
        let Some(index) = self
            .hits
            .space_groups
            .iter()
            .find_map(|(rect, index)| contains(*rect, point).then_some(*index))
        else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(group) = self.config.preferences.space_groups.get_mut(index) {
                    group.collapsed = !group.collapsed;
                }
                self.persist_chrome_preferences(outcome);
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
                    target: ClientContextMenuTarget::SpaceGroup { index },
                    x: mouse.column,
                    y: mouse.row,
                    highlighted: 0,
                }));
            }
            _ => return false,
        }
        outcome.repaint = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    #[test]
    fn workspace_activity_highlight_is_independent_of_details_and_preserves_attention_status() {
        use crate::api::schema::AgentStatus;
        let mut config = ClientShellConfig::from_config(&Config::default());
        let activity = super::super::background_activity::BackgroundActivity::from_tokens(&[(
            "herdr_background_activity".into(),
            r#"{"items":[{"id":"child","kind":"subagent","label":"review"}]}"#.into(),
        )]);
        config.preferences.customization.show_workspace_activity = false;
        assert_eq!(
            super::super::render::sidebar::workspace_activity_lines(&activity, true, &config),
            0
        );
        for status in [AgentStatus::Idle, AgentStatus::Done, AgentStatus::Unknown] {
            let style = super::super::render::sidebar::workspace_activity_icon_style(
                status, &activity, &config,
            );
            assert_eq!(
                style.fg,
                Some(
                    config
                        .preferences
                        .customization
                        .colors
                        .workspace_activity
                        .color()
                )
            );
            assert!(
                !style.add_modifier.contains(Modifier::BOLD),
                "running activity must not pulse like unread completion"
            );
        }
        for status in [AgentStatus::Working, AgentStatus::Blocked] {
            assert_eq!(
                super::super::render::sidebar::workspace_activity_icon_style(
                    status, &activity, &config
                ),
                config.status_icon_style(status)
            );
        }
        config
            .preferences
            .customization
            .highlight_workspace_activity = false;
        assert_eq!(
            super::super::render::sidebar::workspace_activity_icon_style(
                AgentStatus::Idle,
                &activity,
                &config
            ),
            config.status_icon_style(AgentStatus::Idle)
        );
    }

    #[test]
    fn workspace_activity_aggregates_tabs_and_expands_without_focusing_space() {
        for grouped in [false, true] {
            let mut snapshot = super::super::tests::snapshot();
            for (pane, tab, kind, label) in [
                ("pane_1", "tab_1", "subagent", "review child"),
                ("pane_2", "tab_2", "task", "background build"),
            ] {
                snapshot.agents.push(crate::protocol::ClientShellAgent {
                    pane_id: pane.into(),
                    workspace_id: "ws_1".into(),
                    tab_id: tab.into(),
                    name: Some("Parent".into()),
                    display_agent: None,
                    agent: Some("Claude".into()),
                    title: None,
                    terminal_title: None,
                    terminal_title_stripped: None,
                    agent_status: crate::api::schema::AgentStatus::Working,
                    state_change_seq: 0,
                    state_labels: vec![],
                    focused: false,
                    tokens: vec![(
                        "herdr_background_activity".into(),
                        serde_json::json!({"items":[{"id":"same", "kind":kind,"label":label}]})
                            .to_string(),
                    )],
                });
            }
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            // The default 26-column sidebar clips a 16-character label after
            // nested tree/dot prefixes. Give this full-label assertion room.
            state.sidebar_width = 36;
            state.set_snapshot(Box::new(snapshot));
            state.set_pane_surface(super::super::tests::surface());
            if grouped {
                state.save_space_group(
                    Some("ws_1".into()),
                    None,
                    "Work",
                    &mut ClientShellInput::default(),
                );
            }
            let frame = state.compose(120, 50).expect("activity frame").frame;
            let key = super::super::render::sidebar::workspace_activity_key("ws_1");
            let summary = state
                .hits
                .background_activity
                .iter()
                .find(|(_, _, id)| id == &key)
                .expect("workspace summary")
                .0;
            let parent = state
                .hits
                .workspaces
                .iter()
                .find(|hit| hit.workspace_id == "ws_1")
                .expect("parent")
                .rect;
            assert_eq!(summary.y, parent.bottom());
            assert!(!contains(parent, (summary.x + 1, summary.y)));
            let text = frame
                .cells
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>();
            assert!(text.contains("1 subagent running"));
            let outcome = drag_mouse(&mut state, MouseEventKind::Down(MouseButton::Left), summary);
            assert!(outcome.actions.is_empty());
            assert!(state
                .expanded_background_activity
                .contains(&(ClientEndpointId::Local, key)));
            let frame = state.compose(120, 50).expect("expanded frame").frame;
            let expanded_summary = state
                .hits
                .background_activity
                .iter()
                .find(|(_, _, id)| id == "workspace:ws_1")
                .expect("expanded summary")
                .0;
            let child_text = |row: u16| {
                (expanded_summary.x..expanded_summary.right())
                    .map(|x| {
                        frame.cells[usize::from(row) * usize::from(frame.width) + usize::from(x)]
                            .symbol
                            .as_str()
                    })
                    .collect::<String>()
            };
            assert!(child_text(expanded_summary.y + 1).contains("review child"));
            assert!(child_text(expanded_summary.y + 2).contains("background build"));
            assert!(state.workspace_press.is_none());
        }
    }

    #[test]
    fn group_picker_arrows_select_existing_groups_and_typing_creates_new() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(super::super::tests::snapshot()));
        state.set_pane_surface(super::super::tests::surface());
        for name in ["Work", "Personal"] {
            state.save_space_group(None, None, name, &mut ClientShellInput::default());
        }
        state.open_space_group_editor(Some("ws_1".into()), None);
        state.handle_input_bytes(b"\x1b[B");
        state.handle_input_bytes(b"\x1b[B");
        state.handle_input_bytes(b"\x1b[A");
        state.compose(100, 40).expect("picker frame");
        let outcome = state.handle_input_bytes(b"\r");
        assert!(outcome.actions.is_empty());
        assert_eq!(
            state.config.preferences.space_groups[0].workspace_ids,
            ["ws_1"]
        );
        assert_eq!(state.config.preferences.space_groups.len(), 2);
        state.open_space_group_editor(Some("ws_1".into()), None);
        state.handle_input_bytes(b"\x03");
        state.handle_input_bytes(b"Research");
        state.handle_input_bytes(b"\r");
        assert_eq!(
            state.config.preferences.space_groups[2].workspace_ids,
            ["ws_1"]
        );
        assert!(state.config.preferences.space_groups[0]
            .workspace_ids
            .is_empty());
        state.open_space_group_editor(Some("ws_1".into()), None);
        state.handle_input_bytes(b"\x1b[B");
        state.handle_input_bytes(b"\x1b");
        assert_eq!(
            state.config.preferences.space_groups[2].workspace_ids,
            ["ws_1"]
        );
    }

    fn drag_mouse(
        state: &mut ClientShellState,
        kind: MouseEventKind,
        rect: Rect,
    ) -> ClientShellInput {
        state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Mouse(MouseEvent {
            kind,
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::empty(),
        })])
    }

    #[test]
    fn dragging_workspace_into_local_or_remote_group_is_client_only() {
        for remote in [false, true] {
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state.set_snapshot(Box::new(super::super::tests::snapshot()));
            state.set_pane_surface(super::super::tests::surface());
            let endpoint = if remote {
                let id = ClientEndpointId::Ssh(
                    crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                        .expect("profile"),
                );
                let mut entry = state.endpoints[0].clone();
                entry.endpoint_id = id.clone();
                entry.label = "Remote".into();
                entry.snapshot = Some(Box::new(super::super::tests::snapshot()));
                state.endpoints.push(entry);
                id
            } else {
                ClientEndpointId::Local
            };
            state
                .config
                .preferences
                .space_groups
                .push(preferences::NamedSpaceGroup {
                    endpoint: endpoint_key(&endpoint),
                    name: "Target".into(),
                    collapsed: true,
                    ..Default::default()
                });
            state.compose(100, 40).expect("groups frame");
            let workspace = state
                .hits
                .workspaces
                .iter()
                .find(|hit| hit.endpoint_id == endpoint && hit.workspace_id == "ws_1")
                .expect("workspace")
                .rect;
            let group = state.hits.space_groups[0].0;
            assert!(drag_mouse(
                &mut state,
                MouseEventKind::Down(MouseButton::Left),
                workspace
            )
            .actions
            .is_empty());
            assert!(
                drag_mouse(&mut state, MouseEventKind::Drag(MouseButton::Left), group)
                    .actions
                    .is_empty()
            );
            assert!(matches!(
                state.chrome_drag,
                Some(ClientChromeDrag::SpaceGroup {
                    target: Some(0),
                    ..
                })
            ));
            state.compose(100, 40).expect("drop highlight");
            let outcome = drag_mouse(&mut state, MouseEventKind::Up(MouseButton::Left), group);
            assert!(
                outcome.actions.is_empty(),
                "group assignment cannot call workspace move/focus APIs"
            );
            assert_eq!(
                state.config.preferences.space_groups[0].workspace_ids,
                ["ws_1"]
            );
            assert!(!state.config.preferences.space_groups[0].collapsed);
            assert!(state.chrome_drag.is_none());
            assert!(state.workspace_press.is_none());
        }
    }

    #[test]
    fn group_drop_rejects_other_machine_missing_workspace_and_outside_headers() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(super::super::tests::snapshot()));
        state.set_pane_surface(super::super::tests::surface());
        state.save_space_group(None, None, "Local", &mut ClientShellInput::default());
        let remote = ClientEndpointId::Ssh(
            crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                .expect("profile"),
        );
        state
            .config
            .preferences
            .space_groups
            .push(preferences::NamedSpaceGroup {
                endpoint: endpoint_key(&remote),
                name: "Remote".into(),
                ..Default::default()
            });
        let mut outcome = ClientShellInput::default();
        state.assign_workspace_to_group(&ClientEndpointId::Local, "ws_1", 1, &mut outcome);
        state.assign_workspace_to_group(&ClientEndpointId::Local, "missing", 0, &mut outcome);
        assert!(state
            .config
            .preferences
            .space_groups
            .iter()
            .all(|group| group.workspace_ids.is_empty()));
        state.compose(100, 40).expect("frame");
        let workspace = state.hits.workspaces[0].rect;
        let group = state.hits.space_groups[0].0;
        assert_eq!(
            state.space_group_drop_target_at(&remote, (group.x + 1, group.y)),
            None
        );
        drag_mouse(
            &mut state,
            MouseEventKind::Down(MouseButton::Left),
            workspace,
        );
        drag_mouse(&mut state, MouseEventKind::Drag(MouseButton::Left), group);
        let released = drag_mouse(
            &mut state,
            MouseEventKind::Up(MouseButton::Left),
            Rect::new(90, 35, 2, 1),
        );
        assert!(released.actions.is_empty());
        assert!(state.config.preferences.space_groups[0]
            .workspace_ids
            .is_empty());
        assert!(state.chrome_drag.is_none());
        assert!(state.workspace_press.is_none());
    }

    #[test]
    fn disabled_groups_restore_flat_spaces_and_preserve_membership() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(super::super::tests::snapshot()));
        state.set_pane_surface(super::super::tests::surface());
        state.save_space_group(
            Some("ws_1".into()),
            None,
            "Work",
            &mut ClientShellInput::default(),
        );
        state.config.preferences.customization.space_groups_enabled = false;
        state.compose(100, 40).expect("flat frame");
        assert!(state.hits.space_groups.is_empty());
        assert!(state.hits.new_space_group.is_empty());
        assert_eq!(
            state.config.preferences.space_groups[0].workspace_ids,
            ["ws_1"]
        );
        state.open_workspace_context_menu("ws_1".into(), 1, 1);
        let Some(ClientShellOverlay::ContextMenu(menu)) = &state.overlay else {
            panic!("menu")
        };
        assert!(!menu
            .items(false)
            .iter()
            .any(|item| item.action == ClientContextMenuAction::MoveToSpaceGroup));
        state.overlay = None;
        state.config.preferences.customization.space_groups_enabled = true;
        state.compose(100, 40).expect("group frame");
        assert_eq!(state.hits.space_groups.len(), 1);
    }

    #[test]
    fn selected_tab_display_modes_preserve_workspace_identity_and_measure_rows() {
        for grouped in [false, true] {
            for below in [false, true] {
                for replace in [false, true] {
                    let mut snapshot = super::super::tests::snapshot();
                    snapshot.workspaces[0].label = "Original workspace".into();
                    snapshot.workspaces[0].custom_label = true;
                    snapshot.tabs[0].label = "Selected session".into();
                    snapshot.tabs[0].custom_label = true;
                    let mut config = ClientShellConfig::from_config(&Config::default());
                    config.preferences.customization.show_selected_tab_name = below;
                    config
                        .preferences
                        .customization
                        .selected_tab_name_as_workspace = replace;
                    let rows = super::super::render::sidebar::customized_workspace_rows(
                        &snapshot,
                        &snapshot.workspaces[0],
                        snapshot.workspaces[0].agent_status,
                        false,
                        &config,
                    );
                    // Explicit workspace labels take precedence over the optional tab substitution.
                    let expected = "Original workspace";
                    assert!(rows.iter().flatten().any(|token| matches!(&token.kind, crate::ui::ResolvedTokenKind::Workspace(label) if label == expected)));
                    assert_eq!(rows.iter().flatten().any(|token| matches!(&token.kind, crate::ui::ResolvedTokenKind::Tab(label) if label == "Selected session")), below);
                    let mut state = ClientShellState::new(config);
                    state.set_snapshot(Box::new(snapshot));
                    state.set_pane_surface(super::super::tests::surface());
                    if grouped {
                        state.save_space_group(
                            Some("ws_1".into()),
                            None,
                            "Work",
                            &mut ClientShellInput::default(),
                        );
                    }
                    state.compose(100, 40).expect("workspace frame");
                    let hit = state
                        .hits
                        .workspaces
                        .iter()
                        .find(|hit| hit.workspace_id == "ws_1")
                        .expect("workspace hit");
                    assert_eq!(usize::from(hit.rect.height), rows.len());
                    assert_eq!(
                        state.snapshot.as_ref().expect("snapshot").workspaces[0].label,
                        "Original workspace"
                    );
                }
            }
        }
    }

    #[test]
    fn selected_tab_missing_keeps_workspace_label_and_height() {
        let mut snapshot = super::super::tests::snapshot();
        snapshot.tabs.clear();
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.preferences.customization.show_selected_tab_name = true;
        config
            .preferences
            .customization
            .selected_tab_name_as_workspace = true;
        let workspace = &snapshot.workspaces[0];
        assert_eq!(
            super::super::render::sidebar::customized_workspace_rows(
                &snapshot,
                workspace,
                workspace.agent_status,
                false,
                &config
            ),
            super::super::render::sidebar::workspace_rows(
                workspace,
                workspace.agent_status,
                false,
                &config.spaces
            )
        );
    }

    fn created_workspace_result() -> crate::api::schema::ResponseResult {
        serde_json::from_value(serde_json::json!({
            "type": "workspace_created",
            "workspace": {"workspace_id":"new-space", "number":2, "label":"new", "focused":true,
                "pane_count":1, "tab_count":1, "active_tab_id":"new-tab", "agent_status":"idle"},
            "tab": {"tab_id":"new-tab", "workspace_id":"new-space", "number":1, "label":"1",
                "focused":true, "pane_count":1, "agent_status":"idle"},
            "root_pane": {"pane_id":"new-pane", "terminal_id":"new-terminal", "workspace_id":"new-space",
                "tab_id":"new-tab", "focused":true, "agent_status":"idle", "revision":1}
        })).expect("workspace creation response")
    }

    #[test]
    fn group_menu_creates_and_assigns_workspace_only_after_success() {
        for prompt in [false, true] {
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state.config.prompt_new_workspace_name = prompt;
            state.set_snapshot(Box::new(super::super::tests::snapshot()));
            let mut outcome = ClientShellInput::default();
            state.save_space_group(None, None, "Work", &mut outcome);
            state.config.preferences.space_groups[0].collapsed = true;
            state.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
                target: ClientContextMenuTarget::SpaceGroup { index: 0 },
                x: 0,
                y: 0,
                highlighted: 0,
            }));
            let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
                panic!("menu");
            };
            let item = menu
                .items(true)
                .iter()
                .position(|item| item.action == ClientContextMenuAction::NewWorkspaceInGroup)
                .expect("New workspace item");
            state.activate_context_menu_item(item, &mut outcome);
            if prompt {
                outcome = state.handle_input_bytes(b"\r");
            }
            let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
                panic!("creation request");
            };
            assert!(matches!(
                request.method,
                crate::api::schema::Method::WorkspaceCreate(_)
            ));
            assert!(state.config.preferences.space_groups[0]
                .workspace_ids
                .is_empty());
            let request_id = request.id.clone();
            state.handle_endpoint_result("boot-1", &request_id, Ok(created_workspace_result()));
            assert_eq!(
                state.config.preferences.space_groups[0].workspace_ids,
                ["new-space"]
            );
            assert!(!state.config.preferences.space_groups[0].collapsed);
            let saved: preferences::ClientChromePreferences = serde_json::from_str(
                &serde_json::to_string(&state.config.preferences).expect("serialize"),
            )
            .expect("reload");
            assert_eq!(saved.space_groups[0].workspace_ids, ["new-space"]);
        }
    }

    #[test]
    fn group_creation_failure_and_removed_group_do_not_assign_another_group() {
        for remove_group in [false, true] {
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state.config.prompt_new_workspace_name = false;
            state.set_snapshot(Box::new(super::super::tests::snapshot()));
            let mut outcome = ClientShellInput::default();
            state.save_space_group(None, None, "Work", &mut outcome);
            state.new_workspace_in_group(0, &mut outcome);
            let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
                panic!("request");
            };
            let request_id = request.id.clone();
            let response = if remove_group {
                state.config.preferences.space_groups.clear();
                state.save_space_group(None, None, "Other", &mut ClientShellInput::default());
                Ok(created_workspace_result())
            } else {
                Err(ClientShellEndpointError {
                    code: Some("failed".into()),
                    message: "creation failed".into(),
                })
            };
            state.handle_endpoint_result("boot-1", &request_id, response);
            assert!(state.config.preferences.space_groups[0]
                .workspace_ids
                .is_empty());
        }
    }

    #[test]
    fn cancelling_group_creation_does_not_affect_normal_new_workspace() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(super::super::tests::snapshot()));
        state.config.prompt_new_workspace_name = true;
        let mut outcome = ClientShellInput::default();
        state.save_space_group(None, None, "Work", &mut outcome);
        state.new_workspace_in_group(0, &mut outcome);
        state.handle_input_bytes(b"\x1b");
        state.config.prompt_new_workspace_name = false;
        state.record_binding(
            crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorkspace),
            &mut outcome,
        );
        let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
            panic!("normal creation");
        };
        let request_id = request.id.clone();
        state.handle_endpoint_result("boot-1", &request_id, Ok(created_workspace_result()));
        assert!(state.config.preferences.space_groups[0]
            .workspace_ids
            .is_empty());
    }

    #[test]
    fn space_groups_create_move_rename_and_reload_without_crossing_endpoints() {
        let path =
            std::env::temp_dir().join(format!("herdr-space-groups-{}.json", std::process::id()));
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.preferences_path = Some(path.clone());
        let mut state = ClientShellState::new(config);
        let mut outcome = ClientShellInput::default();
        state.save_space_group(None, None, "Work", &mut outcome);
        state.save_space_group(Some("ws_1".into()), None, "Work", &mut outcome);
        state.save_space_group(Some("ws_2".into()), None, "Work", &mut outcome);
        assert_eq!(
            state.config.preferences.space_groups[0].workspace_ids,
            ["ws_1", "ws_2"]
        );
        state.save_space_group(Some("ws_2".into()), None, "Personal", &mut outcome);
        assert_eq!(
            state.config.preferences.space_groups[0].workspace_ids,
            ["ws_1"]
        );
        state.save_space_group(None, Some(1), "Research", &mut outcome);
        state.config.preferences.space_groups[1].collapsed = true;
        state.persist_chrome_preferences(&mut outcome);
        let saved = preferences::load(&path).expect("saved groups");
        assert_eq!(saved.space_groups[1].name, "Research");
        assert_eq!(saved.space_groups[1].workspace_ids, ["ws_2"]);
        assert!(saved.space_groups[1].collapsed);
        state.active_endpoint_id = ClientEndpointId::Ssh(
            crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                .expect("profile"),
        );
        state.save_space_group(Some("ws_1".into()), None, "Remote", &mut outcome);
        assert_eq!(
            state.config.preferences.space_groups[0].workspace_ids,
            ["ws_1"]
        );
        assert_ne!(state.config.preferences.space_groups[2].endpoint, "local");
        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn space_groups_render_collapse_and_ungroup_through_mouse_controls() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let mut snapshot = super::super::tests::snapshot();
        let mut second = snapshot.workspaces[0].clone();
        second.workspace_id = "ws_2".into();
        second.label = "second".into();
        second.focused = false;
        snapshot.workspaces.push(second);
        state.set_snapshot(Box::new(snapshot));
        state.set_pane_surface(super::super::tests::surface());
        state.compose(100, 40).expect("initial frame");
        let button = state.hits.new_space_group;
        let mut outcome = ClientShellInput::default();
        assert!(state.handle_space_group_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: button.x,
                row: button.y,
                modifiers: KeyModifiers::empty()
            },
            &mut outcome
        ));
        let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_mut() else {
            panic!("group editor");
        };
        rename.input = TextEditor::new("Work", false);
        state.save_rename_overlay(&mut outcome);
        state.save_space_group(Some("ws_1".into()), None, "Work", &mut outcome);
        state.save_space_group(Some("ws_2".into()), None, "Work", &mut outcome);
        state.compose(100, 40).expect("grouped frame");
        assert!(state.hits.machines.is_empty());
        assert_eq!(state.hits.space_groups.len(), 1);
        assert!(state
            .hits
            .workspaces
            .iter()
            .any(|hit| hit.workspace_id == "ws_2"));
        let group = state.hits.space_groups[0].0;
        assert!(state.handle_space_group_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: group.x,
                row: group.y,
                modifiers: KeyModifiers::empty()
            },
            &mut outcome
        ));
        state.compose(100, 40).expect("collapsed frame");
        assert!(state
            .hits
            .workspaces
            .iter()
            .any(|hit| hit.workspace_id == "ws_1"));
        assert!(!state
            .hits
            .workspaces
            .iter()
            .any(|hit| hit.workspace_id == "ws_2"));
        assert!(state.handle_space_group_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Right),
                column: group.x,
                row: group.y,
                modifiers: KeyModifiers::empty()
            },
            &mut outcome
        ));
        state.activate_context_menu_item(1, &mut outcome);
        state.compose(100, 40).expect("ungrouped frame");
        assert!(state.config.preferences.space_groups.is_empty());
        assert_eq!(
            state.snapshot.as_ref().expect("snapshot").workspaces.len(),
            2
        );
        assert!(
            outcome.actions.is_empty(),
            "organization must never close server workspaces"
        );
    }

    #[test]
    fn space_groups_reject_duplicate_rename_and_allow_removing_membership() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let mut outcome = ClientShellInput::default();
        state.save_space_group(Some("ws_1".into()), None, "Work", &mut outcome);
        state.save_space_group(None, None, "Other", &mut outcome);
        state.save_space_group(None, Some(1), "Work", &mut outcome);
        assert_eq!(state.config.preferences.space_groups[1].name, "Other");
        assert!(state.endpoint_error.is_some());
        state.save_space_group(Some("ws_1".into()), None, "", &mut outcome);
        assert!(state.config.preferences.space_groups[0]
            .workspace_ids
            .is_empty());
    }

    #[test]
    #[ignore = "manual fixed-geometry Space group render profile"]
    fn space_groups_render_scale_profile() {
        for count in [1, 15, 50] {
            let mut snapshot = super::super::tests::snapshot();
            let workspace = snapshot.workspaces[0].clone();
            let tab = snapshot.tabs[0].clone();
            let pane = snapshot.panes[0].clone();
            for index in 2..=count {
                let mut workspace = workspace.clone();
                workspace.workspace_id = format!("ws_{index}");
                workspace.active_tab_id = format!("tab_{index}");
                workspace.number = index;
                workspace.focused = false;
                let mut tab = tab.clone();
                tab.workspace_id = workspace.workspace_id.clone();
                tab.tab_id = workspace.active_tab_id.clone();
                tab.focused = false;
                let mut pane = pane.clone();
                pane.workspace_id = workspace.workspace_id.clone();
                pane.tab_id = tab.tab_id.clone();
                pane.pane_id = format!("pane_{index}");
                pane.focused = false;
                snapshot.workspaces.push(workspace);
                snapshot.tabs.push(tab);
                snapshot.panes.push(pane);
            }
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state
                .config
                .preferences
                .customization
                .show_selected_tab_name = true;
            state
                .config
                .preferences
                .customization
                .selected_tab_name_as_workspace = true;
            state.set_snapshot(Box::new(snapshot));
            state.set_pane_surface(super::super::tests::surface());
            for index in 1..=count {
                state.save_space_group(
                    Some(format!("ws_{index}")),
                    None,
                    if index % 2 == 0 { "Work" } else { "Research" },
                    &mut ClientShellInput::default(),
                );
            }
            let mut samples = Vec::new();
            for iteration in 0..105 {
                let start = std::time::Instant::now();
                std::hint::black_box(state.compose(120, 40).expect("profile frame"));
                if iteration >= 5 {
                    samples.push(start.elapsed().as_micros());
                }
            }
            samples.sort_unstable();
            println!(
                "Space groups, {count} populated workspaces, 120x40: median {}us, p95 {}us",
                samples[50], samples[95]
            );
        }
    }
}
