use super::*;
use crate::platform::resource_usage::{ProcessUsage, ResourceUsage};
use crate::raw_input::RawInputEvent;
use crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct ResourcePopup {
    hover: bool,
    pinned: bool,
    area: Rect,
    endpoint: Option<(ClientEndpointId, String)>,
    sample: Option<ResourceUsage>,
    error: Option<String>,
    next: Option<Instant>,
    pending: bool,
}

impl ResourcePopup {
    pub(super) fn dismiss(&mut self) {
        self.hover = false;
        self.pinned = false;
        self.area = Rect::default();
    }

    pub(super) fn visible(&self) -> bool {
        self.hover || self.pinned
    }

    pub(super) fn complete(
        &mut self,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) {
        self.pending = false;
        match result {
            Ok(crate::api::schema::ResponseResult::ResourceUsage { sample }) => {
                self.sample = sample;
                self.error = None;
            }
            Ok(_) => self.error = Some("Unexpected resource response".into()),
            Err(error) => self.error = Some(error.message),
        }
        self.next = Some(
            Instant::now()
                + if self.sample.is_some() || self.error.is_some() {
                    Duration::from_secs(5)
                } else {
                    Duration::from_millis(500)
                },
        );
    }
}

impl ClientShellState {
    pub(super) fn handle_resource_usage_event(
        &mut self,
        event: &RawInputEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.hits.agent_session_count.is_empty()
            || self.overlay.is_some()
            || self.popup_pending
            || self.hits.popup.is_some()
        {
            self.resource_usage.hover = false;
            self.resource_usage.pinned = false;
            return false;
        }
        if let RawInputEvent::Key(key) = event {
            if self.resource_usage.visible() && key.code == KeyCode::Esc {
                self.resource_usage.hover = false;
                self.resource_usage.pinned = false;
                outcome.repaint = true;
                return true;
            }
        }
        let RawInputEvent::Mouse(mouse) = event else {
            return false;
        };
        let point = (mouse.column, mouse.row);
        let hit = contains(self.hits.agent_session_count, point);
        let inside = self.resource_usage.visible() && contains(self.resource_usage.area, point);
        match mouse.kind {
            MouseEventKind::Moved => {
                let hover = hit || (self.resource_usage.hover && inside);
                outcome.repaint |= hover != self.resource_usage.hover;
                self.resource_usage.hover = hover;
                self.tick_resource_usage(Instant::now(), outcome);
                hit || inside
            }
            MouseEventKind::Down(MouseButton::Left) if hit => {
                self.resource_usage.pinned = !self.resource_usage.pinned;
                self.resource_usage.hover = self.resource_usage.pinned;
                self.tick_resource_usage(Instant::now(), outcome);
                outcome.repaint = true;
                true
            }
            MouseEventKind::Down(_) if self.resource_usage.visible() && !inside => {
                self.resource_usage.hover = false;
                self.resource_usage.pinned = false;
                outcome.repaint = true;
                true // Dismiss without forwarding the click to a terminal underneath.
            }
            _ => hit || inside,
        }
    }

    pub(crate) fn tick_resource_usage(&mut self, now: Instant, outcome: &mut ClientShellInput) {
        if !self.resource_usage.visible()
            || self.overlay.is_some()
            || self.hits.agent_session_count.is_empty()
        {
            return;
        }
        let identity = self
            .snapshot
            .as_ref()
            .map(|s| (self.active_endpoint_id.clone(), s.boot_id.clone()));
        if self.resource_usage.endpoint != identity {
            self.resource_usage.endpoint = identity;
            self.resource_usage.sample = None;
            self.resource_usage.error = None;
            self.resource_usage.pending = false;
            self.resource_usage.next = None;
            outcome.repaint = true;
        }
        if self.resource_usage.pending || self.resource_usage.next.is_some_and(|next| now < next) {
            return;
        }
        let method = crate::api::schema::Method::ServerResourceUsage(Default::default());
        if !self.endpoint_is_online(&self.active_endpoint_id)
            || !self.endpoints.iter().any(|endpoint| {
                endpoint.endpoint_id == self.active_endpoint_id
                    && endpoint
                        .methods
                        .as_ref()
                        .is_some_and(|methods| methods.contains("server.resource_usage"))
            })
        {
            self.resource_usage.error =
                Some("Unavailable: update/connect the selected server".into());
            self.resource_usage.next = Some(now + Duration::from_secs(5));
            outcome.repaint = true;
            return;
        }
        self.resource_usage.pending = self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::ResourceUsage,
            outcome,
        );
    }

    pub(super) fn render_resource_usage(&mut self, buffer: &mut Buffer) -> Rect {
        let anchor = self.hits.agent_session_count;
        let width = 58.min(buffer.area.width);
        let height = 10.min(buffer.area.height);
        let x = anchor.x.min(buffer.area.right().saturating_sub(width));
        let y = anchor
            .bottom()
            .min(buffer.area.bottom().saturating_sub(height));
        let area = Rect::new(x, y, width, height);
        self.resource_usage.area = area;
        Clear.render(area, buffer);
        let identity_matches = self.snapshot.as_ref().is_some_and(|snapshot| {
            self.resource_usage.endpoint.as_ref()
                == Some(&(self.active_endpoint_id.clone(), snapshot.boot_id.clone()))
        });
        let mut lines = vec![format!(
            "{} — current user's processes",
            self.active_endpoint_label()
        )];
        if !identity_matches {
            lines.push("Waiting for selected machine…".into());
        } else if let Some(error) = self.resource_usage.error.as_ref() {
            lines.push(error.clone());
        } else if let Some(sample) = self.resource_usage.sample.as_ref() {
            if let Some(error) = sample.error.as_ref() {
                lines.push(error.clone());
            } else {
                lines.push("             Processes       Memory (PSS)".into());
                for (label, usage) in [
                    ("Claude", &sample.claude),
                    ("Codex", &sample.codex),
                    ("MCP", &sample.mcp),
                ] {
                    lines.push(format!(
                        "{label:<12} {:>9}       {}",
                        usage.processes,
                        memory(usage)
                    ));
                }
                let age = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .saturating_sub(sample.sampled_unix_seconds);
                lines.push(format!(
                    "Sample: {age}s ago. MCP includes detected helpers."
                ));
            }
        } else {
            lines.push("Sampling process memory…".into());
        }
        lines.push("Herdr sessions above; machine processes here.".into());
        lines.push("Click count to pin · Esc / outside click closes".into());
        Paragraph::new(lines.join("\n"))
            .style(
                Style::default()
                    .fg(self.config.palette.text)
                    .bg(self.config.palette.panel_bg),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Agent resources "),
            )
            .render(area, buffer);
        area
    }
}

fn memory(usage: &ProcessUsage) -> String {
    if usage.measured < usage.processes {
        format!(
            "{:.1} MiB (partial {}/{})",
            usage.memory_bytes as f64 / 1048576.0,
            usage.measured,
            usage.processes
        )
    } else {
        format!("{:.1} MiB", usage.memory_bytes as f64 / 1048576.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseEvent};

    #[test]
    fn resource_usage_header_hover_click_response_and_dismiss() {
        let config = ClientShellConfig::from_config(&Config::default());
        let mut state = ClientShellState::new(config);
        state.set_snapshot(Box::new(super::super::tests::snapshot()));
        state.set_pane_surface(super::super::tests::surface());
        state.set_endpoint_methods(Some(vec!["server.resource_usage".into()]));
        state.compose(106, 30).unwrap();
        let hit = state.hits.agent_session_count;
        assert!(!hit.is_empty());
        assert!(hit.right() < state.hits.agent_sort_toggle.x);
        let event = |kind, x, y| {
            RawInputEvent::Mouse(MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::empty(),
            })
        };
        let out = state.handle_raw_events(vec![event(MouseEventKind::Moved, hit.x, hit.y)]);
        assert!(state.resource_usage.visible());
        let [ClientShellAction::Endpoint { request, .. }] = &out.actions[..] else {
            panic!("expected resource query");
        };
        let id = request.id.clone();
        let sample = ResourceUsage {
            claude: ProcessUsage {
                processes: 3,
                measured: 3,
                memory_bytes: 1048576,
            },
            ..Default::default()
        };
        state.handle_endpoint_result(
            "boot-1",
            &id,
            Ok(crate::api::schema::ResponseResult::ResourceUsage {
                sample: Some(sample),
            }),
        );
        let frame = state.compose(106, 30).unwrap();
        let buffer = frame.to_ratatui_buffer().unwrap();
        let text = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Claude"));
        assert!(text.contains("1.0 MiB"));
        state.handle_raw_events(vec![event(
            MouseEventKind::Down(MouseButton::Left),
            hit.x,
            hit.y,
        )]);
        assert!(state.resource_usage.pinned);
        state.handle_raw_events(vec![event(MouseEventKind::Moved, 100, 28)]);
        assert!(state.resource_usage.visible());
        let out = state.handle_raw_events(vec![event(
            MouseEventKind::Down(MouseButton::Left),
            100,
            28,
        )]);
        assert!(!state.resource_usage.visible());
        assert!(out.actions.is_empty() && out.requests.is_empty());
    }

    #[test]
    fn resource_usage_old_server_is_unavailable_without_query() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(super::super::tests::snapshot()));
        state.set_endpoint_methods(Some(vec![]));
        state.hits.agent_session_count = Rect::new(10, 10, 1, 1);
        state.resource_usage.hover = true;
        let mut out = ClientShellInput::default();
        state.tick_resource_usage(Instant::now(), &mut out);
        assert!(out.actions.is_empty());
        assert!(state
            .resource_usage
            .error
            .as_ref()
            .unwrap()
            .contains("Unavailable"));
        assert!(memory(&ProcessUsage {
            processes: 3,
            measured: 1,
            memory_bytes: 1024
        })
        .contains("partial 1/3"));
    }

    #[test]
    fn resource_usage_hidden_anchor_dismisses_and_stops_intercepting() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.resource_usage.pinned = true;
        state.resource_usage.area = Rect::new(1, 1, 30, 10);
        let event = RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 3,
            modifiers: KeyModifiers::empty(),
        });
        assert!(!state.handle_resource_usage_event(&event, &mut ClientShellInput::default()));
        assert!(!state.resource_usage.visible());
    }

    #[test]
    fn resource_usage_count_geometry_preserves_sort_at_narrow_widths() {
        let config = ClientShellConfig::from_config(&Config::default());
        for width in 0..60 {
            let area = Rect::new(0, 0, width, 5);
            let mut buffer = Buffer::empty(area);
            let mut hits = ShellHitMap::default();
            super::super::agent_sidebar::render_agent_panel_header(
                &mut buffer,
                area,
                None,
                123,
                &config,
                &mut hits,
            );
            if !hits.agent_session_count.is_empty() {
                assert!(hits.agent_session_count.right() < hits.agent_sort_toggle.x);
                assert_eq!(hits.agent_session_count.width, 3);
            }
        }
    }
}
