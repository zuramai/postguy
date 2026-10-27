//! Resizing by dragging: the sidebar edge, the console's top edge and the request/response
//! divider.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_ui_egui::PostguyShell;
use postguy_ui_egui::widgets::{SPLIT_GAP, SplitAxis, split};

/// Press at `from`, move to `to` over a few frames, and release there.
fn drag<State>(harness: &mut Harness<'_, State>, from: egui::Pos2, to: egui::Pos2) {
    harness.hover_at(from);
    harness.step();
    harness.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    harness.step();
    for step in 1..=4 {
        harness.hover_at(from.lerp(to, step as f32 / 4.0));
        harness.step();
    }
    harness.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    harness.run_steps(3);
}

fn shell_harness(console_open: bool) -> Harness<'static, PostguyShell> {
    let mut shell = PostguyShell::new("demo");
    shell.state.console_open = console_open;
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(|ui, shell: &mut PostguyShell| shell.ui(ui), shell);
    harness.run_steps(2);
    harness
}

#[test]
fn the_sidebar_edge_drags_from_a_few_points_away() {
    let mut harness = shell_harness(false);
    let before = harness.get_by_label("COLLECTIONS").rect();
    // The panel edge sits 8 px (the inner margin) past the section header; grab 9 px inside
    // it. The edge then follows the pointer, so it ends 9 px short of the 120 px move.
    let grab = egui::pos2(before.right() - 1.0, 500.0);
    drag(&mut harness, grab, grab + egui::vec2(120.0, 0.0));
    let grown = harness.get_by_label("COLLECTIONS").rect();
    assert!((grown.width() - before.width() - 111.0).abs() < 2.0, "sidebar width went from {} to {}", before.width(), grown.width());

    let grab = egui::pos2(grown.right() + 17.0, 500.0);
    drag(&mut harness, grab, grab - egui::vec2(160.0, 0.0));
    let shrunk = harness.get_by_label("COLLECTIONS").rect().width();
    assert!((grown.width() - shrunk - 151.0).abs() < 2.0, "sidebar width went from {} to {shrunk}", grown.width());
}

#[test]
fn the_console_top_edge_drags_its_height() {
    let mut harness = shell_harness(true);
    let before = harness.get_by_label("Clear").rect().top();
    // The grab band straddles the panel's top edge; its middle is the edge itself.
    let band: Vec<f32> = (0..80)
        .map(|offset| before + 10.0 - offset as f32)
        .filter(|y| {
            harness.hover_at(egui::pos2(700.0, *y));
            harness.step();
            harness.output().platform_output.cursor_icon == egui::CursorIcon::ResizeVertical
        })
        .collect();
    let (Some(low), Some(high)) = (band.first(), band.last()) else { panic!("the console's top edge offers no resize cursor") };
    let edge = egui::pos2(700.0, (low + high) / 2.0);
    // Stretching the console only works if the panel is not pinned to one height.
    drag(&mut harness, edge, edge - egui::vec2(0.0, 150.0));
    let after = harness.get_by_label("Clear").rect().top();
    assert!((before - after - 150.0).abs() < 2.0, "the console toolbar moved from {before} to {after}");
}

#[test]
fn the_snippet_panel_keeps_its_width_with_a_long_url() {
    let mut shell = PostguyShell::new("demo");
    shell.state.right_panel_open = true;
    let url = format!("https://api.example.com/{}", "segment/".repeat(80));
    shell.snippet_request = Some(postguy_model::HttpRequest { url, ..postguy_model::HttpRequest::default() });
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(|ui, shell: &mut PostguyShell| shell.ui(ui), shell);
    harness.run_steps(2);
    let left = harness.get_by_label("Code snippet").rect().left();
    harness.run_steps(20);
    let after = harness.get_by_label("Code snippet").rect().left();
    assert!((after - left).abs() < 1.0, "the snippet panel's left edge moved from {left} to {after}");
    assert!(left > 800.0, "a long line does not widen the panel past its default (left edge at {left})");
}

struct SplitState {
    axis: SplitAxis,
    first: egui::Rect,
    second: egui::Rect,
}

fn split_harness(axis: SplitAxis) -> Harness<'static, SplitState> {
    let state = SplitState { axis, first: egui::Rect::NOTHING, second: egui::Rect::NOTHING };
    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 800.0)).build_ui_state(
        |ui, state: &mut SplitState| {
            let (first, second) = split(ui, egui::Id::new("test-split"), state.axis, 0.5, 200.0, 150.0);
            state.first = first;
            state.second = second;
        },
        state,
    );
    harness.run_steps(2);
    harness
}

#[test]
fn the_stacked_divider_drags_between_the_minimums() {
    let mut harness = split_harness(SplitAxis::Vertical);
    let first = harness.state().first;
    let total = harness.state().second.bottom() - first.top() - SPLIT_GAP;
    assert!((first.height() - total * 0.5).abs() < 1.0, "starts at the default fraction");
    let grab = egui::pos2(500.0, first.bottom() + SPLIT_GAP / 2.0);

    drag(&mut harness, grab, grab + egui::vec2(0.0, 100.0));
    let state = harness.state();
    assert!((state.first.height() - (total * 0.5 + 100.0)).abs() < 2.0, "first pane height {}", state.first.height());
    assert!((state.second.top() - state.first.bottom() - SPLIT_GAP).abs() < 0.5, "the panes keep the gap");

    // Dragging past the bottom stops at the response's minimum.
    let grab = egui::pos2(500.0, state.first.bottom() + SPLIT_GAP / 2.0);
    drag(&mut harness, grab, egui::pos2(500.0, 2000.0));
    assert!((harness.state().second.height() - 150.0).abs() < 1.0, "second pane height {}", harness.state().second.height());
}

#[test]
fn the_side_by_side_divider_drags_and_stops_at_the_request_minimum() {
    let mut harness = split_harness(SplitAxis::Horizontal);
    let grab = egui::pos2(harness.state().first.right() + SPLIT_GAP / 2.0, 400.0);
    drag(&mut harness, grab, egui::pos2(-500.0, 400.0));
    assert!((harness.state().first.width() - 200.0).abs() < 1.0, "first pane width {}", harness.state().first.width());
}
