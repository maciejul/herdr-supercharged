//! Client-only tab list beneath each endpoint's open workspace.
use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

const VISIBLE_TABS: usize = 8;

pub(super) fn line_count(
    snapshot: &ClientShellSnapshot,
    workspace: &ClientShellWorkspace,
    config: &ClientShellConfig,
) -> usize {
    if !config.preferences.customization.show_workspace_tabs || !workspace.focused {
        return 0;
    }
    let count = snapshot
        .tabs
        .iter()
        .filter(|tab| tab.workspace_id == workspace.workspace_id)
        .count();
    count.min(VISIBLE_TABS) + usize::from(count > VISIBLE_TABS)
}

pub(super) fn render(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    workspace: &ClientShellWorkspace,
    endpoint: &ClientEndpointId,
    config: &ClientShellConfig,
    offsets: &HashMap<(ClientEndpointId, String), usize>,
    hits: &mut ShellHitMap,
) -> u16 {
    let height = line_count(snapshot, workspace, config).min(usize::from(area.height));
    if height == 0 || area.width == 0 {
        return 0;
    }
    let tabs = snapshot
        .tabs
        .iter()
        .filter(|tab| tab.workspace_id == workspace.workspace_id)
        .collect::<Vec<_>>();
    let overflow = tabs.len() > height;
    // Keep at least one tab reachable even when the sidebar is only one line tall.
    let visible = height.saturating_sub(usize::from(overflow && height > 1));
    let max_offset = tabs.len().saturating_sub(visible);
    let key = (endpoint.clone(), workspace.workspace_id.clone());
    let offset = offsets.get(&key).copied().unwrap_or(0).min(max_offset);
    for (index, tab) in tabs.iter().skip(offset).take(visible).enumerate() {
        let rect = Rect::new(area.x, area.y + index as u16, area.width, 1);
        let focused = tab.tab_id == workspace.active_tab_id;
        let label: String = tab.label.chars().filter(|c| !c.is_control()).collect();
        let label = if label.trim().is_empty() {
            format!("Tab {}", tab.number)
        } else {
            label
        };
        let text = format!("   {} {}", if focused { "▸" } else { "·" }, label);
        let style = Style::default().fg(if focused {
            config.palette.accent
        } else {
            config.palette.subtext0
        });
        render::put_text(buffer, rect.x, rect.y, rect.width, &text, style);
        hits.workspace_tabs
            .push((rect, endpoint.clone(), tab.tab_id.clone()));
    }
    if overflow && height > 1 {
        render::put_text(
            buffer,
            area.x,
            area.y + height as u16 - 1,
            area.width,
            &format!(
                "   {}–{} / {} · scroll",
                offset + 1,
                offset + visible,
                tabs.len()
            ),
            Style::default().fg(config.palette.overlay0),
        );
    }
    if max_offset > 0 {
        hits.workspace_tabs_scroll.push((
            Rect::new(area.x, area.y, area.width, height as u16),
            endpoint.clone(),
            workspace.workspace_id.clone(),
            max_offset,
        ));
    }
    height as u16
}

impl ClientShellState {
    pub(super) fn handle_workspace_tabs_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.config.mouse_capture {
            return false;
        }
        let point = (mouse.column, mouse.row);
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            let Some((_, endpoint, workspace, max_offset)) = self
                .hits
                .workspace_tabs_scroll
                .iter()
                .find(|(rect, _, _, _)| super::contains(*rect, point))
            else {
                return false;
            };
            let offset = self
                .workspace_tab_offsets
                .entry((endpoint.clone(), workspace.clone()))
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
        let Some((_, endpoint, tab)) = self
            .hits
            .workspace_tabs
            .iter()
            .find(|(rect, _, _)| super::contains(*rect, point))
            .cloned()
        else {
            return false;
        };
        self.workspace_press = None;
        self.tab_press = None;
        if endpoint == self.active_endpoint_id {
            self.push_endpoint_method(
                crate::api::schema::Method::TabFocus(crate::api::schema::TabTarget { tab_id: tab }),
                outcome,
            );
        } else {
            outcome.actions.push(ClientShellAction::ActivateEndpoint {
                endpoint_id: endpoint,
                target: Some(ClientEndpointFocusTarget::Tab(tab)),
            });
        }
        outcome.repaint = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(count: usize, grouped: bool) -> ClientShellState {
        let mut snapshot = super::super::tests::snapshot();
        for number in 2..=count {
            let mut tab = snapshot.tabs[0].clone();
            tab.tab_id = format!("tab_{number}");
            tab.number = number;
            tab.label = format!("Session {number}");
            tab.focused = false;
            snapshot.tabs.push(tab);
        }
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.preferences.customization.show_workspace_tabs = true;
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
        state
    }

    fn mouse(rect: Rect, kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: rect.x + 1,
            row: rect.y,
            modifiers: crossterm::event::KeyModifiers::empty(),
        }
    }

    #[test]
    fn tab_rows_focus_stable_ids_in_flat_and_grouped_spaces() {
        for grouped in [false, true] {
            let mut state = state(3, grouped);
            state.compose(100, 40).expect("frame");
            assert_eq!(state.hits.workspace_tabs.len(), 3);
            let (rect, _, id) = state.hits.workspace_tabs[1].clone();
            assert_eq!(id, "tab_2");
            let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(mouse(
                rect,
                MouseEventKind::Down(MouseButton::Left),
            ))]);
            assert!(outcome.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(&request.method, crate::api::schema::Method::TabFocus(target) if target.tab_id == "tab_2"))));
            assert!(state.workspace_press.is_none());
        }
    }

    #[test]
    fn overflow_tabs_remain_reachable_in_short_grouped_sidebar() {
        let mut state = state(20, true);
        state.compose(100, 20).expect("short frame");
        assert!(!state.hits.workspace_tabs.is_empty());
        for _ in 0..25 {
            let rect = state.hits.workspace_tabs_scroll[0].0;
            state.handle_raw_events(vec![RawInputEvent::Mouse(mouse(
                rect,
                MouseEventKind::ScrollDown,
            ))]);
            state.compose(100, 20).expect("scrolled frame");
        }
        assert_eq!(
            state.hits.workspace_tabs.last().expect("last tab").2,
            "tab_20"
        );
    }

    #[test]
    fn preference_defaults_off_and_list_suppresses_duplicate_subtitle() {
        let old: preferences::CustomizationPreferences =
            serde_json::from_str("{}").expect("old settings");
        assert!(!old.show_workspace_tabs);
        let mut state = state(3, false);
        state
            .config
            .preferences
            .customization
            .show_selected_tab_name = true;
        let snapshot = state.snapshot.as_deref().expect("snapshot");
        let workspace = &snapshot.workspaces[0];
        let rows = sidebar::customized_workspace_rows(
            snapshot,
            workspace,
            workspace.agent_status,
            false,
            &state.config,
        );
        assert!(!rows
            .iter()
            .flatten()
            .any(|token| matches!(token.kind, crate::ui::ResolvedTokenKind::Tab(_))));
        let saved = serde_json::to_string(&state.config.preferences).expect("serialize");
        let loaded: preferences::ClientChromePreferences =
            serde_json::from_str(&saved).expect("reload");
        assert!(loaded.customization.show_workspace_tabs);
        state.config.preferences.customization.show_workspace_tabs = false;
        state.compose(100, 40).expect("disabled frame");
        assert!(state.hits.workspace_tabs.is_empty());
    }

    #[test]
    fn tab_click_targets_the_owning_endpoint() {
        let mut state = state(1, false);
        let endpoint = ClientEndpointId::Ssh(
            crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                .expect("profile"),
        );
        let rect = Rect::new(0, 0, 10, 1);
        state
            .hits
            .workspace_tabs
            .push((rect, endpoint.clone(), "tab_1".into()));
        let mut outcome = ClientShellInput::default();
        assert!(state.handle_workspace_tabs_mouse(
            mouse(rect, MouseEventKind::Down(MouseButton::Left)),
            &mut outcome
        ));
        assert!(
            matches!(&outcome.actions[..], [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Tab(tab)) }] if endpoint_id == &endpoint && tab == "tab_1")
        );
    }
}
