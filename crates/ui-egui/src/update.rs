//! The update dialog and the header's Update button state. The app owns the checks, downloads
//! and the install; this module only shows where that stands and reports what the user chose.

use egui::{Align, Layout, RichText, ScrollArea};

use crate::theme::Theme;
use crate::widgets;

/// Where the update flow stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdatePhase {
    /// Nothing checked yet in this session.
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// A newer release exists. `manual` says why it cannot be installed from here (a package
    /// manager install, a development build); `None` means the app can install it.
    Available {
        version: String,
        notes: String,
        page_url: String,
        manual: Option<String>,
    },
    Downloading {
        version: String,
    },
    /// Downloaded and verified; restarting installs it.
    Ready {
        version: String,
        verification: String,
    },
    Failed {
        message: String,
        page_url: Option<String>,
    },
}

/// Dialog state owned by the app.
#[derive(Debug, Clone, Default)]
pub struct UpdateState {
    pub open: bool,
    pub phase: UpdatePhase,
}

impl UpdateState {
    /// The header button for this phase: `(label, tooltip)`, or `None` when there is nothing
    /// to offer.
    pub fn header_button(&self) -> Option<(&'static str, String)> {
        match &self.phase {
            UpdatePhase::Available { version, .. } => Some(("Update", format!("Postguy {version} is available"))),
            UpdatePhase::Downloading { version } => Some(("Update", format!("Downloading Postguy {version}…"))),
            UpdatePhase::Ready { version, .. } => Some(("Restart to update", format!("Postguy {version} is ready to install"))),
            _ => None,
        }
    }

    /// One line for Settings > About.
    pub fn summary(&self) -> String {
        match &self.phase {
            UpdatePhase::Idle => "Not checked since Postguy started".into(),
            UpdatePhase::Checking => "Checking…".into(),
            UpdatePhase::UpToDate => "Postguy is up to date".into(),
            UpdatePhase::Available { version, .. } => format!("Postguy {version} is available"),
            UpdatePhase::Downloading { version } => format!("Downloading Postguy {version}…"),
            UpdatePhase::Ready { version, .. } => format!("Postguy {version} is ready; restart to install it"),
            UpdatePhase::Failed { message, .. } => message.clone(),
        }
    }
}

/// What the user chose in the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateAction {
    CheckAgain,
    /// Download, verify and stage the available release.
    Download,
    CancelDownload,
    /// Close the app, install the staged release and start it.
    Restart,
    OpenPage(String),
}

/// Draw the modal while `state.open`.
pub fn update_dialog(ctx: &egui::Context, state: &mut UpdateState, current_version: &str) -> Option<UpdateAction> {
    let theme = Theme::current(ctx);
    let palette = theme.palette;
    let mut action = None;
    let phase = state.phase.clone();
    widgets::modal(ctx, "update-dialog", "Software update", &mut state.open, |ui| {
        ui.set_min_width(440.0);
        let muted = |text: &str| RichText::new(text).color(palette.text_muted);
        match &phase {
            UpdatePhase::Idle | UpdatePhase::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking for updates…");
                });
            }
            UpdatePhase::UpToDate => {
                ui.label(format!("Postguy {current_version} is the newest version."));
            }
            UpdatePhase::Available { version, notes, page_url, manual } => {
                ui.label(RichText::new(format!("Postguy {version} is available")).strong());
                ui.label(muted(&format!("You have {current_version}.")));
                if !notes.trim().is_empty() {
                    ui.add_space(8.0);
                    ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                        ui.label(notes.as_str());
                    });
                }
                if let Some(manual) = manual {
                    ui.add_space(8.0);
                    ui.label(muted(manual));
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if manual.is_none()
                        && widgets::text_button(ui, widgets::ButtonKind::Primary, Some("download"), "Download and install", true).clicked()
                    {
                        action = Some(UpdateAction::Download);
                    }
                    if widgets::text_button(ui, widgets::ButtonKind::Secondary, None, "Release page", true).clicked() {
                        action = Some(UpdateAction::OpenPage(page_url.clone()));
                    }
                });
            }
            UpdatePhase::Downloading { version } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Downloading and verifying Postguy {version}…"));
                });
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::text_button(ui, widgets::ButtonKind::Secondary, None, "Cancel", true).clicked() {
                        action = Some(UpdateAction::CancelDownload);
                    }
                });
            }
            UpdatePhase::Ready { version, verification } => {
                ui.label(RichText::new(format!("Postguy {version} is ready to install")).strong());
                ui.label(muted(verification));
                ui.label(muted("Restarting saves your session, installs the update and opens Postguy again."));
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::text_button(ui, widgets::ButtonKind::Primary, None, "Restart to update", true).clicked() {
                        action = Some(UpdateAction::Restart);
                    }
                });
            }
            UpdatePhase::Failed { message, page_url } => {
                ui.label(RichText::new(message.as_str()).color(palette.danger));
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::text_button(ui, widgets::ButtonKind::Secondary, None, "Try again", true).clicked() {
                        action = Some(UpdateAction::CheckAgain);
                    }
                    if let Some(page_url) = page_url
                        && widgets::text_button(ui, widgets::ButtonKind::Secondary, None, "Release page", true).clicked()
                    {
                        action = Some(UpdateAction::OpenPage(page_url.clone()));
                    }
                });
            }
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_button_appears_only_when_there_is_something_to_install() {
        let mut state = UpdateState::default();
        for phase in
            [UpdatePhase::Idle, UpdatePhase::Checking, UpdatePhase::UpToDate, UpdatePhase::Failed { message: "offline".into(), page_url: None }]
        {
            state.phase = phase;
            assert!(state.header_button().is_none(), "{:?}", state.phase);
        }
        state.phase = UpdatePhase::Available { version: "1.2.0".into(), notes: String::new(), page_url: String::new(), manual: None };
        assert_eq!(state.header_button().map(|(label, _)| label), Some("Update"));
        state.phase = UpdatePhase::Ready { version: "1.2.0".into(), verification: String::new() };
        assert_eq!(state.header_button().map(|(label, _)| label), Some("Restart to update"));
    }
}
