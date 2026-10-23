use crate::terminal::TerminalId;

/// Viewport state for a pane.
///
/// Terminal identity, cwd, labels, and agent metadata live in TerminalState.
pub struct PaneState {
    pub attached_terminal_id: TerminalId,
    /// Whether the user has seen this pane since its last state change to Idle.
    /// False = "Done" (agent finished while user was in another workspace).
    pub seen: bool,
    /// Server-owned inactivity deadline anchor; reset when this pane is read.
    pub(crate) suspension_reset_at: Option<std::time::Instant>,
    pub(crate) unread_since: Option<std::time::Instant>,
    /// Retain this pane while its resumable agent process is stopped.
    pub(crate) suspended: bool,
    pub(crate) suspended_resume_requested: bool,
    pub(crate) suspension_exit_pending: bool,
    /// Whether unmodified right-click gestures should be forwarded to the pane application.
    pub right_click_passthrough: bool,
}

impl PaneState {
    pub fn new(attached_terminal_id: TerminalId) -> Self {
        Self {
            attached_terminal_id,
            seen: true,
            unread_since: None,
            suspension_reset_at: None,
            suspended: false,
            suspended_resume_requested: false,
            suspension_exit_pending: false,
            right_click_passthrough: false,
        }
    }
}
