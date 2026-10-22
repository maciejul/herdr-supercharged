use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ActivityKind {
    Subagent,
    Task,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct BackgroundItem {
    pub(super) id: String,
    pub(super) kind: ActivityKind,
    pub(super) label: String,
}

#[derive(Default, Deserialize)]
pub(super) struct BackgroundActivity {
    pub(super) items: Vec<BackgroundItem>,
}

pub(super) fn agent_icon_style(
    agent: &crate::protocol::ClientShellAgent,
    config: &ClientShellConfig,
) -> Style {
    if activity_only(&agent.tokens) {
        Style::default().fg(config
            .preferences
            .customization
            .colors
            .background_tasks
            .color())
    } else {
        config.status_icon_style(agent.agent_status)
    }
}

pub(super) fn activity_only(tokens: &[(String, String)]) -> bool {
    tokens
        .iter()
        .any(|(key, value)| key == "herdr_activity_only" && value == "true")
}

impl BackgroundActivity {
    pub(super) fn for_workspace(
        snapshot: &ClientShellSnapshot,
        workspace_id: &str,
        config: &ClientShellConfig,
    ) -> Self {
        let mut items = Vec::new();
        for agent in snapshot
            .agents
            .iter()
            .filter(|agent| agent.workspace_id == workspace_id)
        {
            for mut item in Self::for_agent(agent, config).items {
                // Child IDs are local to the parent session, not globally unique.
                item.id = format!("{}:{}", agent.pane_id, item.id);
                items.push(item);
            }
        }
        items.sort_by_key(|item| item.kind != ActivityKind::Subagent);
        Self { items }
    }

    pub(super) fn for_agent(
        agent: &crate::protocol::ClientShellAgent,
        config: &ClientShellConfig,
    ) -> Self {
        let prefs = &config.preferences.customization;
        let ordinary_shell = activity_only(&agent.tokens);
        if ordinary_shell && !prefs.include_terminal_shell_tasks {
            return Self::default();
        }
        let mut activity = Self::from_tokens(&agent.tokens);
        if !ordinary_shell && !prefs.include_agent_shell_tasks {
            activity
                .items
                .retain(|item| item.kind == ActivityKind::Subagent);
        }
        activity
    }

    pub(super) fn from_tokens(tokens: &[(String, String)]) -> Self {
        let Some((_, value)) = tokens
            .iter()
            .find(|(key, _)| key == "herdr_background_activity")
        else {
            return Self::default();
        };
        // Metadata is external input. Bound decoding and presentation work per agent.
        if value.len() > 16_384 {
            return Self::default();
        }
        let Ok(mut activity) = serde_json::from_str::<Self>(value) else {
            return Self::default();
        };
        let mut ids = HashSet::new();
        activity.items.retain(|item| {
            !item.id.is_empty()
                && ids.insert((item.kind == ActivityKind::Subagent, item.id.clone()))
        });
        activity
            .items
            .sort_by_key(|item| item.kind != ActivityKind::Subagent);
        activity.items.truncate(64);
        activity
    }

    pub(super) fn lines(&self, expanded: bool) -> usize {
        if self.items.is_empty() {
            0
        } else {
            1 + if expanded { self.items.len() } else { 0 }
        }
    }

    fn summary(&self) -> (ActivityKind, String) {
        let subagents = self
            .items
            .iter()
            .filter(|item| item.kind == ActivityKind::Subagent)
            .count();
        if subagents > 0 {
            (
                ActivityKind::Subagent,
                format!(
                    "{subagents} subagent{} running",
                    if subagents == 1 { "" } else { "s" }
                ),
            )
        } else {
            let count = self.items.len();
            (
                ActivityKind::Task,
                format!("{count} task{} running", if count == 1 { "" } else { "s" }),
            )
        }
    }

    pub(super) fn render(
        &self,
        buffer: &mut Buffer,
        rect: Rect,
        expanded: bool,
        offset: usize,
        config: &ClientShellConfig,
    ) {
        if rect.is_empty() || self.items.is_empty() {
            return;
        }
        let colors = &config.preferences.customization.colors;
        let color = |kind: &ActivityKind| match kind {
            ActivityKind::Subagent => colors.subagents.color(),
            ActivityKind::Task => colors.background_tasks.color(),
        };
        let (kind, mut label) = self.summary();
        let visible = usize::from(rect.height.saturating_sub(1));
        let offset = offset.min(self.items.len().saturating_sub(visible));
        if expanded && visible < self.items.len() {
            label.push_str(&format!(" ({}/{}; scroll)", offset + 1, self.items.len()));
        }
        let line = ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(
                format!("   {} ", if expanded { "▾" } else { "▸" }),
                Style::default().fg(config.palette.overlay0),
            ),
            ratatui::text::Span::styled("● ", Style::default().fg(color(&kind))),
            ratatui::text::Span::styled(label, Style::default().fg(config.palette.subtext0)),
        ]);
        use ratatui::widgets::Widget;
        ratatui::widgets::Paragraph::new(line)
            .render(Rect::new(rect.x, rect.y, rect.width, 1), buffer);
        if expanded {
            for (index, item) in self
                .items
                .iter()
                .skip(offset)
                .take(rect.height.saturating_sub(1) as usize)
                .enumerate()
            {
                let label: String = item
                    .label
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(256)
                    .collect();
                let label = if label.trim().is_empty() {
                    match item.kind {
                        ActivityKind::Subagent => "Subagent",
                        ActivityKind::Task => "Background task",
                    }
                } else {
                    &label
                };
                let line = ratatui::text::Line::from(vec![
                    ratatui::text::Span::styled(
                        "     └ ",
                        Style::default().fg(config.palette.surface_dim),
                    ),
                    ratatui::text::Span::styled("● ", Style::default().fg(color(&item.kind))),
                    ratatui::text::Span::styled(
                        label,
                        Style::default().fg(config.palette.subtext0),
                    ),
                ]);
                ratatui::widgets::Paragraph::new(line).render(
                    Rect::new(rect.x, rect.y + index as u16 + 1, rect.width, 1),
                    buffer,
                );
            }
        }
    }
}

impl ClientShellState {
    pub(super) fn handle_background_activity_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            let Some((_, endpoint, pane, max_offset)) = self
                .hits
                .background_activity_scroll
                .iter()
                .find(|(rect, _, _, _)| super::contains(*rect, (mouse.column, mouse.row)))
            else {
                return false;
            };
            let offset = self
                .background_activity_offsets
                .entry((endpoint.clone(), pane.clone()))
                .or_default();
            *offset = (*offset).min(*max_offset);
            *offset = if mouse.kind == MouseEventKind::ScrollUp {
                offset.saturating_sub(1)
            } else {
                offset.saturating_add(1).min(*max_offset)
            };
            outcome.repaint = true;
            return true;
        }
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return false;
        }
        let Some((_, endpoint, pane)) = self
            .hits
            .background_activity
            .iter()
            .find(|(rect, _, _)| super::contains(*rect, (mouse.column, mouse.row)))
        else {
            return false;
        };
        let key = (endpoint.clone(), pane.clone());
        if !self.expanded_background_activity.remove(&key) {
            self.expanded_background_activity.insert(key);
        }
        outcome.repaint = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn activity_agent(
        pane: &str,
        workspace: &str,
        ordinary_shell: bool,
    ) -> crate::protocol::ClientShellAgent {
        crate::protocol::ClientShellAgent {
            pane_id: pane.into(),
            workspace_id: workspace.into(),
            tab_id: format!("tab-{pane}"),
            name: None,
            display_agent: None,
            agent: None,
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: crate::api::schema::AgentStatus::Working,
            state_change_seq: 0,
            state_labels: vec![],
            focused: false,
            tokens: if ordinary_shell {
                vec![
                    ("herdr_activity_only".into(), "true".into()),
                    (
                        "herdr_background_activity".into(),
                        r#"{"items":[{"id":"same","kind":"task","label":"terminal build"}]}"#
                            .into(),
                    ),
                ]
            } else {
                vec![("herdr_background_activity".into(), r#"{"items":[{"id":"same","kind":"task","label":"agent build"},{"id":"a","kind":"subagent","label":"review"}]}"#.into())]
            },
        }
    }

    #[test]
    fn workspace_activity_aggregates_all_tabs_and_applies_source_filters() {
        let mut snapshot = super::super::tests::snapshot();
        snapshot.agents = vec![
            activity_agent("one", "ws_1", false),
            activity_agent("two", "ws_1", true),
            activity_agent("other", "ws_2", false),
        ];
        let mut config = ClientShellConfig::from_config(&Config::default());
        let activity = BackgroundActivity::for_workspace(&snapshot, "ws_1", &config);
        assert_eq!(
            activity.items.len(),
            3,
            "ordinary command counted once plus agent command and subagent"
        );
        assert_eq!(activity.items[0].kind, ActivityKind::Subagent);
        assert_eq!(
            activity
                .items
                .iter()
                .map(|item| &item.id)
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        config.preferences.customization.include_agent_shell_tasks = false;
        assert_eq!(
            BackgroundActivity::for_workspace(&snapshot, "ws_1", &config)
                .items
                .len(),
            2
        );
        config
            .preferences
            .customization
            .include_terminal_shell_tasks = false;
        let activity = BackgroundActivity::for_workspace(&snapshot, "ws_1", &config);
        assert_eq!(activity.items.len(), 1);
        assert_eq!(activity.items[0].kind, ActivityKind::Subagent);
        assert!(
            BackgroundActivity::for_workspace(&snapshot, "missing", &config)
                .items
                .is_empty()
        );
    }

    #[test]
    fn hidden_terminal_tasks_are_excluded_from_local_and_endpoint_navigation() {
        let mut snapshot = super::super::tests::snapshot();
        snapshot.agents = vec![
            activity_agent("agent", "ws_1", false),
            activity_agent("shell", "ws_1", true),
        ];
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state
            .config
            .preferences
            .customization
            .include_terminal_shell_tasks = false;
        state.set_snapshot(Box::new(snapshot));
        assert_eq!(
            super::super::agent_sidebar::visible_agent_pane_ids(
                state.snapshot.as_deref().expect("snapshot"),
                &state.config
            ),
            vec!["agent"]
        );
        let targets = super::super::aggregate_navigation::online_agent_targets(
            &state.endpoints,
            &state.active_endpoint_id,
            state.config.agent_panel_sort,
            false,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].pane_id, "agent");
    }

    #[test]
    fn ordinary_shell_dots_are_blue_and_agent_dots_stay_orange() {
        let config = ClientShellConfig::from_config(&Config::default());
        assert_eq!(
            agent_icon_style(&activity_agent("shell", "ws_1", true), &config).fg,
            Some(
                config
                    .preferences
                    .customization
                    .colors
                    .background_tasks
                    .color()
            )
        );
        assert_eq!(
            agent_icon_style(&activity_agent("agent", "ws_1", false), &config).fg,
            Some(config.preferences.customization.colors.working.color())
        );
    }

    #[test]
    fn summary_prioritizes_subagents_and_details_preserve_tasks() {
        let activity = BackgroundActivity::from_tokens(&[("herdr_background_activity".into(), r#"{"items":[{"id":"task","kind":"task","label":"build"},{"id":"agent","kind":"subagent","label":"review"}]}"#.into())]);
        assert_eq!(
            activity.summary(),
            (ActivityKind::Subagent, "1 subagent running".into())
        );
        assert_eq!(activity.items[0].kind, ActivityKind::Subagent);
        assert_eq!(activity.lines(false), 1);
        assert_eq!(activity.lines(true), 3);
        let config = ClientShellConfig::from_config(&Config::default());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 3));
        activity.render(&mut buffer, Rect::new(0, 0, 40, 3), true, 0, &config);
        assert_eq!(
            buffer[(5, 0)].fg,
            config.preferences.customization.colors.subagents.color()
        );
        assert_eq!(
            buffer[(7, 2)].fg,
            config
                .preferences
                .customization
                .colors
                .background_tasks
                .color()
        );
    }
    #[test]
    fn nested_activity_has_separate_hit_and_toggle_does_not_focus_parent() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let row = super::super::agent_sidebar::AgentRow {
            pane_id: "pane_1".into(),
            status: crate::api::schema::AgentStatus::Working,
            focused: false,
            activity_only: false,
            rows: vec![],
            activity: BackgroundActivity::from_tokens(&[(
                "herdr_background_activity".into(),
                r#"{"items":[{"id":"a","kind":"subagent","label":"review"}]}"#.into(),
            )]),
        };
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 8));
        let parent = super::super::agent_sidebar::render_agent_with_activity(
            &mut buffer,
            Rect::new(0, 2, 40, 2),
            &row,
            &ClientEndpointId::Local,
            &state.expanded_background_activity,
            &state.background_activity_offsets,
            &state.config,
            &mut state.hits,
        );
        assert_eq!(parent, Rect::new(0, 2, 40, 1));
        assert_eq!(state.hits.background_activity[0].0, Rect::new(0, 3, 40, 1));
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 6,
            row: 3,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        let mut outcome = ClientShellInput::default();
        assert!(state.handle_background_activity_mouse(mouse, &mut outcome));
        assert!(outcome.repaint);
        assert!(state
            .expanded_background_activity
            .contains(&(ClientEndpointId::Local, "pane_1".into())));
        assert_eq!(row.line_count(true), 3);
        assert!(state.handle_background_activity_mouse(mouse, &mut outcome));
        assert!(state.expanded_background_activity.is_empty());
        assert!(
            !state.handle_background_activity_mouse(MouseEvent { row: 2, ..mouse }, &mut outcome)
        );
    }

    #[test]
    fn long_activity_list_scrolls_to_last_item_without_moving_parent() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let activity = BackgroundActivity {
            items: (0..10)
                .map(|index| BackgroundItem {
                    id: format!("task{index}"),
                    kind: ActivityKind::Task,
                    label: format!("job {index}"),
                })
                .collect(),
        };
        let row = super::super::agent_sidebar::AgentRow {
            pane_id: "pane_1".into(),
            status: crate::api::schema::AgentStatus::Working,
            focused: false,
            activity_only: false,
            rows: vec![],
            activity,
        };
        state
            .expanded_background_activity
            .insert((ClientEndpointId::Local, "pane_1".into()));
        let rect = Rect::new(0, 0, 45, 4);
        let mut buffer = Buffer::empty(rect);
        super::super::agent_sidebar::render_agent_with_activity(
            &mut buffer,
            rect,
            &row,
            &ClientEndpointId::Local,
            &state.expanded_background_activity,
            &state.background_activity_offsets,
            &state.config,
            &mut state.hits,
        );
        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 7,
            row: 2,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        for _ in 0..20 {
            assert!(state.handle_background_activity_mouse(mouse, &mut ClientShellInput::default()));
        }
        assert_eq!(
            state.background_activity_offsets[&(ClientEndpointId::Local, "pane_1".into())],
            8
        );
        assert_eq!(state.agent_scroll, 0);
        super::super::agent_sidebar::render_agent_with_activity(
            &mut buffer,
            rect,
            &row,
            &ClientEndpointId::Local,
            &state.expanded_background_activity,
            &state.background_activity_offsets,
            &state.config,
            &mut state.hits,
        );
        let text = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("job 9"));
    }

    #[test]
    #[ignore = "manual fixed geometry background activity render profile"]
    fn background_activity_render_scale_profile() {
        for count in [1, 15] {
            let mut snapshot = super::super::tests::snapshot();
            for index in 0..count {
                let mut pane = snapshot.panes[0].clone();
                pane.pane_id = format!("activity_pane_{index}");
                pane.focused = false;
                snapshot.agents.push(crate::protocol::ClientShellAgent {
                    pane_id: pane.pane_id.clone(), workspace_id: pane.workspace_id.clone(), tab_id: pane.tab_id.clone(),
                    name: Some(format!("agent {index}")), display_agent: None, agent: Some("Claude".into()), title: None,
                    terminal_title: None, terminal_title_stripped: None, agent_status: crate::api::schema::AgentStatus::Working,
                    state_change_seq: 0, state_labels: vec![],
                    tokens: vec![("herdr_background_activity".into(), r#"{"items":[{"id":"a","kind":"subagent","label":"review"},{"id":"b","kind":"task","label":"build"}]}"#.into())], focused: false,
                });
                snapshot.panes.push(pane);
            }
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state.set_snapshot(Box::new(snapshot));
            state.set_pane_surface(super::super::tests::surface());
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
                "Background activity, {count} populated agents, 120x40: median {}us, p95 {}us",
                samples[50], samples[95]
            );
        }
    }

    #[test]
    fn shared_custom_colors_do_not_make_running_activity_pulse() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let colors = &mut state.config.preferences.customization.colors;
        colors.working = colors.completion;
        colors.subagents = colors.completion;
        colors.background_tasks = colors.completion;
        let activity = BackgroundActivity::from_tokens(&[("herdr_background_activity".into(), r#"{"items":[{"id":"a","kind":"subagent","label":"review"},{"id":"b","kind":"task","label":"build"}]}"#.into())]);
        let area = Rect::new(0, 0, 40, 5);
        let mut buffer = Buffer::empty(area);
        activity.render(&mut buffer, Rect::new(0, 0, 40, 3), true, 0, &state.config);
        buffer[(0, 3)].set_symbol("●").set_style(
            state
                .config
                .status_icon_style(crate::api::schema::AgentStatus::Working),
        );
        buffer[(0, 4)].set_symbol("●").set_style(
            state
                .config
                .status_icon_style(crate::api::schema::AgentStatus::Done),
        );
        let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        let before = frame.cells.clone();
        state.completion_glow_step = 5;
        state.apply_completion_glow(&mut frame, area, std::time::Instant::now());
        assert_eq!(&frame.cells[..160], &before[..160]);
        assert_ne!(frame.cells[160].fg, before[160].fg);
    }

    #[test]
    fn metadata_deduplicates_ids_within_kind_not_across_kinds() {
        let activity = BackgroundActivity::from_tokens(&[("herdr_background_activity".into(), r#"{"items":[{"id":"same","kind":"task","label":"build"},{"id":"same","kind":"subagent","label":"review"},{"id":"same","kind":"task","label":"duplicate build"}]}"#.into())]);
        assert_eq!(activity.items.len(), 2);
        assert_eq!(activity.items[0].kind, ActivityKind::Subagent);
        assert_eq!(activity.items[1].label, "build");
    }

    #[test]
    fn tasks_have_their_own_summary_and_bad_input_is_ignored() {
        let activity = BackgroundActivity::from_tokens(&[(
            "herdr_background_activity".into(),
            r#"{"items":[{"id":"task","kind":"task","label":"build"}]}"#.into(),
        )]);
        assert_eq!(
            activity.summary(),
            (ActivityKind::Task, "1 task running".into())
        );
        assert_eq!(
            BackgroundActivity::from_tokens(&[(
                "herdr_background_activity".into(),
                "invalid".into()
            )])
            .lines(true),
            0
        );
    }
}
