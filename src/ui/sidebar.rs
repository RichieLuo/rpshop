use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke};

pub(super) const STORAGE_KEY: &str = "rpshop-sidebar-layout";
const DIVIDER_HEIGHT: f32 = 10.0;
const MIN_HEIGHTS: [f32; 3] = [130.0, 120.0, 110.0];

/// View preferences belong to the application, never to the edited document.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct SidebarLayout {
    first: f32,
    second: f32,
}

impl Default for SidebarLayout {
    fn default() -> Self {
        Self {
            first: 0.36,
            second: 0.70,
        }
    }
}

pub(super) struct PaneRects {
    pub panes: [Rect; 3],
    pub dividers: [Rect; 2],
}

impl SidebarLayout {
    pub fn rects(&self, bounds: Rect) -> PaneRects {
        let gap = DIVIDER_HEIGHT.min(bounds.height().max(0.0) / 2.0);
        let height = (bounds.height() - gap * 2.0).max(0.0);
        let scale = (height / MIN_HEIGHTS.iter().sum::<f32>()).min(1.0);
        let [min_page, min_library, min_layers] = MIN_HEIGHTS.map(|h| h * scale);
        let defaults = Self::default();
        let first = if self.first.is_finite() {
            self.first
        } else {
            defaults.first
        };
        let second = if self.second.is_finite() {
            self.second
        } else {
            defaults.second
        };
        let first =
            (height * first).clamp(min_page, (height - min_library - min_layers).max(min_page));
        let second = (height * second).clamp(
            first + min_library,
            (height - min_layers).max(first + min_library),
        );
        let first_y = bounds.top() + first;
        let second_y = bounds.top() + gap + second;
        let rect = |top, bottom| {
            Rect::from_min_max(
                Pos2::new(bounds.left(), top),
                Pos2::new(bounds.right(), bottom),
            )
        };
        PaneRects {
            panes: [
                rect(bounds.top(), first_y),
                rect(first_y + gap, second_y),
                rect(second_y + gap, bounds.bottom()),
            ],
            dividers: [rect(first_y, first_y + gap), rect(second_y, second_y + gap)],
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> PaneRects {
        let bounds = ui.available_rect_before_wrap();
        // Reserve the complete sidebar once. Content in a child pane cannot move
        // the next pane, even when a tree grows or a library contains more rows.
        ui.allocate_rect(bounds, Sense::hover());
        let mut rects = self.rects(bounds);
        for index in 0..2 {
            let response = ui
                .interact(
                    rects.dividers[index],
                    ui.id().with(("sidebar-divider", index)),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(egui::CursorIcon::ResizeVertical)
                .on_hover_text("上下拖动调整区域高度；双击恢复默认布局");
            if response.double_clicked() {
                *self = Self::default();
                rects = self.rects(bounds);
            } else if response.dragged() && bounds.height() > DIVIDER_HEIGHT * 2.0 {
                let total = (bounds.height() - DIVIDER_HEIGHT * 2.0).max(1.0);
                let delta = response.drag_delta().y;
                let scale = (total / MIN_HEIGHTS.iter().sum::<f32>()).min(1.0);
                let second = rects.panes[0].height() + rects.panes[1].height();
                if index == 0 {
                    self.first = (rects.panes[0].height() + delta)
                        .clamp(MIN_HEIGHTS[0] * scale, second - MIN_HEIGHTS[1] * scale)
                        / total;
                    self.second = second / total;
                } else {
                    self.first = rects.panes[0].height() / total;
                    self.second = (second + delta).clamp(
                        rects.panes[0].height() + MIN_HEIGHTS[1] * scale,
                        total - MIN_HEIGHTS[2] * scale,
                    ) / total;
                }
                rects = self.rects(bounds);
                // Store the constrained positions so reversing a drag at a limit
                // responds immediately, without any invisible overshoot.
                self.first = rects.panes[0].height() / total;
                self.second = (rects.panes[0].height() + rects.panes[1].height()) / total;
            }
            let divider = rects.dividers[index];
            let active = response.hovered() || response.dragged();
            if active {
                ui.painter()
                    .rect_filled(divider, 2.0, Color32::from_rgb(247, 245, 255));
            }
            let color = if active { super::PURPLE } else { super::BORDER };
            ui.painter().hline(
                divider.x_range(),
                divider.center().y,
                Stroke::new(1.0, color),
            );
            ui.painter().hline(
                divider.center().x - 12.0..=divider.center().x + 12.0,
                divider.center().y,
                Stroke::new(3.0, color),
            );
        }
        rects
    }
}

pub(super) fn pane(ui: &mut egui::Ui, rect: Rect, name: &str) -> egui::Ui {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(name)
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(ui.clip_rect().intersect(rect));
    child
}
