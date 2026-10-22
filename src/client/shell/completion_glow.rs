//! Animate visible completion dots and unread tab labels, never pane content.
use super::*;
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_millis(100);
const STEPS: u8 = 20;

impl ClientShellState {
    pub(crate) fn tick_completion_glow(&mut self, now: Instant) -> bool {
        if self
            .completion_glow_deadline
            .is_none_or(|deadline| now < deadline)
        {
            return false;
        }
        self.completion_glow_step = (self.completion_glow_step + 1) % STEPS;
        self.completion_glow_deadline = None;
        true
    }

    pub(super) fn apply_completion_glow(
        &mut self,
        frame: &mut FrameData,
        sidebar: Rect,
        now: Instant,
    ) {
        let bright = self
            .config
            .preferences
            .customization
            .colors
            .completion
            .color();
        let marker_color = crate::protocol::color_to_u32(bright);
        let bold = crate::protocol::modifier_to_u16(Modifier::BOLD);
        let dim = crate::protocol::modifier_to_u16(Modifier::DIM);
        let color = crate::protocol::color_to_u32(pulse_color(bright, self.completion_glow_step));
        let mut visible = false;
        // Bounded by sidebar screen cells, independent of hidden panes/agents.
        // Inspect the final frame so collapsed sections and covering overlays
        // cannot keep an invisible completion animation running.
        for y in sidebar.y..sidebar.bottom().min(frame.height) {
            for x in sidebar.x..sidebar.right().min(frame.width) {
                let cell =
                    &mut frame.cells[usize::from(y) * usize::from(frame.width) + usize::from(x)];
                if self.config.status_indicators == crate::config::StatusIndicatorStyle::Dots
                    && cell.symbol == "●"
                    && cell.fg == marker_color
                    && cell.modifier & bold != 0
                    && cell.modifier & dim == 0
                {
                    cell.fg = color;
                    visible = true;
                }
            }
        }
        // Only registered visible tab rectangles can animate. Pause tab highlights
        // while an overlay is open so identically styled overlay text is never changed.
        if self.overlay.is_none()
            && self.endpoint_status(&self.active_endpoint_id) == Some(ClientEndpointStatus::Online)
        {
            if let Some(snapshot) = self.snapshot.as_deref() {
                for (rect, id) in &self.hits.tabs {
                    if !snapshot.tabs.iter().any(|tab| {
                        tab.tab_id == *id
                            && tab.agent_status == crate::api::schema::AgentStatus::Done
                    }) {
                        continue;
                    }
                    for y in rect.y..rect.bottom().min(frame.height) {
                        for x in rect.x..rect.right().min(frame.width) {
                            let cell = &mut frame.cells
                                [usize::from(y) * usize::from(frame.width) + usize::from(x)];
                            if cell.fg == marker_color
                                && cell.modifier & bold != 0
                                && cell.modifier & dim == 0
                                && !cell.symbol.trim().is_empty()
                            {
                                cell.fg = color;
                                visible = true;
                            }
                        }
                    }
                }
            }
        }
        if visible {
            self.completion_glow_deadline.get_or_insert(now + INTERVAL);
        } else {
            self.completion_glow_deadline = None;
            self.completion_glow_step = 0;
        }
    }
}

fn pulse_color(bright: ratatui::style::Color, step: u8) -> ratatui::style::Color {
    let (r, g, b) = match bright {
        ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
        _ => (144, 255, 144),
    };
    let distance = (step % STEPS).min(STEPS - step % STEPS);
    // A two-second breath: full brightness -> 50% -> full brightness.
    let scale = 100 - u16::from(distance) * 5;
    let channel = |value: u8| (u16::from(value) * scale / 100) as u8;
    ratatui::style::Color::Rgb(channel(r), channel(g), channel(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_tab_labels_pulse_and_stop_after_seen_without_touching_panes() {
        let mut snapshot = super::super::tests::snapshot();
        snapshot.tabs[0].label = "Ready result".into();
        snapshot.tabs[0].custom_label = true;
        snapshot.tabs[0].agent_status = crate::api::schema::AgentStatus::Done;
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(snapshot));
        state.set_pane_surface(super::super::tests::surface());
        state.completion_glow_step = 5;
        let frame = state.compose(100, 40).expect("unread tab frame").frame;
        let rect = state.hits.tabs[0].0;
        let expected = crate::protocol::color_to_u32(pulse_color(
            state
                .config
                .preferences
                .customization
                .colors
                .completion
                .color(),
            5,
        ));
        assert!((rect.x..rect.right()).any(|x| {
            let cell =
                &frame.cells[usize::from(rect.y) * usize::from(frame.width) + usize::from(x)];
            cell.symbol == "R" && cell.fg == expected
        }));
        assert!(state.completion_glow_deadline.is_some());
        let mut snapshot = state.snapshot.as_ref().expect("snapshot").as_ref().clone();
        snapshot.tabs[0].agent_status = crate::api::schema::AgentStatus::Idle;
        state.set_snapshot(Box::new(snapshot));
        state.compose(100, 40).expect("seen tab frame");
        assert!(state.completion_glow_deadline.is_none());
    }

    #[test]
    fn tab_animation_is_bounded_to_visible_tabs_and_pauses_for_overlays_or_stale_endpoints() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let mut snapshot = super::super::tests::snapshot();
        snapshot.tabs[0].agent_status = crate::api::schema::AgentStatus::Done;
        state.set_snapshot(Box::new(snapshot));
        state
            .hits
            .tabs
            .push((Rect::new(0, 0, 2, 1), "tab_1".into()));
        let marker = Style::default()
            .fg(state
                .config
                .preferences
                .customization
                .colors
                .completion
                .color())
            .add_modifier(Modifier::BOLD);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
        for x in 0..4 {
            buffer[(x, 0)].set_symbol("T").set_style(marker);
        }
        buffer[(0, 1)].set_symbol("P").set_style(marker);
        let source = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        let mut frame = source.clone();
        state.completion_glow_step = 5;
        state.apply_completion_glow(&mut frame, Rect::default(), Instant::now());
        assert_ne!(frame.cells[0].fg, source.cells[0].fg);
        assert_eq!(&frame.cells[2..], &source.cells[2..]);
        state.open_settings_overlay();
        let mut frame = source.clone();
        state.apply_completion_glow(&mut frame, Rect::default(), Instant::now());
        assert_eq!(frame.cells, source.cells);
        assert!(state.completion_glow_deadline.is_none());
        state.overlay = None;
        state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Reconnecting);
        let mut frame = source.clone();
        state.apply_completion_glow(&mut frame, Rect::default(), Instant::now());
        assert_eq!(frame.cells, source.cells);
        assert!(state.completion_glow_deadline.is_none());
    }

    #[test]
    fn configured_colors_drive_working_and_completion_glow() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.config.preferences.customization.colors.completion = preferences::ColorChoice::Purple;
        state.config.preferences.customization.colors.working = preferences::ColorChoice::Blue;
        assert_eq!(
            state
                .config
                .status_color(crate::api::schema::AgentStatus::Working),
            preferences::ColorChoice::Blue.color()
        );
        let marker = state
            .config
            .status_icon_style(crate::api::schema::AgentStatus::Done);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer[(0, 0)].set_symbol("●").set_style(marker);
        let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        state.completion_glow_step = 10;
        state.apply_completion_glow(&mut frame, Rect::new(0, 0, 1, 1), Instant::now());
        assert_eq!(
            frame.cells[0].fg,
            crate::protocol::color_to_u32(pulse_color(
                preferences::ColorChoice::Purple.color(),
                10
            ))
        );
        assert!(state.completion_glow_deadline.is_some());
    }

    #[test]
    fn completion_glow_breathes_and_returns_to_full_brightness() {
        use ratatui::style::Color;
        let bright = Color::Rgb(144, 240, 180);
        assert_eq!(pulse_color(bright, 0), bright);
        assert_eq!(pulse_color(bright, 10), Color::Rgb(72, 120, 90));
        assert_eq!(pulse_color(bright, 20), bright);
        for step in 1..10 {
            assert_eq!(pulse_color(bright, step), pulse_color(bright, 20 - step));
        }
    }

    #[test]
    fn completion_glow_preserves_pane_content_and_ignores_covered_or_dim_dots() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let marker = state
            .config
            .status_icon_style(crate::api::schema::AgentStatus::Done);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        for x in [0, 1, 3] {
            buffer[(x, 0)].set_symbol("●").set_style(marker);
        }
        buffer[(0, 0)].set_style(Style::default().add_modifier(Modifier::DIM));
        let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        let pane = frame.cells[3].clone();
        let stale = frame.cells[0].clone();
        state.completion_glow_step = 5;
        let now = Instant::now();
        state.apply_completion_glow(&mut frame, Rect::new(0, 0, 2, 1), now);
        assert_eq!(frame.cells[3], pane);
        assert_eq!(frame.cells[0], stale);
        assert_ne!(frame.cells[1].fg, pane.fg);
        assert!(state.completion_glow_deadline.is_some());

        buffer[(1, 0)].set_symbol(" "); // An overlay covers the only live dot.
        let mut covered = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        state.apply_completion_glow(&mut covered, Rect::new(0, 0, 2, 1), now);
        assert!(state.completion_glow_deadline.is_none());
    }
}
