//! The AI Agent pane through the real shell: the bottom-bar toggle, example prompts and Send.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_ui_egui::{AgentAction, AgentRow, PostguyShell};

struct AgentHarness {
    shell: PostguyShell,
    /// The last pane action (the shell clears its own field every frame).
    last: Option<AgentAction>,
}

fn harness() -> Harness<'static, AgentHarness> {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut AgentHarness| {
            state.shell.ui(ui);
            if let Some(action) = state.shell.agent_action.take() {
                state.last = Some(action);
            }
        },
        AgentHarness { shell: PostguyShell::new("demo"), last: None },
    );
    harness.set_size(egui::vec2(1400.0, 900.0));
    harness.run();
    harness
}

#[test]
fn the_bottom_bar_opens_the_agent_panel_and_examples_fill_the_input() {
    let mut harness = harness();
    assert!(!harness.state().shell.state.agent_open);
    harness.get_by_label("AI Agent").click();
    harness.run();
    assert!(harness.state().shell.state.agent_open);

    let example = "Tidy up collection \"A\"".to_string();
    harness.state_mut().shell.agent.examples = vec![example.clone()];
    harness.run();
    harness.get_by_label(&example).click();
    harness.run();
    assert_eq!(harness.state().shell.agent.input, example, "an example fills the box and is not sent");
    assert_eq!(harness.state().last, None);

    harness.get_by_label("Send").click();
    harness.run();
    assert_eq!(harness.state().last, Some(AgentAction::Send(example)));
}

#[test]
fn a_pending_approval_offers_approve_and_decline() {
    let mut harness = harness();
    harness.state_mut().shell.state.agent_open = true;
    harness.state_mut().shell.agent.rows = vec![AgentRow::Approval { id: 7, summary: "Delete collection “Shop”".into(), decision: None }];
    harness.state_mut().shell.agent.busy = Some("Waiting for approval".into());
    // `run` fails on a frame that keeps asking for repaints: a waiting approval must not animate.
    harness.run();
    harness.get_by_label("Decline").click();
    harness.run();
    assert_eq!(harness.state().last, Some(AgentAction::Decline(7)));
    assert!(harness.query_by_label("Stop").is_some(), "a running turn can be stopped");
}
