//! Canvas navigation in screen points. Document coordinates never change.
use eframe::egui::{self, Pos2, Rect, Vec2, vec2};

pub(super) const MIN_ZOOM: f32 = 0.1;
pub(super) const MAX_ZOOM: f32 = 10.0;
pub(super) const RULER_SIZE: f32 = 18.0;

pub(super) fn content_rect(viewport: Rect) -> Rect {
    Rect::from_min_max(viewport.min + vec2(RULER_SIZE, RULER_SIZE), viewport.max)
}

pub(super) fn paint_auto_rulers(painter: &egui::Painter, viewport: Rect, origin: Pos2, zoom: f32) {
    use egui::{Align2, Color32, FontId, Stroke};
    let top = Rect::from_min_max(
        viewport.min,
        egui::pos2(viewport.max.x, viewport.min.y + RULER_SIZE),
    );
    let left = Rect::from_min_max(
        viewport.min,
        egui::pos2(viewport.min.x + RULER_SIZE, viewport.max.y),
    );
    let fill = Color32::from_rgb(250, 250, 250);
    let border = Stroke::new(1.0, Color32::from_rgb(216, 216, 216));
    painter.rect_filled(top, 0.0, fill);
    painter.rect_filled(left, 0.0, fill);
    painter.line_segment([top.left_bottom(), top.right_bottom()], border);
    painter.line_segment([left.right_top(), left.right_bottom()], border);
    let major = ((55.0 / (100.0 * zoom)).ceil().max(1.0) * 100.0) as i32;
    for horizontal in [true, false] {
        let (min, max, base) = if horizontal {
            (viewport.left() + RULER_SIZE, viewport.right(), origin.x)
        } else {
            (viewport.top() + RULER_SIZE, viewport.bottom(), origin.y)
        };
        let start = (((min - base) / zoom) / major as f32).floor() as i32;
        let end = (((max - base) / zoom) / major as f32).ceil() as i32;
        for index in start..=end {
            let value = index * major;
            let position = base + value as f32 * zoom;
            if position < min || position > max {
                continue;
            }
            let (a, b, label) = if horizontal {
                (
                    egui::pos2(position, top.bottom() - 4.0),
                    egui::pos2(position, top.bottom()),
                    egui::pos2(position + 3.0, top.center().y - 1.0),
                )
            } else {
                (
                    egui::pos2(left.right() - 4.0, position),
                    egui::pos2(left.right(), position),
                    egui::pos2(left.left() + 2.0, position + 7.0),
                )
            };
            painter.line_segment([a, b], border);
            painter.text(
                label,
                Align2::LEFT_CENTER,
                value.to_string(),
                FontId::proportional(8.0),
                Color32::from_rgb(145, 145, 145),
            );
        }
    }
    painter.rect_filled(
        Rect::from_min_size(viewport.min, vec2(RULER_SIZE, RULER_SIZE)),
        0.0,
        fill,
    );
}

#[derive(Clone, Copy)]
pub(super) enum ViewCommand {
    Step(f32),
    Zoom(f32),
    CenterPage,
    FitSelection,
    CenterSelection,
    Origin,
}
pub(super) struct PanDrag {
    pub pointer: Pos2,
    pub pan: Vec2,
    pub button: egui::PointerButton,
}
pub(super) struct CanvasView {
    pub pan: Vec2,
    pub pending: Option<ViewCommand>,
    pub panning: Option<PanDrag>,
}
impl Default for CanvasView {
    fn default() -> Self {
        Self {
            pan: vec2(40.0, 40.0),
            pending: None,
            panning: None,
        }
    }
}
impl CanvasView {
    pub fn zoom_at(&mut self, zoom: &mut f32, target: f32, anchor: Vec2) {
        if !target.is_finite() || !anchor.is_finite() {
            return;
        }
        let target = target.clamp(MIN_ZOOM, MAX_ZOOM);
        self.pan = anchor - (anchor - self.pan) * (target / *zoom);
        *zoom = target;
    }
    pub fn center(&mut self, zoom: f32, viewport: Rect, bounds: Rect) {
        self.pan = viewport.size() / 2.0 - bounds.center().to_vec2() * zoom;
    }
    pub fn fit(&mut self, zoom: &mut f32, viewport: Rect, bounds: Rect) {
        let available = (viewport.size() - vec2(80.0, 80.0)).max(vec2(1.0, 1.0));
        *zoom = (available.x / bounds.width().max(1.0))
            .min(available.y / bounds.height().max(1.0))
            .clamp(MIN_ZOOM, MAX_ZOOM);
        self.center(*zoom, viewport, bounds);
    }
    pub fn wheel(
        &mut self,
        zoom: &mut f32,
        unit: egui::MouseWheelUnit,
        delta: Vec2,
        modifiers: egui::Modifiers,
        anchor: Vec2,
        viewport: Rect,
    ) {
        if !delta.is_finite() {
            return;
        }
        if modifiers.ctrl || modifiers.command {
            let step = match unit {
                egui::MouseWheelUnit::Line => 0.05,
                egui::MouseWheelUnit::Point => 0.15 / 120.0,
                egui::MouseWheelUnit::Page => 0.15,
            };
            let delta = if delta.y != 0.0 { delta.y } else { delta.x };
            let target = *zoom + delta * step;
            let target = if (target - 1.0).abs() < 0.025 {
                1.0
            } else {
                target
            };
            self.zoom_at(zoom, target, anchor);
        } else {
            let scale = match unit {
                egui::MouseWheelUnit::Line => 40.0,
                egui::MouseWheelUnit::Point => 1.0,
                egui::MouseWheelUnit::Page => viewport.height(),
            };
            let delta = if modifiers.shift {
                vec2(delta.y, delta.x)
            } else {
                delta
            };
            self.pan += delta * scale;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zoom_keeps_the_pointer_document_point_and_clamps_limits() {
        let mut view = CanvasView::default();
        let mut zoom = 0.5;
        let pointer = vec2(300.0, 180.0);
        let point = (pointer - view.pan) / zoom;
        view.zoom_at(&mut zoom, 0.65, pointer);
        assert!(((pointer - view.pan) / zoom - point).length() < 0.0001);
        view.zoom_at(&mut zoom, 100.0, pointer);
        assert_eq!(zoom, MAX_ZOOM);
        view.zoom_at(&mut zoom, -10.0, pointer);
        assert_eq!(zoom, MIN_ZOOM);
    }
    #[test]
    fn wheel_modifiers_choose_navigation_without_touching_document_geometry() {
        let mut view = CanvasView::default();
        let mut zoom = 0.5;
        let viewport = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let pointer = vec2(300.0, 200.0);
        view.wheel(
            &mut zoom,
            egui::MouseWheelUnit::Line,
            vec2(0.0, 3.0),
            egui::Modifiers::CTRL,
            pointer,
            viewport,
        );
        assert!((zoom - 0.65).abs() < 0.0001);
        let pan = view.pan;
        view.wheel(
            &mut zoom,
            egui::MouseWheelUnit::Point,
            vec2(0.0, 21.0),
            egui::Modifiers::SHIFT,
            pointer,
            viewport,
        );
        assert_eq!(view.pan, pan + vec2(21.0, 0.0));
        assert!((zoom - 0.65).abs() < 0.0001);
    }
}
