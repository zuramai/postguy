//! Icon rendering: the generated Lucide path data (`icons_gen`) drawn as stroked polylines.
//! No SVG renderer, no font icons: a small path-data parser supports the
//! commands Lucide uses (M/L/H/V/C/S/Q/T/A/Z, absolute and relative) and flattens curves.
//! Unknown commands skip that subpath and report nothing; an icon missing from the table draws
//! nothing (a visible gap is better than a wrong glyph).

use egui::{Color32, Pos2, Rect, Shape, Stroke, Vec2, pos2};

use crate::icons_gen::ICONS;

/// All icon names compiled into the binary (one enum-like table from the generator).
pub fn names() -> impl Iterator<Item = &'static str> {
    ICONS.iter().map(|(name, _)| *name)
}

/// Path data for an icon, if the build includes it.
pub fn data(name: &str) -> Option<&'static [&'static str]> {
    ICONS.iter().find(|(candidate, _)| *candidate == name).map(|(_, data)| *data)
}

/// Draw `name` into `rect` (icons are square; the smaller side wins) with 1.5 px strokes.
pub fn paint(painter: &egui::Painter, rect: Rect, name: &str, color: Color32, stroke_width: f32) {
    let Some(data) = data(name) else { return };
    let size = rect.width().min(rect.height());
    if size <= 0.0 {
        return;
    }
    let scale = size / 24.0;
    let origin = rect.center() - Vec2::splat(size / 2.0);
    let mut polylines: Vec<Vec<Pos2>> = Vec::new();
    for path in data {
        flatten_path(path, |point| {
            let mapped = pos2(origin.x + point.x * scale, origin.y + point.y * scale);
            match polylines.last_mut() {
                Some(line) => line.push(mapped),
                None => polylines.push(vec![mapped]),
            }
        });
        if let Some(last) = polylines.last_mut() {
            // Each source element starts its own stroked polyline (the generator guarantees
            // relative moves anchor at their own element).
            if last.len() == 1 {
                last.push(*last.first().unwrap_or(&last[0]));
            }
        }
        polylines.push(Vec::new());
        // The trailing empty vector starts the next element's line; drop it below.
    }
    polylines.retain(|line| line.len() >= 2);
    let stroke = Stroke::new(stroke_width, color);
    for line in polylines {
        painter.add(Shape::line(line, stroke));
    }
}

/// The SVG path-data parser's output callback receives every flattened point in order.
fn flatten_path(data: &str, mut point: impl FnMut(Pos2)) {
    let tokens = tokenize(data);
    let mut index = 0usize;
    let mut current = pos2(0.0, 0.0);
    let mut subpath_start = current;
    let mut last_control: Option<Pos2> = None;
    let mut command = 'M';
    while index < tokens.len() || command != 'Z' {
        let Some(token) = tokens.get(index) else { break };
        if let Token::Command(next) = token {
            command = *next;
            index += 1;
            if command == 'Z' || command == 'z' {
                point(subpath_start);
                current = subpath_start;
                last_control = None;
                continue;
            }
        }
        let relative = command.is_ascii_lowercase();
        let base = if relative { current } else { pos2(0.0, 0.0) };
        let mut number = |count: usize| -> Option<Vec<f32>> {
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                match tokens.get(index) {
                    Some(Token::Number(value)) => {
                        values.push(*value);
                        index += 1;
                    }
                    _ => return None,
                }
            }
            Some(values)
        };
        let applied = match command.to_ascii_uppercase() {
            'M' => {
                let Some(values) = number(2) else { break };
                current = pos2(base.x + values[0], base.y + values[1]);
                subpath_start = current;
                point(current);
                // Further coordinate pairs are implicit line-tos.
                command = if relative { 'l' } else { 'L' };
                last_control = None;
                true
            }
            'L' => {
                let Some(values) = number(2) else { break };
                current = pos2(base.x + values[0], base.y + values[1]);
                point(current);
                last_control = None;
                true
            }
            'H' => {
                let Some(values) = number(1) else { break };
                current = pos2(if relative { current.x + values[0] } else { values[0] }, current.y);
                point(current);
                last_control = None;
                true
            }
            'V' => {
                let Some(values) = number(1) else { break };
                current = pos2(current.x, if relative { current.y + values[0] } else { values[0] });
                point(current);
                last_control = None;
                true
            }
            'C' => {
                let Some(values) = number(6) else { break };
                let c1 = pos2(base.x + values[0], base.y + values[1]);
                let c2 = pos2(base.x + values[2], base.y + values[3]);
                let end = pos2(base.x + values[4], base.y + values[5]);
                flatten_cubic(current, c1, c2, end, &mut point);
                current = end;
                last_control = Some(c2);
                true
            }
            'S' => {
                let Some(values) = number(4) else { break };
                let c1 = last_control.map_or(current, |control| pos2(2.0 * current.x - control.x, 2.0 * current.y - control.y));
                let c2 = pos2(base.x + values[0], base.y + values[1]);
                let end = pos2(base.x + values[2], base.y + values[3]);
                flatten_cubic(current, c1, c2, end, &mut point);
                current = end;
                last_control = Some(c2);
                true
            }
            'Q' => {
                let Some(values) = number(4) else { break };
                let control = pos2(base.x + values[0], base.y + values[1]);
                let end = pos2(base.x + values[2], base.y + values[3]);
                flatten_quadratic(current, control, end, &mut point);
                current = end;
                last_control = Some(control);
                true
            }
            'T' => {
                let Some(values) = number(2) else { break };
                let control = last_control.map_or(current, |previous| pos2(2.0 * current.x - previous.x, 2.0 * current.y - previous.y));
                let end = pos2(base.x + values[0], base.y + values[1]);
                flatten_quadratic(current, control, end, &mut point);
                current = end;
                last_control = Some(control);
                true
            }
            'A' => {
                let Some(values) = number(7) else { break };
                let end = pos2(base.x + values[5], base.y + values[6]);
                flatten_arc(current, values[0], values[1], values[2], values[3] != 0.0, values[4] != 0.0, end, &mut point);
                current = end;
                last_control = None;
                true
            }
            _ => break,
        };
        if !applied {
            break;
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Token {
    Command(char),
    Number(f32),
}

fn tokenize(data: &str) -> Vec<Token> {
    let bytes = data.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes.get(index).copied().unwrap_or_default();
        if byte.is_ascii_alphabetic() {
            tokens.push(Token::Command(byte as char));
            index += 1;
        } else if byte.is_ascii_whitespace() || byte == b',' {
            index += 1;
        } else {
            let start = index;
            if matches!(byte, b'+' | b'-') {
                index += 1;
            }
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
            if bytes.get(index) == Some(&b'.') {
                index += 1;
                while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                    index += 1;
                }
            }
            if matches!(bytes.get(index), Some(b'e' | b'E')) {
                index += 1;
                if matches!(bytes.get(index), Some(b'+' | b'-')) {
                    index += 1;
                }
                while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                    index += 1;
                }
            }
            match data.get(start..index).and_then(|text| text.parse::<f32>().ok()) {
                Some(value) => tokens.push(Token::Number(value)),
                None => break,
            }
        }
    }
    tokens
}

const CURVE_STEPS: usize = 16;

fn flatten_cubic(start: Pos2, c1: Pos2, c2: Pos2, end: Pos2, point: &mut impl FnMut(Pos2)) {
    for step in 1..=CURVE_STEPS {
        let t = step as f32 / CURVE_STEPS as f32;
        let u = 1.0 - t;
        let x = u * u * u * start.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * end.x;
        let y = u * u * u * start.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * end.y;
        point(pos2(x, y));
    }
}

fn flatten_quadratic(start: Pos2, control: Pos2, end: Pos2, point: &mut impl FnMut(Pos2)) {
    for step in 1..=CURVE_STEPS {
        let t = step as f32 / CURVE_STEPS as f32;
        let u = 1.0 - t;
        let x = u * u * start.x + 2.0 * u * t * control.x + t * t * end.x;
        let y = u * u * start.y + 2.0 * u * t * control.y + t * t * end.y;
        point(pos2(x, y));
    }
}

/// SVG endpoint-parameterized elliptic arc (implementation notes F.6.5), sampled by angle.
#[allow(clippy::too_many_arguments)]
fn flatten_arc(start: Pos2, rx: f32, ry: f32, rotation_deg: f32, large_arc: bool, sweep: bool, end: Pos2, point: &mut impl FnMut(Pos2)) {
    let mut rx = rx.abs();
    let mut ry = ry.abs();
    if rx < f32::EPSILON || ry < f32::EPSILON || start == end {
        point(end);
        return;
    }
    let phi = rotation_deg.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let dx = (start.x - end.x) / 2.0;
    let dy = (start.y - end.y) / 2.0;
    let x1 = cos_phi * dx + sin_phi * dy;
    let y1 = -sin_phi * dx + cos_phi * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
    }
    let numerator = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let denominator = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coefficient = if denominator > 0.0 { (numerator / denominator).sqrt() } else { 0.0 };
    if large_arc == sweep {
        coefficient = -coefficient;
    }
    let cx1 = coefficient * rx * y1 / ry;
    let cy1 = -coefficient * ry * x1 / rx;
    let cx = cos_phi * cx1 - sin_phi * cy1 + (start.x + end.x) / 2.0;
    let cy = sin_phi * cx1 + cos_phi * cy1 + (start.y + end.y) / 2.0;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| -> f32 {
        let dot = ux * vx + uy * vy;
        let lengths = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let sign = if ux * vy - uy * vx < 0.0 { -1.0 } else { 1.0 };
        if lengths <= f32::EPSILON { 0.0 } else { sign * (dot / lengths).clamp(-1.0, 1.0).acos() }
    };
    let start_angle = angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut sweep_angle = angle((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    if !sweep && sweep_angle > 0.0 {
        sweep_angle -= std::f32::consts::TAU;
    } else if sweep && sweep_angle < 0.0 {
        sweep_angle += std::f32::consts::TAU;
    }
    let steps = ((sweep_angle.abs() / (std::f32::consts::PI / 24.0)).ceil() as usize).clamp(2, 128);
    for step in 1..=steps {
        let theta = start_angle + sweep_angle * (step as f32 / steps as f32);
        let (sin_theta, cos_theta) = theta.sin_cos();
        let x = cx + cos_phi * rx * cos_theta - sin_phi * ry * sin_theta;
        let y = cy + sin_phi * rx * cos_theta + cos_phi * ry * sin_theta;
        point(pos2(x, y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_generated_icon_flattens_to_at_least_one_polyline() {
        for name in names() {
            let data = data(name).unwrap_or_default();
            assert!(!data.is_empty(), "{name} has no paths");
            for path in data {
                let mut points = 0usize;
                flatten_path(path, |_| points += 1);
                assert!(points >= 2, "{name}: a path produced {points} points from `{path}`");
            }
        }
    }

    #[test]
    fn a_simple_move_line_parses_and_scales() {
        let mut points: Vec<Pos2> = Vec::new();
        flatten_path("M5 12h14", |point| points.push(point));
        assert_eq!(points.first().copied(), Some(pos2(5.0, 12.0)));
        assert_eq!(points.last().copied(), Some(pos2(19.0, 12.0)));
    }

    #[test]
    fn relative_curves_and_arcs_end_where_the_data_says() {
        let mut points = Vec::new();
        flatten_path("M2 12 a10 10 0 1 0 20 0 a10 10 0 1 0 -20 0", |point| points.push(point));
        let last = points.last().copied().unwrap_or(pos2(f32::NAN, f32::NAN));
        assert!((last.x - 2.0).abs() < 0.5 && (last.y - 12.0).abs() < 0.5, "two half-circles return home: {last:?}");
        assert!(points.len() > 20, "arcs are sampled, not a single chord: {}", points.len());
    }

    #[test]
    fn malformed_or_empty_data_never_panics() {
        for data in ["", "M", "M 1", "L 1 2", "M 1 2 X 3 4", "M 1 2 C 1 2 3"] {
            let mut points = 0usize;
            flatten_path(data, |_| points += 1);
        }
    }
}
