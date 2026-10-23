use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

pub(super) fn normalized_theme_name(name: &str) -> String {
    name.to_lowercase().replace([' ', '_'], "-")
}

fn theme_index(name: &str) -> usize {
    let normalized = normalized_theme_name(name);
    crate::config::THEME_NAMES
        .iter()
        .position(|candidate| normalized_theme_name(candidate) == normalized)
        .unwrap_or(0)
}

fn indicator_index(style: crate::config::StatusIndicatorStyle) -> usize {
    usize::from(style == crate::config::StatusIndicatorStyle::Symbols)
}

fn toast_index(delivery: crate::config::ToastDelivery) -> usize {
    match delivery {
        crate::config::ToastDelivery::Off => 0,
        crate::config::ToastDelivery::Herdr => 1,
        crate::config::ToastDelivery::Terminal => 2,
        crate::config::ToastDelivery::System => 3,
    }
}

pub(super) fn integration_needs_install(info: &crate::api::schema::IntegrationInfo) -> bool {
    info.state == crate::api::schema::IntegrationState::Outdated
        || info.available && info.state == crate::api::schema::IntegrationState::NotInstalled
}

pub(super) fn background_tracking_enabled(snapshot: Option<&ClientShellSnapshot>) -> Option<bool> {
    let commands = &snapshot?.commands;
    if commands
        .iter()
        .any(|command| command.command_id == "tracking.disable")
    {
        Some(true)
    } else if commands
        .iter()
        .any(|command| command.command_id == "tracking.enable")
    {
        Some(false)
    } else {
        None
    }
}

pub(super) fn unread_suspend_delay(
    snapshot: Option<&ClientShellSnapshot>,
) -> Option<(&str, &'static str)> {
    snapshot?.commands.iter().find_map(|command| {
        let value = command.command_id.strip_prefix("unread-suspend.delay.")?;
        let label = match value {
            "0" => "Immediate",
            "3600" => "1h",
            "14400" => "4h",
            "28800" => "8h",
            "86400" => "24h (default)",
            "172800" => "48h",
            _ => return None,
        };
        Some((command.command_id.as_str(), label))
    })
}

impl ClientShellState {
    fn confirm_background_tracking(
        &mut self,
        enable: bool,
        endpoint: &ClientEndpointId,
        outcome: &mut ClientShellInput,
    ) {
        let expected = Some(!enable);
        if endpoint != &self.active_endpoint_id
            || background_tracking_enabled(self.snapshot.as_deref()) != expected
        {
            if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                settings.tracking_confirmation = None;
                settings.tracking_message = Some(
                    "Tracking availability changed. Review the current state and try again.".into(),
                );
            }
            outcome.repaint = true;
            return;
        }
        let params = crate::api::schema::CommandInvokeParams {
            command_id: if enable {
                "tracking.enable"
            } else {
                "tracking.disable"
            }
            .into(),
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            selection: None,
        };
        let submitted = self.push_endpoint_method_with_kind(
            crate::api::schema::Method::CommandInvoke(params),
            PendingEndpointKind::BackgroundTracking,
            outcome,
        );
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.tracking_confirmation = None;
            settings.tracking_pending = submitted;
            settings.tracking_message = Some(
                if submitted {
                    "Updating tracking hooks…"
                } else {
                    "Tracking update could not be sent to this machine."
                }
                .into(),
            );
        }
        outcome.repaint = true;
    }

    pub(super) fn open_settings_overlay(&mut self) {
        self.overlay = Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            customization: self.config.preferences.customization.clone(),
            tracking_confirmation: None,
            tracking_pending: false,
            tracking_message: None,
            section: ClientSettingsSection::Theme,
            selected: theme_index(&self.config.theme_name),
            original_theme_name: self.config.theme_name.clone(),
            original_palette: self.config.palette.clone(),
            integrations: Vec::new(),
            integration_messages: Vec::new(),
            loading_integrations: false,
            installing_integrations: false,
        }));
    }

    fn selected_index_for_settings_section(&self, section: ClientSettingsSection) -> usize {
        match section {
            ClientSettingsSection::Theme => theme_index(&self.config.theme_name),
            ClientSettingsSection::Indicators => indicator_index(self.config.status_indicators),
            ClientSettingsSection::Sound => usize::from(!self.config.sound_enabled),
            ClientSettingsSection::Toast => toast_index(self.config.toast_delivery),
            ClientSettingsSection::Integrations | ClientSettingsSection::Customization => 0,
        }
    }

    pub(super) fn select_settings_section(
        &mut self,
        section: ClientSettingsSection,
        outcome: &mut ClientShellInput,
    ) {
        let selected = self.selected_index_for_settings_section(section);
        let request_integrations = matches!(section, ClientSettingsSection::Integrations)
            && matches!(
                self.overlay,
                Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
                    loading_integrations: false,
                    installing_integrations: false,
                    ..
                }))
            );
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.tracking_confirmation = None;
            settings.section = section;
            settings.selected = selected;
        }
        if request_integrations {
            self.queue_integration_list(outcome, true);
        }
        outcome.repaint = true;
    }

    fn move_settings_section(&mut self, delta: isize, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_ref() else {
            return;
        };
        let current = ClientSettingsSection::ALL
            .iter()
            .position(|section| *section == settings.section)
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(ClientSettingsSection::ALL.len() as isize)
            as usize;
        self.select_settings_section(ClientSettingsSection::ALL[next], outcome);
    }

    fn settings_choice_count(&self) -> usize {
        match self.overlay.as_ref() {
            Some(ClientShellOverlay::Settings(settings)) => match settings.section {
                ClientSettingsSection::Theme => crate::config::THEME_NAMES.len(),
                ClientSettingsSection::Indicators | ClientSettingsSection::Sound => 2,
                ClientSettingsSection::Toast => 4,
                ClientSettingsSection::Customization => 21,
                ClientSettingsSection::Integrations => settings.integrations.len(),
            },
            _ => 0,
        }
    }

    pub(super) fn move_settings_selection(&mut self, delta: isize) {
        let count = self.settings_choice_count();
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() else {
            return;
        };
        if count == 0 {
            settings.selected = 0;
            return;
        }
        settings.selected = (settings.selected as isize + delta)
            .clamp(0, count.saturating_sub(1) as isize) as usize;
        if settings.section == ClientSettingsSection::Theme {
            self.preview_selected_theme();
        }
    }

    pub(super) fn select_settings_choice(&mut self, index: usize) {
        let count = self.settings_choice_count();
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            if count > 0 {
                settings.selected = index.min(count - 1);
            }
        }
        if matches!(
            self.overlay,
            Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
                section: ClientSettingsSection::Theme,
                ..
            }))
        ) {
            self.preview_selected_theme();
        }
    }

    fn preview_selected_theme(&mut self) {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_ref() else {
            return;
        };
        let Some(name) = crate::config::THEME_NAMES.get(settings.selected) else {
            return;
        };
        self.config.theme_name = (*name).to_owned();
        self.config.palette =
            crate::app::client_palette_for_theme(&self.config.theme_runtime, name);
    }

    pub(super) fn cancel_settings_overlay(&mut self) {
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            if settings.tracking_confirmation.take().is_some() {
                return;
            }
        }
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.take() else {
            return;
        };
        self.config.theme_name = settings.original_theme_name;
        self.config.palette = settings.original_palette;
    }

    fn save_settings_edit(
        &mut self,
        edit: crate::config::ConfigEdit<'_>,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if let Err(error) = crate::config::write_edit(edit) {
            self.set_endpoint_error(error);
            outcome.repaint = true;
            return false;
        }
        self.reload_client_config();
        self.push_endpoint_method_with_kind(
            crate::api::schema::Method::ServerReloadConfig(
                crate::api::schema::EmptyParams::default(),
            ),
            PendingEndpointKind::ReloadConfig,
            outcome,
        );
        outcome.repaint = true;
        true
    }

    pub(super) fn apply_settings_choice(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_ref() else {
            return;
        };
        if settings.tracking_pending {
            return;
        }
        if let Some((enable, endpoint_id)) = settings.tracking_confirmation.clone() {
            self.confirm_background_tracking(enable, &endpoint_id, outcome);
            return;
        }
        let section = settings.section;
        let selected = settings.selected;
        match section {
            ClientSettingsSection::Theme => {
                let Some(name) = crate::config::THEME_NAMES.get(selected).copied() else {
                    return;
                };
                if self.save_settings_edit(crate::config::ConfigEdit::Theme(name), outcome) {
                    self.overlay = None;
                }
            }
            ClientSettingsSection::Indicators => {
                let style = if selected == 0 {
                    crate::config::StatusIndicatorStyle::Dots
                } else {
                    crate::config::StatusIndicatorStyle::Symbols
                };
                self.save_settings_edit(
                    crate::config::ConfigEdit::StatusIndicators(style),
                    outcome,
                );
            }
            ClientSettingsSection::Sound => {
                self.save_settings_edit(crate::config::ConfigEdit::Sound(selected == 0), outcome);
            }
            ClientSettingsSection::Toast => {
                let delivery = match selected {
                    0 => crate::config::ToastDelivery::Off,
                    1 => crate::config::ToastDelivery::Herdr,
                    2 => crate::config::ToastDelivery::Terminal,
                    _ => crate::config::ToastDelivery::System,
                };
                self.save_settings_edit(
                    crate::config::ConfigEdit::ToastDelivery(delivery),
                    outcome,
                );
            }
            ClientSettingsSection::Customization => {
                if selected == 19 {
                    let command =
                        unread_suspend_delay(self.snapshot.as_deref()).map(|(id, _)| id.to_owned());
                    let submitted = command.is_some_and(|command_id| {
                        self.push_endpoint_method_with_kind(
                            crate::api::schema::Method::CommandInvoke(
                                crate::api::schema::CommandInvokeParams {
                                    command_id,
                                    workspace_id: None,
                                    tab_id: None,
                                    pane_id: None,
                                    selection: None,
                                },
                            ),
                            PendingEndpointKind::UnreadSuspendDelay,
                            outcome,
                        )
                    });
                    if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                        settings.tracking_pending = submitted;
                        settings.tracking_message = Some(
                            if submitted {
                                "Updating unread suspension delay…"
                            } else {
                                "Unread suspension settings are unavailable on this machine."
                            }
                            .into(),
                        );
                    }
                    outcome.repaint = true;
                    return;
                }
                if selected == 8 {
                    let enabled = background_tracking_enabled(self.snapshot.as_deref());
                    if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                        settings.tracking_message = None;
                        if let Some(enabled) = enabled {
                            settings.tracking_confirmation =
                                Some((!enabled, self.active_endpoint_id.clone()));
                        } else {
                            settings.tracking_message = Some("Tracking is unavailable on this machine. Install or update its Claude/Codex integrations first.".into());
                        }
                    }
                    outcome.repaint = true;
                    return;
                }
                let customization = &mut self.config.preferences.customization;
                match selected {
                    20 => customization.show_workspace_tabs = !customization.show_workspace_tabs,
                    0 => customization.space_groups_enabled = !customization.space_groups_enabled,
                    1 => {
                        customization.show_selected_tab_name = !customization.show_selected_tab_name
                    }
                    2 => {
                        customization.selected_tab_name_as_workspace =
                            !customization.selected_tab_name_as_workspace
                    }
                    3 => customization.colors.working = customization.colors.working.next(),
                    4 => customization.colors.subagents = customization.colors.subagents.next(),
                    5 => {
                        customization.colors.background_tasks =
                            customization.colors.background_tasks.next()
                    }
                    6 => customization.colors.completion = customization.colors.completion.next(),
                    7 => *customization = preferences::CustomizationPreferences::default(),
                    9 => {
                        customization.rename_workspace_with_tab =
                            !customization.rename_workspace_with_tab
                    }
                    10 => {
                        customization.show_agent_status_dividers =
                            !customization.show_agent_status_dividers
                    }
                    11 => {
                        customization.show_workspace_activity =
                            !customization.show_workspace_activity
                    }
                    12 => customization.show_agent_activity = !customization.show_agent_activity,
                    13 => {
                        customization.include_agent_shell_tasks =
                            !customization.include_agent_shell_tasks
                    }
                    14 => {
                        customization.include_terminal_shell_tasks =
                            !customization.include_terminal_shell_tasks
                    }
                    15 => {
                        customization.highlight_workspace_activity =
                            !customization.highlight_workspace_activity
                    }
                    16 => {
                        customization.colors.workspace_activity =
                            customization.colors.workspace_activity.next()
                    }
                    17 => customization.automatic_tab_names = !customization.automatic_tab_names,
                    18 => {
                        customization.automatic_workspace_names =
                            !customization.automatic_workspace_names
                    }
                    _ => return,
                }
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    settings.customization = customization.clone();
                }
                if matches!(selected, 7 | 17 | 18) {
                    self.refresh_automatic_names();
                }
                self.reveal_focused_workspace = true;
                self.persist_chrome_preferences(outcome);
                outcome.repaint = true;
            }
            ClientSettingsSection::Integrations => self.install_recommended_integrations(outcome),
        }
    }

    fn queue_integration_list(&mut self, outcome: &mut ClientShellInput, clear_messages: bool) {
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.loading_integrations = true;
            if clear_messages {
                settings.integration_messages.clear();
            }
        }
        if !self.push_endpoint_method_with_kind(
            crate::api::schema::Method::IntegrationList(crate::api::schema::EmptyParams::default()),
            PendingEndpointKind::IntegrationList,
            outcome,
        ) {
            if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                settings.loading_integrations = false;
            }
        }
    }

    fn install_recommended_integrations(&mut self, outcome: &mut ClientShellInput) {
        if self.pending_integration_installs > 0 {
            return;
        }
        let targets = match self.overlay.as_ref() {
            Some(ClientShellOverlay::Settings(settings)) => settings
                .integrations
                .iter()
                .filter(|integration| integration_needs_install(integration))
                .map(|integration| integration.target)
                .collect::<Vec<_>>(),
            _ => return,
        };
        if targets.is_empty() {
            return;
        }
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.installing_integrations = true;
            settings.integration_messages.clear();
        }
        self.pending_integration_installs = 0;
        for target in targets {
            if self.push_endpoint_method_with_kind(
                crate::api::schema::Method::IntegrationInstall(
                    crate::api::schema::IntegrationInstallParams { target },
                ),
                PendingEndpointKind::IntegrationInstall,
                outcome,
            ) {
                self.pending_integration_installs += 1;
            }
        }
        if self.pending_integration_installs == 0 {
            if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                settings.installing_integrations = false;
            }
        }
        outcome.repaint = true;
    }

    pub(super) fn handle_settings_endpoint_result(
        &mut self,
        kind: PendingEndpointKind,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        match kind {
            PendingEndpointKind::UnreadSuspendDelay => {
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    settings.tracking_pending = false;
                    settings.tracking_message = Some(match result {
                        Ok(crate::api::schema::ResponseResult::Ok {}) => {
                            "Unread suspension delay saved for this machine.".into()
                        }
                        Ok(_) => "Machine returned an unexpected settings response.".into(),
                        Err(error) => format!("Unread suspension update failed: {}", error.message),
                    });
                }
                (true, Vec::new())
            }
            PendingEndpointKind::BackgroundTracking => {
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    settings.tracking_pending = false;
                    settings.tracking_message = Some(match result {
                        Ok(_) => "Tracking hooks updated for this machine. Restart running Claude/Codex sessions to load the change.".into(),
                        Err(error) => format!("Tracking update failed: {}", error.message),
                    });
                }
                (true, Vec::new())
            }
            PendingEndpointKind::IntegrationList => {
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    settings.loading_integrations = false;
                    match result {
                        Ok(crate::api::schema::ResponseResult::IntegrationList {
                            integrations,
                        }) => {
                            settings.integrations = integrations;
                            settings.selected = settings
                                .selected
                                .min(settings.integrations.len().saturating_sub(1));
                        }
                        Ok(_) => {
                            self.set_endpoint_error(
                                "endpoint returned an unexpected integration list result",
                            );
                        }
                        Err(_) => {}
                    }
                }
                (true, Vec::new())
            }
            PendingEndpointKind::IntegrationInstall => {
                let cancelled = result
                    .as_ref()
                    .is_err_and(|error| error.code.as_deref() == Some("endpoint_cancelled"));
                self.pending_integration_installs =
                    self.pending_integration_installs.saturating_sub(1);
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    match result {
                        Ok(crate::api::schema::ResponseResult::IntegrationInstall {
                            details,
                            ..
                        }) => settings.integration_messages.extend(details.messages),
                        Ok(_) => settings
                            .integration_messages
                            .push("endpoint returned an unexpected integration result".into()),
                        Err(error) => settings.integration_messages.push(error.message),
                    }
                    settings.installing_integrations = self.pending_integration_installs > 0;
                }
                let actions = if !cancelled
                    && self.pending_integration_installs == 0
                    && matches!(self.overlay, Some(ClientShellOverlay::Settings(_)))
                {
                    let mut deferred = ClientShellInput::default();
                    self.queue_integration_list(&mut deferred, false);
                    deferred.actions
                } else {
                    Vec::new()
                };
                (true, actions)
            }
            _ => (false, Vec::new()),
        }
    }

    pub(super) fn route_settings_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !matches!(self.overlay, Some(ClientShellOverlay::Settings(_))) {
            return false;
        }
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        if code == KeyCode::Esc {
            if !matches!(
                self.overlay,
                Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
                    installing_integrations: true,
                    ..
                }))
            ) {
                self.cancel_settings_overlay();
                outcome.repaint = true;
            }
            return true;
        }
        if matches!(code, KeyCode::Tab | KeyCode::Right | KeyCode::Char('l'))
            && modifiers.is_empty()
        {
            self.move_settings_section(1, outcome);
            return true;
        }
        if matches!(code, KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h'))
            && modifiers.difference(KeyModifiers::SHIFT).is_empty()
        {
            self.move_settings_section(-1, outcome);
            return true;
        }
        if matches!(code, KeyCode::Up | KeyCode::Char('k')) && modifiers.is_empty() {
            self.move_settings_selection(-1);
            outcome.repaint = true;
            return true;
        }
        if matches!(code, KeyCode::Down | KeyCode::Char('j')) && modifiers.is_empty() {
            self.move_settings_selection(1);
            outcome.repaint = true;
            return true;
        }
        if matches!(code, KeyCode::Enter | KeyCode::Char(' ')) && modifiers.is_empty() {
            self.apply_settings_choice(outcome);
            return true;
        }
        true
    }
}
