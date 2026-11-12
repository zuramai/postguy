//! The AI Agent pane: the chat, example prompts from the user's collections and the message
//! box. The app owns the agent; this module only draws [`AgentPanelState`] and reports an
//! [`AgentAction`].

use egui::{Align, Id, Key, Layout, Margin, Modifiers, ScrollArea, Ui, vec2};

use crate::theme::Theme;
use crate::widgets::{self, ButtonKind, ButtonOptions};

/// Height kept under the messages for the examples row, the message box and the buttons.
const INPUT_AREA_HEIGHT: f32 = 170.0;
const LINK_PREFIX: &str = "postguy://run/";

/// One row of the chat as the pane shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRow {
    User(String),
    Assistant(String),
    /// A tool the agent ran.
    Step {
        label: String,
        ok: bool,
    },
    /// A destructive step waiting for (or given) the user's answer.
    Approval {
        id: u64,
        summary: String,
        decision: Option<bool>,
    },
    Notice {
        text: String,
        error: bool,
    },
}

/// What the user did in the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAction {
    Send(String),
    Stop,
    NewChat,
    Approve(u64),
    Decline(u64),
    /// A run link in a reply: open the run in the Runner.
    OpenRun {
        collection: String,
        run: String,
    },
    OpenSettings,
}

/// The pane's state; the app fills `rows`, `busy`, `header`, `configured` and `examples`.
#[derive(Debug, Clone, Default)]
pub struct AgentPanelState {
    pub input: String,
    pub examples: Vec<String>,
    pub rows: Vec<AgentRow>,
    /// What the agent is doing while a turn runs.
    pub busy: Option<String>,
    /// Provider and model, under the title.
    pub header: String,
    /// A model is set (the empty chat offers the settings otherwise).
    pub configured: bool,
    /// Focus the message box on the next frame.
    pub focus_input: bool,
}

/// A piece of an assistant reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextPart {
    Text(String),
    Code(String),
    RunLink { collection: String, run: String },
}

/// Split a reply into prose, fenced code blocks and `postguy://run/<collection>/<run>` links.
/// A fence's language tag is dropped; a malformed link stays text. Total on any input.
pub fn split_reply(text: &str) -> Vec<TextPart> {
    let mut parts = Vec::new();
    let mut prose = String::new();
    let mut code: Option<String> = None;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            match code.take() {
                Some(block) => parts.push(TextPart::Code(block.trim_end_matches('\n').to_string())),
                None => {
                    push_prose(&mut parts, &prose);
                    prose.clear();
                    code = Some(String::new());
                }
            }
            continue;
        }
        match code.as_mut() {
            Some(block) => block.push_str(line),
            None => prose.push_str(line),
        }
    }
    match code {
        Some(block) => parts.push(TextPart::Code(block.trim_end_matches('\n').to_string())),
        None => push_prose(&mut parts, &prose),
    }
    // Blank lines next to a code block or a link are layout, not text.
    parts
        .into_iter()
        .filter_map(|part| match part {
            TextPart::Text(text) => {
                let text = text.trim_matches('\n');
                (!text.is_empty()).then(|| TextPart::Text(text.to_string()))
            }
            other => Some(other),
        })
        .collect()
}

fn push_text(parts: &mut Vec<TextPart>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(TextPart::Text(last)) = parts.last_mut() {
        last.push_str(text);
    } else {
        parts.push(TextPart::Text(text.to_string()));
    }
}

/// `[A-Za-z0-9_-]+` at the start of `text`, and the rest.
fn take_id(text: &str) -> (&str, &str) {
    let end = text.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(text.len());
    (text.get(..end).unwrap_or_default(), text.get(end..).unwrap_or_default())
}

fn push_prose(parts: &mut Vec<TextPart>, text: &str) {
    let mut rest = text;
    while let Some(at) = rest.find(LINK_PREFIX) {
        let before = rest.get(..at).unwrap_or_default();
        let after = rest.get(at + LINK_PREFIX.len()..).unwrap_or_default();
        let (collection, tail) = take_id(after);
        let (run, tail) = match tail.strip_prefix('/') {
            Some(tail) if !collection.is_empty() => take_id(tail),
            _ => ("", tail),
        };
        if run.is_empty() {
            push_text(parts, rest.get(..at + LINK_PREFIX.len()).unwrap_or_default());
            rest = after;
            continue;
        }
        push_text(parts, before);
        parts.push(TextPart::RunLink { collection: collection.to_string(), run: run.to_string() });
        rest = tail;
    }
    push_text(parts, rest);
}

/// Draw the pane; returns what the user did this frame.
pub fn agent_panel(ui: &mut Ui, state: &mut AgentPanelState) -> Option<AgentAction> {
    let theme = Theme::current(ui.ctx());
    let palette = theme.palette;
    let busy = state.busy.is_some();
    let mut action = None;

    ui.horizontal(|ui| {
        let (icon, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
        crate::icons::paint(ui.painter(), icon, "bot", palette.text, 1.5);
        ui.vertical(|ui| {
            widgets::plain_text(ui, "AI Agent", theme.semibold_font(14.0), palette.text);
            if !state.header.is_empty() {
                widgets::plain_text(ui, &state.header, egui::TextStyle::Small.resolve(ui.style()), palette.text_muted);
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if widgets::icon_button(ui, "plus", "New chat").clicked() {
                    action = Some(AgentAction::NewChat);
                }
            });
        });
    });
    ui.add_space(6.0);
    ui.separator();

    let messages_height = (ui.available_height() - INPUT_AREA_HEIGHT).max(80.0);
    ScrollArea::vertical().id_salt("agent-messages").stick_to_bottom(true).auto_shrink([false, false]).max_height(messages_height).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        if state.rows.is_empty() && !busy {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Ask the agent to create, edit, run or export your collections, requests and environments.")
                    .color(palette.text_muted),
            );
            if !state.configured {
                ui.add_space(8.0);
                if widgets::text_button(ui, ButtonKind::Primary, Some("settings"), "Set up AI Agent", true).clicked() {
                    action = Some(AgentAction::OpenSettings);
                }
            }
        }
        for row in &state.rows {
            if let Some(row_action) = draw_row(ui, &theme, row) {
                action = Some(row_action);
            }
        }
        if let Some(label) = &state.busy {
            // A pending approval waits on the user, not the agent: no spinner, so no per-frame repaint.
            let waiting = state.rows.iter().any(|row| matches!(row, AgentRow::Approval { decision: None, .. }));
            ui.horizontal(|ui| {
                if !waiting {
                    ui.spinner();
                }
                ui.label(egui::RichText::new(label).small().color(palette.text_muted));
            });
        }
    });

    ui.add_space(8.0);
    ScrollArea::horizontal().id_salt("agent-examples").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
        // A plain mouse wheel scrolls the row sideways while the pointer is over it.
        if ui.rect_contains_pointer(ui.clip_rect()) {
            let delta = ui.input(|input| input.smooth_scroll_delta);
            if delta.x == 0.0 && delta.y != 0.0 {
                ui.scroll_with_delta(vec2(delta.y, 0.0));
            }
        }
        ui.horizontal(|ui| {
            let options = ButtonOptions { small: true, ..ButtonOptions::default() };
            for example in &state.examples {
                if widgets::text_button_with(ui, ButtonKind::Secondary, None, example, !busy, options).clicked() {
                    state.input = example.clone();
                    state.focus_input = true;
                }
            }
        });
    });

    ui.add_space(6.0);
    let input_id = Id::new("agent-input");
    let has_focus = ui.memory(|memory| memory.has_focus(input_id));
    let sendable = !busy && !state.input.trim().is_empty();
    if has_focus
        && sendable
        && ui.input_mut(|input| !input.modifiers.shift && !input.modifiers.command && input.consume_key(Modifiers::NONE, Key::Enter))
    {
        action = Some(AgentAction::Send(state.input.trim().to_string()));
    }
    let response = ui.add(
        egui::TextEdit::multiline(&mut state.input)
            .id(input_id)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .margin(Margin::symmetric(8, 6))
            .hint_text("Ask the AI Agent… (Enter to send, Shift+Enter for a new line)"),
    );
    if state.focus_input {
        response.request_focus();
        state.focus_input = false;
    }
    ui.add_space(6.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if busy {
            if widgets::text_button(ui, ButtonKind::Secondary, Some("x"), "Stop", true).clicked() {
                action = Some(AgentAction::Stop);
            }
        } else if widgets::text_button(ui, ButtonKind::Primary, Some("send"), "Send", !state.input.trim().is_empty()).clicked() {
            action = Some(AgentAction::Send(state.input.trim().to_string()));
        }
    });
    action
}

fn draw_row(ui: &mut Ui, theme: &Theme, row: &AgentRow) -> Option<AgentAction> {
    let palette = theme.palette;
    let mut action = None;
    match row {
        AgentRow::User(text) => {
            let max_width = ui.available_width() * 0.85;
            ui.with_layout(Layout::top_down(Align::Max), |ui| {
                egui::Frame::NONE.fill(palette.surface_alt).corner_radius(8).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                    ui.set_max_width(max_width);
                    ui.add(egui::Label::new(egui::RichText::new(text).color(palette.text)).wrap().selectable(true));
                });
            });
        }
        AgentRow::Assistant(text) => {
            for part in split_reply(text) {
                match part {
                    TextPart::Text(text) => {
                        ui.add(egui::Label::new(egui::RichText::new(text).color(palette.text)).wrap().selectable(true));
                    }
                    TextPart::Code(code) => {
                        egui::Frame::NONE.fill(palette.surface_sunken).corner_radius(6).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(egui::Label::new(egui::RichText::new(code).monospace().color(palette.text)).wrap().selectable(true));
                        });
                    }
                    TextPart::RunLink { collection, run } => {
                        if ui.link("Open run in Runner").clicked() {
                            action = Some(AgentAction::OpenRun { collection, run });
                        }
                    }
                }
            }
        }
        AgentRow::Step { label, ok } => {
            ui.horizontal(|ui| {
                let (icon, color) = if *ok { ("check", palette.success) } else { ("x", palette.danger) };
                let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
                crate::icons::paint(ui.painter(), rect, icon, color, 1.5);
                ui.add(egui::Label::new(egui::RichText::new(label).small().color(palette.text_muted)).wrap());
            });
        }
        AgentRow::Approval { id, summary, decision } => match decision {
            None => {
                egui::Frame::NONE.stroke(egui::Stroke::new(1.0, palette.warning)).corner_radius(8).inner_margin(Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(egui::RichText::new(summary).color(palette.text)).wrap());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if widgets::text_button(ui, ButtonKind::Primary, None, "Approve", true).clicked() {
                            action = Some(AgentAction::Approve(*id));
                        }
                        if widgets::text_button(ui, ButtonKind::Secondary, None, "Decline", true).clicked() {
                            action = Some(AgentAction::Decline(*id));
                        }
                    });
                });
            }
            Some(approved) => {
                let verdict = if *approved { "Approved" } else { "Declined" };
                ui.add(egui::Label::new(egui::RichText::new(format!("{verdict}: {summary}")).small().color(palette.text_muted)).wrap());
            }
        },
        AgentRow::Notice { text, error } => {
            let color = if *error { palette.danger } else { palette.text_muted };
            ui.add(egui::Label::new(egui::RichText::new(text).small().color(color)).wrap().selectable(true));
        }
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> TextPart {
        TextPart::Text(value.to_string())
    }

    fn link(collection: &str, run: &str) -> TextPart {
        TextPart::RunLink { collection: collection.to_string(), run: run.to_string() }
    }

    #[test]
    fn split_reply_finds_run_links_and_code_blocks() {
        assert_eq!(split_reply("Run done: postguy://run/c-1/0001-ab"), vec![text("Run done: "), link("c-1", "0001-ab")]);
        assert_eq!(split_reply("See postguy://run/c_1/r1."), vec![text("See "), link("c_1", "r1"), text(".")]);
        assert_eq!(split_reply("bad postguy://run/ and postguy://run/c/"), vec![text("bad postguy://run/ and postguy://run/c/")]);
        assert_eq!(
            split_reply("Here:\n```json\n{\"a\": 1}\n```\nafter"),
            vec![text("Here:"), TextPart::Code("{\"a\": 1}".to_string()), text("after")]
        );
        assert_eq!(split_reply("é postguy://run/a/b ü"), vec![text("é "), link("a", "b"), text(" ü")]);
        assert_eq!(split_reply("```\nunclosed"), vec![TextPart::Code("unclosed".to_string())]);
        assert_eq!(split_reply(""), Vec::<TextPart>::new());
    }
}
