//! Document-space marquee selection, constrained translation, and layer order.

use std::collections::BTreeSet;

use crate::model::Widget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxSelectionMode {
    Contained,
    Intersecting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerMove {
    ToFront,
    Forward,
    Backward,
    ToBack,
}

fn corners(widget: &Widget) -> [[f32; 2]; 4] {
    let center = [
        widget.x + widget.width * 0.5,
        widget.y + widget.height * 0.5,
    ];
    let (sin, cos) = widget.rotation.rem_euclid(360.0).to_radians().sin_cos();
    [
        [-widget.width * 0.5, -widget.height * 0.5],
        [widget.width * 0.5, -widget.height * 0.5],
        [widget.width * 0.5, widget.height * 0.5],
        [-widget.width * 0.5, widget.height * 0.5],
    ]
    .map(|point| {
        [
            center[0] + point[0] * cos - point[1] * sin,
            center[1] + point[0] * sin + point[1] * cos,
        ]
    })
}

fn projected(points: &[[f32; 2]; 4], axis: [f32; 2]) -> [f32; 2] {
    points
        .iter()
        .map(|point| point[0] * axis[0] + point[1] * axis[1])
        .fold([f32::INFINITY, f32::NEG_INFINITY], |range, value| {
            [range[0].min(value), range[1].max(value)]
        })
}

fn intersects(left: &[[f32; 2]; 4], right: &[[f32; 2]; 4]) -> bool {
    let axes = [
        [1.0, 0.0],
        [0.0, 1.0],
        [left[1][0] - left[0][0], left[1][1] - left[0][1]],
        [left[3][0] - left[0][0], left[3][1] - left[0][1]],
    ];
    axes.into_iter().all(|axis| {
        let a = projected(left, axis);
        let b = projected(right, axis);
        a[0] <= b[1] && b[0] <= a[1]
    })
}

/// Select visible, unlocked widgets by their rotated local frames. The drag
/// endpoints can arrive in either order. `Intersecting` uses separating axes,
/// avoiding the false hits produced by a rotated frame's axis-aligned bounds.
pub fn box_select(
    widgets: &[Widget],
    from: [f32; 2],
    to: [f32; 2],
    mode: BoxSelectionMode,
) -> BTreeSet<u64> {
    if !from.into_iter().chain(to).all(f32::is_finite) {
        return BTreeSet::new();
    }
    let min = [from[0].min(to[0]), from[1].min(to[1])];
    let max = [from[0].max(to[0]), from[1].max(to[1])];
    let rectangle = [min, [max[0], min[1]], max, [min[0], max[1]]];
    widgets
        .iter()
        .filter(|widget| !widget.hidden && !widget.locked)
        .filter_map(|widget| {
            let frame = corners(widget);
            if !frame.iter().flatten().all(|value| value.is_finite()) {
                return None;
            }
            let selected = match mode {
                BoxSelectionMode::Contained => frame.iter().all(|point| {
                    point[0] >= min[0]
                        && point[0] <= max[0]
                        && point[1] >= min[1]
                        && point[1] <= max[1]
                }),
                BoxSelectionMode::Intersecting => intersects(&frame, &rectangle),
            };
            selected.then_some(widget.id)
        })
        .collect()
}

/// Shift-style movement follows the dominant document axis; ties choose X.
/// Pass the total drag delta from its start so the constraint does not drift.
pub fn constrained_delta(delta: [f32; 2], constrain: bool) -> [f32; 2] {
    if !delta.into_iter().all(f32::is_finite) {
        return [0.0, 0.0];
    }
    if !constrain {
        return delta;
    }
    if delta[0].abs() >= delta[1].abs() {
        [delta[0], 0.0]
    } else {
        [0.0, delta[1]]
    }
}

/// Apply one common delta, preserving the spacing within a selection. Explicit
/// hidden selections participate; locked widgets remain stationary.
pub fn translate(widgets: &mut [Widget], ids: &BTreeSet<u64>, delta: [f32; 2], constrain: bool) {
    let delta = constrained_delta(delta, constrain);
    for widget in widgets
        .iter_mut()
        .filter(|widget| ids.contains(&widget.id) && !widget.locked)
    {
        let x = widget.x + delta[0];
        let y = widget.y + delta[1];
        if x.is_finite() && y.is_finite() {
            widget.x = x;
            widget.y = y;
        }
    }
}

/// Layers are stored back-to-front. Move all unlocked selected widgets while
/// retaining their mutual order and the mutual order of unselected widgets.
pub fn reorder(widgets: &mut [Widget], ids: &BTreeSet<u64>, movement: LayerMove) {
    let selected = |widget: &Widget| ids.contains(&widget.id) && !widget.locked;
    match movement {
        LayerMove::Forward => {
            for index in (0..widgets.len().saturating_sub(1)).rev() {
                if selected(&widgets[index]) && !selected(&widgets[index + 1]) {
                    widgets.swap(index, index + 1);
                }
            }
        }
        LayerMove::Backward => {
            for index in 1..widgets.len() {
                if selected(&widgets[index]) && !selected(&widgets[index - 1]) {
                    widgets.swap(index, index - 1);
                }
            }
        }
        LayerMove::ToFront => {
            let mut end = widgets.len();
            for index in (0..widgets.len()).rev() {
                if selected(&widgets[index]) {
                    widgets[index..end].rotate_left(1);
                    end -= 1;
                }
            }
        }
        LayerMove::ToBack => {
            let mut start = 0;
            for index in 0..widgets.len() {
                if selected(&widgets[index]) {
                    widgets[start..=index].rotate_right(1);
                    start += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WidgetKind;

    fn widget(id: u64) -> Widget {
        let mut widget = Widget::new(id, WidgetKind::Rectangle, 0.0, 0.0);
        widget.width = 100.0;
        widget.height = 20.0;
        widget
    }

    #[test]
    fn rotated_marquee_rejects_empty_aabb_corners_and_accepts_contact() {
        let mut shape = widget(1);
        shape.rotation = 45.0;
        assert!(
            box_select(
                &[shape.clone()],
                [7.0, 48.0],
                [12.0, 53.0],
                BoxSelectionMode::Intersecting
            )
            .is_empty()
        );
        assert_eq!(
            box_select(
                &[shape.clone()],
                [40.0, 0.0],
                [60.0, 20.0],
                BoxSelectionMode::Intersecting
            ),
            BTreeSet::from([1])
        );
        assert!(
            box_select(
                &[shape.clone()],
                [0.0, 0.0],
                [100.0, 20.0],
                BoxSelectionMode::Contained
            )
            .is_empty()
        );
        assert_eq!(
            box_select(
                &[shape],
                [110.0, 70.0],
                [-10.0, -50.0],
                BoxSelectionMode::Contained
            ),
            BTreeSet::from([1])
        );
    }

    #[test]
    fn marquee_excludes_hidden_locked_and_invalid_input() {
        let mut shapes = vec![widget(1), widget(2), widget(3)];
        shapes[1].hidden = true;
        shapes[2].locked = true;
        assert_eq!(
            box_select(
                &shapes,
                [-1.0, -1.0],
                [101.0, 21.0],
                BoxSelectionMode::Contained
            ),
            BTreeSet::from([1])
        );
        assert!(
            box_select(
                &shapes,
                [f32::NAN, 0.0],
                [100.0, 20.0],
                BoxSelectionMode::Intersecting
            )
            .is_empty()
        );
    }

    #[test]
    fn movement_uses_one_dominant_axis_and_keeps_locked_widgets_fixed() {
        let mut shapes = vec![widget(1), widget(2), widget(3)];
        shapes[1].x = 40.0;
        shapes[2].locked = true;
        translate(&mut shapes, &BTreeSet::from([1, 2, 3]), [12.0, -5.0], true);
        assert_eq!((shapes[0].x, shapes[0].y), (12.0, 0.0));
        assert_eq!(shapes[1].x - shapes[0].x, 40.0);
        assert_eq!((shapes[2].x, shapes[2].y), (0.0, 0.0));
        assert_eq!(constrained_delta([2.0, -7.0], true), [0.0, -7.0]);
        assert_eq!(constrained_delta([f32::INFINITY, 1.0], false), [0.0, 0.0]);
    }

    #[test]
    fn layer_moves_preserve_selected_relative_order() {
        for (mode, expected) in [
            (LayerMove::ToFront, vec![2, 4, 5, 1, 3]),
            (LayerMove::ToBack, vec![1, 3, 2, 4, 5]),
            (LayerMove::Forward, vec![2, 1, 4, 3, 5]),
            (LayerMove::Backward, vec![1, 3, 2, 4, 5]),
        ] {
            let mut shapes = (1..=5).map(widget).collect::<Vec<_>>();
            reorder(&mut shapes, &BTreeSet::from([1, 3]), mode);
            assert_eq!(shapes.iter().map(|w| w.id).collect::<Vec<_>>(), expected);
        }
        let mut shapes = (1..=4).map(widget).collect::<Vec<_>>();
        reorder(&mut shapes, &BTreeSet::from([2, 3]), LayerMove::Forward);
        assert_eq!(
            shapes.iter().map(|w| w.id).collect::<Vec<_>>(),
            [1, 4, 2, 3]
        );
    }

    #[test]
    fn locked_selected_layers_are_not_moved_as_selected_content() {
        let mut shapes = (1..=3).map(widget).collect::<Vec<_>>();
        shapes[1].locked = true;
        reorder(&mut shapes, &BTreeSet::from([1, 2]), LayerMove::ToFront);
        assert_eq!(shapes.iter().map(|w| w.id).collect::<Vec<_>>(), [2, 3, 1]);
        reorder(&mut [], &BTreeSet::new(), LayerMove::Forward);
    }
}
