//! Document-space geometry for selection, resizing, rotation and arrangement.

use std::collections::BTreeSet;

use crate::model::{Widget, WidgetKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    NorthWest,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    Rotate,
}

pub const RESIZE_HANDLES: [Handle; 8] = [
    Handle::NorthWest,
    Handle::North,
    Handle::NorthEast,
    Handle::East,
    Handle::SouthEast,
    Handle::South,
    Handle::SouthWest,
    Handle::West,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Top,
    Middle,
    Bottom,
}

fn center(widget: &Widget) -> [f32; 2] {
    [
        widget.x + widget.width / 2.0,
        widget.y + widget.height / 2.0,
    ]
}

fn rotated(vector: [f32; 2], degrees: f32) -> [f32; 2] {
    let angle = if degrees.is_finite() {
        degrees.rem_euclid(360.0)
    } else {
        0.0
    };
    let (sin, cos) = angle.to_radians().sin_cos();
    [
        vector[0] * cos - vector[1] * sin,
        vector[0] * sin + vector[1] * cos,
    ]
}

fn local_to_document(widget: &Widget, point: [f32; 2]) -> [f32; 2] {
    let origin = center(widget);
    let offset = rotated(point, widget.rotation);
    [origin[0] + offset[0], origin[1] + offset[1]]
}

/// Selection handles follow the local box. The rotation handle sits 24
/// document units beyond the midpoint of its top edge.
pub fn handle_positions(widget: &Widget) -> [(Handle, [f32; 2]); 9] {
    let half_width = widget.width / 2.0;
    let half_height = widget.height / 2.0;
    [
        (Handle::NorthWest, [-half_width, -half_height]),
        (Handle::North, [0.0, -half_height]),
        (Handle::NorthEast, [half_width, -half_height]),
        (Handle::East, [half_width, 0.0]),
        (Handle::SouthEast, [half_width, half_height]),
        (Handle::South, [0.0, half_height]),
        (Handle::SouthWest, [-half_width, half_height]),
        (Handle::West, [-half_width, 0.0]),
        (Handle::Rotate, [0.0, -half_height - 24.0]),
    ]
    .map(|(handle, point)| (handle, local_to_document(widget, point)))
}

/// Test the rendered geometry. Visibility/locking eligibility belongs to the
/// caller so the same geometry works for canvas and layer selections.
pub fn hit_test(widget: &Widget, point: [f32; 2], tolerance: f32) -> bool {
    if !point.iter().all(|coordinate| coordinate.is_finite()) {
        return false;
    }
    let origin = center(widget);
    let offset = rotated(
        [point[0] - origin[0], point[1] - origin[1]],
        -widget.rotation,
    );
    let local = [
        offset[0] + widget.width / 2.0,
        offset[1] + widget.height / 2.0,
    ];
    let tolerance = if tolerance.is_finite() {
        tolerance.max(0.0)
    } else {
        0.0
    };
    if widget.kind == WidgetKind::Line {
        let [a, b] = crate::vector_edit::line_endpoints(widget);
        let a = [a[0] - widget.x, a[1] - widget.y];
        let d = [b[0] - widget.x - a[0], b[1] - widget.y - a[1]];
        let length_squared = d[0] * d[0] + d[1] * d[1];
        let fraction = if length_squared > 0.0 {
            (((local[0] - a[0]) * d[0] + (local[1] - a[1]) * d[1]) / length_squared).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dx = local[0] - a[0] - fraction * d[0];
        let dy = local[1] - a[1] - fraction * d[1];
        let radius = tolerance + widget.height.clamp(1.0, 4.0) / 2.0;
        return dx * dx + dy * dy <= radius * radius;
    }
    if widget.kind == WidgetKind::Ellipse {
        let rx = widget.width / 2.0 + tolerance;
        let ry = widget.height / 2.0 + tolerance;
        return rx > 0.0 && ry > 0.0 && (offset[0] / rx).powi(2) + (offset[1] / ry).powi(2) <= 1.0;
    }
    local[0] >= -tolerance
        && local[0] <= widget.width + tolerance
        && local[1] >= -tolerance
        && local[1] <= widget.height + tolerance
}

/// Axis-aligned visual bounds of the rotated local rectangle.
pub fn rotated_bounds(widget: &Widget) -> [f32; 4] {
    let points = handle_positions(widget);
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for index in [0, 2, 4, 6] {
        let point = points[index].1;
        bounds[0] = bounds[0].min(point[0]);
        bounds[1] = bounds[1].min(point[1]);
        bounds[2] = bounds[2].max(point[0]);
        bounds[3] = bounds[3].max(point[1]);
    }
    bounds
}

fn directions(handle: Handle) -> [f32; 2] {
    match handle {
        Handle::NorthWest => [-1.0, -1.0],
        Handle::North => [0.0, -1.0],
        Handle::NorthEast => [1.0, -1.0],
        Handle::East => [1.0, 0.0],
        Handle::SouthEast => [1.0, 1.0],
        Handle::South => [0.0, 1.0],
        Handle::SouthWest => [-1.0, 1.0],
        Handle::West => [-1.0, 0.0],
        Handle::Rotate => [0.0, 0.0],
    }
}

fn snapped(value: f32, grid: f32) -> f32 {
    if grid.is_finite() && grid > 0.0 {
        (value / grid).round() * grid
    } else {
        value
    }
}

/// Resize from the original drag-start widget using a document-space delta.
/// The opposite corner/edge center stays fixed, or the center when requested.
/// Grid snapping applies to local dimensions; aspect-constrained resizing
/// snaps the controlling dimension and derives the other from the ratio.
pub fn resize(
    widget: &Widget,
    handle: Handle,
    delta: [f32; 2],
    keep_ratio: bool,
    from_center: bool,
    snap: f32,
) -> Widget {
    let mut result = widget.clone();
    if widget.locked
        || handle == Handle::Rotate
        || delta == [0.0, 0.0]
        || !delta.iter().all(|coordinate| coordinate.is_finite())
    {
        return result;
    }
    let axes = directions(handle);
    let local_delta = rotated(delta, -widget.rotation);
    let multiplier = if from_center { 2.0 } else { 1.0 };
    let raw_width = widget.width + local_delta[0] * axes[0] * multiplier;
    let raw_height = widget.height + local_delta[1] * axes[1] * multiplier;
    let (width, height) = if keep_ratio {
        let controls_width = axes[1] == 0.0
            || (axes[0] != 0.0
                && (local_delta[0] / widget.width).abs() >= (local_delta[1] / widget.height).abs());
        let factor = if controls_width {
            snapped(raw_width, snap) / widget.width
        } else {
            snapped(raw_height, snap) / widget.height
        }
        .max((1.0 / widget.width).max(1.0 / widget.height));
        (widget.width * factor, widget.height * factor)
    } else {
        (
            if axes[0] != 0.0 {
                snapped(raw_width, snap).max(1.0)
            } else {
                widget.width
            },
            if axes[1] != 0.0 {
                snapped(raw_height, snap).max(1.0)
            } else {
                widget.height
            },
        )
    };
    let old_center = center(widget);
    let offset = if from_center {
        [0.0, 0.0]
    } else {
        rotated(
            [
                (width - widget.width) * axes[0] / 2.0,
                (height - widget.height) * axes[1] / 2.0,
            ],
            widget.rotation,
        )
    };
    result.width = width;
    result.height = height;
    result.x = old_center[0] + offset[0] - width / 2.0;
    result.y = old_center[1] + offset[1] - height / 2.0;
    result
}

/// Rotate both the center and the angle, supporting a shared selection pivot.
pub fn rotate(widget: &Widget, delta_degrees: f32, pivot: [f32; 2]) -> Widget {
    let mut result = widget.clone();
    if widget.locked || !delta_degrees.is_finite() || !pivot.iter().all(|value| value.is_finite()) {
        return result;
    }
    let old_center = center(widget);
    let offset = rotated(
        [old_center[0] - pivot[0], old_center[1] - pivot[1]],
        delta_degrees,
    );
    result.x = pivot[0] + offset[0] - widget.width / 2.0;
    result.y = pivot[1] + offset[1] - widget.height / 2.0;
    result.rotation = ((f64::from(widget.rotation) + f64::from(delta_degrees)).rem_euclid(360.0)
        as f32)
        .rem_euclid(360.0);
    result
}

/// Align visual bounds. Explicitly selected hidden widgets participate, and
/// locked widgets provide reference bounds while remaining stationary.
pub fn align(widgets: &mut [Widget], ids: &BTreeSet<u64>, mode: Align) {
    let selected = widgets
        .iter()
        .filter(|widget| ids.contains(&widget.id))
        .collect::<Vec<_>>();
    if selected.len() < 2 {
        return;
    }
    let mut group = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for widget in selected {
        let bounds = rotated_bounds(widget);
        group[0] = group[0].min(bounds[0]);
        group[1] = group[1].min(bounds[1]);
        group[2] = group[2].max(bounds[2]);
        group[3] = group[3].max(bounds[3]);
    }
    for widget in widgets
        .iter_mut()
        .filter(|widget| ids.contains(&widget.id) && !widget.locked)
    {
        let bounds = rotated_bounds(widget);
        match mode {
            Align::Left => widget.x += group[0] - bounds[0],
            Align::Center => widget.x += (group[0] + group[2] - bounds[0] - bounds[2]) / 2.0,
            Align::Right => widget.x += group[2] - bounds[2],
            Align::Top => widget.y += group[1] - bounds[1],
            Align::Middle => widget.y += (group[1] + group[3] - bounds[1] - bounds[3]) / 2.0,
            Align::Bottom => widget.y += group[3] - bounds[3],
        }
    }
}

/// Distribute equal visual gaps between fixed end widgets. Locked interior
/// widgets act as additional anchors, producing equal gaps in each span.
pub fn distribute(widgets: &mut [Widget], ids: &BTreeSet<u64>, horizontal: bool) {
    let axis = if horizontal { 0 } else { 1 };
    let mut selected = widgets
        .iter()
        .enumerate()
        .filter(|(_, widget)| ids.contains(&widget.id))
        .map(|(index, widget)| (index, rotated_bounds(widget)))
        .collect::<Vec<_>>();
    if selected.len() < 3 {
        return;
    }
    selected.sort_by(|left, right| {
        left.1[axis]
            .total_cmp(&right.1[axis])
            .then(left.0.cmp(&right.0))
    });
    let anchors = selected
        .iter()
        .enumerate()
        .filter(|(position, (index, _))| {
            *position == 0 || *position + 1 == selected.len() || widgets[*index].locked
        })
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    for pair in anchors.windows(2) {
        let (first, last) = (pair[0], pair[1]);
        if last == first + 1 {
            continue;
        }
        let occupied = selected[first + 1..last]
            .iter()
            .map(|(_, bounds)| bounds[axis + 2] - bounds[axis])
            .sum::<f32>();
        let gap = (selected[last].1[axis] - selected[first].1[axis + 2] - occupied)
            / (last - first) as f32;
        let mut position = selected[first].1[axis + 2] + gap;
        for (index, bounds) in &selected[first + 1..last] {
            let shift = position - bounds[axis];
            if horizontal {
                widgets[*index].x += shift;
            } else {
                widgets[*index].y += shift;
            }
            position += bounds[axis + 2] - bounds[axis] + gap;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(id: u64, x: f32, y: f32, width: f32, height: f32, rotation: f32) -> Widget {
        let mut widget = Widget::new(id, WidgetKind::Rectangle, x, y);
        widget.width = width;
        widget.height = height;
        widget.rotation = rotation;
        widget
    }

    fn close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.001,
            "actual {actual}, expected {expected}"
        );
    }

    fn same_point(actual: [f32; 2], expected: [f32; 2]) {
        close(actual[0], expected[0]);
        close(actual[1], expected[1]);
    }

    #[test]
    fn every_rotated_resize_handle_preserves_its_opposite_anchor() {
        let original = widget(1, 13.0, -5.0, 120.0, 80.0, 37.0);
        let positions = handle_positions(&original);
        for (index, handle) in RESIZE_HANDLES.into_iter().enumerate() {
            let axes = directions(handle);
            let delta = rotated([axes[0] * 23.0, axes[1] * 17.0], original.rotation);
            let resized = resize(&original, handle, delta, false, false, 0.0);
            same_point(
                handle_positions(&resized)[(index + 4) % 8].1,
                positions[(index + 4) % 8].1,
            );
            close(resized.width, original.width + axes[0].abs() * 23.0);
            close(resized.height, original.height + axes[1].abs() * 17.0);
            close(resized.rotation, original.rotation);
        }
        let offset = [
            positions[8].1[0] - positions[1].1[0],
            positions[8].1[1] - positions[1].1[1],
        ];
        close(offset[0].hypot(offset[1]), 24.0);
    }

    #[test]
    fn proportional_center_resizing_snaps_without_moving_the_center() {
        let original = widget(1, 11.0, 12.0, 120.0, 80.0, 31.0);
        let delta = rotated([20.0, 6.0], original.rotation);
        let resized = resize(&original, Handle::SouthEast, delta, true, true, 10.0);
        same_point(center(&resized), center(&original));
        close(resized.width, 160.0);
        close(resized.width / resized.height, 1.5);
        let small = resize(
            &original,
            Handle::SouthEast,
            rotated([-200.0, -200.0], original.rotation),
            true,
            false,
            0.0,
        );
        assert!(small.width >= 1.0 && small.height >= 1.0);
        close(small.width / small.height, 1.5);
        same_point(
            handle_positions(&small)[0].1,
            handle_positions(&original)[0].1,
        );
        let edge = resize(
            &original,
            Handle::North,
            rotated([0.0, -20.0], original.rotation),
            true,
            false,
            10.0,
        );
        close(edge.height, 100.0);
        close(edge.width, 150.0);
        same_point(
            handle_positions(&edge)[5].1,
            handle_positions(&original)[5].1,
        );
    }

    #[test]
    fn rotated_hit_testing_uses_line_segments_and_local_shapes() {
        let mut line = widget(1, 25.0, 50.0, 100.0, 40.0, 68.0);
        line.kind = WidgetKind::Line;
        assert!(hit_test(&line, local_to_document(&line, [10.0, 4.0]), 0.0));
        assert!(hit_test(&line, handle_positions(&line)[0].1, 2.0));
        assert!(!hit_test(
            &line,
            local_to_document(&line, [-50.0, 20.0]),
            2.0
        ));
        assert!(!hit_test(&line, local_to_document(&line, [0.0, 9.0]), 2.0));
        let rectangle = widget(2, -50.0, -10.0, 100.0, 20.0, 45.0);
        assert!(!hit_test(&rectangle, [38.0, 0.0], 0.0));
        assert!(hit_test(
            &rectangle,
            local_to_document(&rectangle, [45.0, 0.0]),
            0.0
        ));
        let mut ellipse = widget(3, 0.0, 0.0, 100.0, 40.0, 25.0);
        ellipse.kind = WidgetKind::Ellipse;
        assert!(hit_test(&ellipse, center(&ellipse), 0.0));
        assert!(!hit_test(&ellipse, handle_positions(&ellipse)[0].1, 0.0));
    }

    #[test]
    fn rotation_moves_the_center_around_the_shared_pivot_and_normalizes_angle() {
        let original = widget(1, 10.0, 25.0, 20.0, 10.0, 350.0);
        let rotated = rotate(&original, 90.0, [10.0, 10.0]);
        same_point(center(&rotated), [-10.0, 20.0]);
        close(rotated.rotation, 80.0);
        close(rotated.width, original.width);
        close(rotated.height, original.height);
        let mut locked = original.clone();
        locked.locked = true;
        assert_eq!(rotate(&locked, 45.0, [0.0, 0.0]), locked);
        assert_eq!(
            resize(&locked, Handle::East, [30.0, 0.0], false, false, 10.0),
            locked
        );
    }

    #[test]
    fn alignment_uses_rotated_bounds_and_locked_reference_widgets() {
        let mut widgets = vec![
            widget(1, 0.0, 0.0, 100.0, 20.0, 90.0),
            widget(2, 200.0, 60.0, 40.0, 60.0, 30.0),
            widget(3, -1000.0, -1000.0, 10.0, 10.0, 0.0),
        ];
        widgets[0].locked = true;
        let original = widgets.clone();
        let ids = BTreeSet::from([1, 2]);
        align(&mut widgets, &ids, Align::Left);
        close(
            rotated_bounds(&widgets[1])[0],
            rotated_bounds(&widgets[0])[0],
        );
        assert_eq!(widgets[0], original[0]);
        assert_eq!(widgets[2], original[2]);
        for mode in [
            Align::Center,
            Align::Right,
            Align::Top,
            Align::Middle,
            Align::Bottom,
        ] {
            let mut pair = original[..2].to_vec();
            pair[0].locked = false;
            align(&mut pair, &ids, mode);
            let a = rotated_bounds(&pair[0]);
            let b = rotated_bounds(&pair[1]);
            match mode {
                Align::Center => close(a[0] + a[2], b[0] + b[2]),
                Align::Right => close(a[2], b[2]),
                Align::Top => close(a[1], b[1]),
                Align::Middle => close(a[1] + a[3], b[1] + b[3]),
                Align::Bottom => close(a[3], b[3]),
                Align::Left => unreachable!(),
            }
        }
    }

    #[test]
    fn distribution_uses_visual_widths_and_partitions_at_locked_anchors() {
        let mut widgets = vec![
            widget(1, 0.0, 0.0, 10.0, 10.0, 0.0),
            widget(2, 45.0, 0.0, 20.0, 40.0, 90.0),
            widget(3, 200.0, 0.0, 30.0, 10.0, 0.0),
        ];
        let before = widgets.clone();
        distribute(&mut widgets, &BTreeSet::from([1, 2, 3]), true);
        let bounds = widgets.iter().map(rotated_bounds).collect::<Vec<_>>();
        close(bounds[1][0] - bounds[0][2], bounds[2][0] - bounds[1][2]);
        assert_eq!(widgets[0], before[0]);
        assert_eq!(widgets[2], before[2]);
        let mut vertical = [0.0, 50.0, 150.0, 250.0, 300.0]
            .into_iter()
            .enumerate()
            .map(|(index, y)| widget(index as u64 + 1, 0.0, y, 10.0, 10.0, 0.0))
            .collect::<Vec<_>>();
        vertical[2].locked = true;
        vertical[1].hidden = true;
        distribute(&mut vertical, &BTreeSet::from([1, 2, 3, 4, 5]), false);
        close(vertical[1].y, 75.0);
        close(vertical[2].y, 150.0);
        close(vertical[3].y, 225.0);
        close(vertical[0].y, 0.0);
        close(vertical[4].y, 300.0);
    }
}
