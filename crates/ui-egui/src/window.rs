//! Window placement math for the Window menu: halves, quarters, thirds, fill and center,
//! computed against the monitor's work area in egui points.

use egui::{Rect, Vec2, pos2};

use crate::commands::{Placement, WindowCommand};

/// The outer rectangle `cmd` asks for, or `None` when the command is not a placement (or
/// `Center` has no current size). Every coordinate is rounded to whole points.
pub fn placement(cmd: WindowCommand, work: Rect, current: Option<Rect>) -> Option<Rect> {
    let (w, h) = (work.width(), work.height());
    let cell = |x: f32, y: f32, width: f32, height: f32| {
        let min = pos2((work.min.x + x).round(), (work.min.y + y).round());
        let max = pos2((work.min.x + x + width).round(), (work.min.y + y + height).round());
        Rect::from_min_max(min, max)
    };
    Some(match cmd {
        WindowCommand::Fill => cell(0.0, 0.0, w, h),
        WindowCommand::Center => {
            let size: Vec2 = current?.size();
            let min = pos2(work.center().x - size.x / 2.0, work.center().y - size.y / 2.0);
            Rect::from_min_size(pos2(min.x.round(), min.y.round()), size)
        }
        WindowCommand::Place(placement) => match placement {
            Placement::LeftHalf => cell(0.0, 0.0, w / 2.0, h),
            Placement::RightHalf => cell(w / 2.0, 0.0, w / 2.0, h),
            Placement::TopHalf => cell(0.0, 0.0, w, h / 2.0),
            Placement::BottomHalf => cell(0.0, h / 2.0, w, h / 2.0),
            Placement::TopLeft => cell(0.0, 0.0, w / 2.0, h / 2.0),
            Placement::TopRight => cell(w / 2.0, 0.0, w / 2.0, h / 2.0),
            Placement::BottomLeft => cell(0.0, h / 2.0, w / 2.0, h / 2.0),
            Placement::BottomRight => cell(w / 2.0, h / 2.0, w / 2.0, h / 2.0),
            Placement::LeftThird => cell(0.0, 0.0, w / 3.0, h),
            Placement::CenterThird => cell(w / 3.0, 0.0, w / 3.0, h),
            Placement::RightThird => cell(2.0 * w / 3.0, 0.0, w / 3.0, h),
            Placement::LeftTwoThirds => cell(0.0, 0.0, 2.0 * w / 3.0, h),
            Placement::RightTwoThirds => cell(w / 3.0, 0.0, 2.0 * w / 3.0, h),
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placements_tile_the_work_area() {
        let work = Rect::from_min_max(pos2(0.0, 25.0), pos2(1440.0, 900.0));
        let place = |p| placement(WindowCommand::Place(p), work, None);
        assert_eq!(place(Placement::LeftHalf), Some(Rect::from_min_max(pos2(0.0, 25.0), pos2(720.0, 900.0))));
        assert_eq!(place(Placement::BottomRight), Some(Rect::from_min_max(pos2(720.0, 463.0), pos2(1440.0, 900.0))));
        assert_eq!(place(Placement::RightTwoThirds), Some(Rect::from_min_max(pos2(480.0, 25.0), pos2(1440.0, 900.0))));
        assert_eq!(placement(WindowCommand::Fill, work, None), Some(work));
        let current = Rect::from_min_size(pos2(10.0, 10.0), egui::vec2(1000.0, 600.0));
        let centered = placement(WindowCommand::Center, work, Some(current));
        assert_eq!(centered.map(|rect| rect.min), Some(pos2(220.0, 163.0)));
        assert_eq!(centered.map(|rect| rect.size()), Some(egui::vec2(1000.0, 600.0)));
        assert_eq!(placement(WindowCommand::Center, work, None), None);
        assert_eq!(placement(WindowCommand::Minimize, work, Some(current)), None);
    }

    #[test]
    fn halves_and_thirds_cover_the_width_without_gaps() {
        let work = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0));
        let rect = |p| placement(WindowCommand::Place(p), work, None).unwrap_or(Rect::NOTHING);
        assert_eq!(rect(Placement::LeftThird).max.x, rect(Placement::CenterThird).min.x);
        assert_eq!(rect(Placement::CenterThird).max.x, rect(Placement::RightThird).min.x);
        assert_eq!(rect(Placement::RightThird).max.x, 1000.0);
        assert_eq!(rect(Placement::LeftHalf).max.x, rect(Placement::RightHalf).min.x);
    }
}
