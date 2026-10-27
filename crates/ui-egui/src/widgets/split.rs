//! Two panes with a draggable divider between them (the request builder above or beside its
//! response). The divider's position is a fraction kept in egui memory, like panel widths, so
//! it follows window resizes and survives tab switches within a run.

use egui::{CursorIcon, Id, Rect, Sense, Stroke, Ui, pos2};

use crate::theme::Theme;

/// Thickness of the gap between the panes. The divider line runs down its middle and the whole
/// gap is the grab area.
pub const SPLIT_GAP: f32 = 9.0;

/// Which way the panes sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    /// First pane on the left, second on the right; the divider is vertical.
    Horizontal,
    /// First pane on top, second below; the divider is horizontal.
    Vertical,
}

/// Divide the rest of `ui`'s available area into two panes with a draggable divider. The divider
/// starts at `default_fraction` of the room and never squeezes a pane below its minimum while
/// there is room for both. Returns the two pane rects; the caller fills each (clipped to it) and
/// then advances the cursor past `ui.available_rect_before_wrap()`.
pub fn split(ui: &mut Ui, id: Id, axis: SplitAxis, default_fraction: f32, min_first: f32, min_second: f32) -> (Rect, Rect) {
    let area = ui.available_rect_before_wrap();
    let range = match axis {
        SplitAxis::Horizontal => area.x_range(),
        SplitAxis::Vertical => area.y_range(),
    };
    let total = (range.span() - SPLIT_GAP).max(0.0);
    let stored = ui.data(|data| data.get_temp::<f32>(id)).unwrap_or(default_fraction);
    let mut first = first_size(total, stored, min_first, min_second);

    let gap_rect = |first: f32| {
        let start = range.min + first;
        match axis {
            SplitAxis::Horizontal => Rect::from_x_y_ranges(start..=start + SPLIT_GAP, area.y_range()),
            SplitAxis::Vertical => Rect::from_x_y_ranges(area.x_range(), start..=start + SPLIT_GAP),
        }
    };
    let response = ui.interact(gap_rect(first), id, Sense::drag());
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
        && total > 0.0
    {
        let along = match axis {
            SplitAxis::Horizontal => pointer.x,
            SplitAxis::Vertical => pointer.y,
        };
        first = first_size(total, (along - range.min - SPLIT_GAP / 2.0) / total, min_first, min_second);
        ui.data_mut(|data| data.insert_temp(id, first / total));
    }
    let active = response.hovered() || response.dragged();
    let gap = gap_rect(first);
    let stroke = if active { Stroke::new(2.0, Theme::current(ui.ctx()).palette.accent) } else { ui.visuals().widgets.noninteractive.bg_stroke };
    let painter = ui.painter_at(gap);
    let _ = match axis {
        SplitAxis::Horizontal => painter.vline(gap.center().x, gap.y_range(), stroke),
        SplitAxis::Vertical => painter.hline(gap.x_range(), gap.center().y, stroke),
    };
    response.on_hover_and_drag_cursor(match axis {
        SplitAxis::Horizontal => CursorIcon::ResizeHorizontal,
        SplitAxis::Vertical => CursorIcon::ResizeVertical,
    });

    match axis {
        SplitAxis::Horizontal => {
            (Rect::from_min_max(area.min, pos2(gap.left(), area.bottom())), Rect::from_min_max(pos2(gap.right(), area.top()), area.max))
        }
        SplitAxis::Vertical => {
            (Rect::from_min_max(area.min, pos2(area.right(), gap.top())), Rect::from_min_max(pos2(area.left(), gap.bottom()), area.max))
        }
    }
}

/// Size of the first pane out of `total` (the room without the gap) for `fraction`. Each pane
/// keeps its minimum while both fit; when they don't, the second pane's minimum wins so the
/// response stays visible. Never negative and never more than `total`, whatever the input.
pub fn first_size(total: f32, fraction: f32, min_first: f32, min_second: f32) -> f32 {
    // `is_finite` also rejects NaN.
    if !total.is_finite() || total <= 0.0 {
        return 0.0;
    }
    let fraction = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.5 };
    let upper = (total - min_second.max(0.0)).max(0.0);
    let lower = min_first.max(0.0).min(upper);
    (total * fraction).max(lower).min(upper)
}

#[cfg(test)]
mod tests {
    use super::first_size;

    #[test]
    fn the_first_pane_follows_the_fraction_within_both_minimums() {
        assert_eq!(first_size(1000.0, 0.5, 100.0, 100.0), 500.0);
        assert_eq!(first_size(1000.0, 0.05, 100.0, 100.0), 100.0, "the first pane keeps its minimum");
        assert_eq!(first_size(1000.0, 0.95, 100.0, 100.0), 900.0, "the second pane keeps its minimum");
    }

    #[test]
    fn a_cramped_or_hostile_layout_stays_inside_the_room() {
        assert_eq!(first_size(150.0, 0.5, 100.0, 100.0), 50.0, "too small for both: the second pane's minimum wins");
        assert_eq!(first_size(50.0, 0.5, 100.0, 100.0), 0.0);
        assert_eq!(first_size(0.0, 0.5, 100.0, 100.0), 0.0);
        assert_eq!(first_size(f32::NAN, 0.5, 10.0, 10.0), 0.0);
        assert_eq!(first_size(f32::INFINITY, 0.5, 10.0, 10.0), 0.0);
        assert_eq!(first_size(1000.0, f32::NAN, 0.0, 0.0), 500.0);
        assert_eq!(first_size(1000.0, -3.0, 0.0, 0.0), 0.0);
        assert_eq!(first_size(1000.0, 7.0, 0.0, 0.0), 1000.0);
    }
}
