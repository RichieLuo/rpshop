//! Edit retained native vector contours without applying a generic glyph flip.
//! Reflections affect normalized anchors/handles and reverse object rotation.

use anyhow::{Context, Result, ensure};
use serde_json::{Number, Value};

use crate::model::Widget;

const MAX_CONTOURS: usize = 65_536;
const MAX_POINTS: usize = 20_000;
const MAX_COORDINATE: f64 = 1_000_000.0;
const MAX_VALUES: usize = 1_000_000;
const MAX_DEPTH: usize = 32;

/// Check the fixed contour/point structure and all coordinates before editing.
/// A missing/null handle or a handle with both coordinates null is an empty
/// native handle; unknown properties are retained within bounded depth/size.
pub fn validate_points(points: &Value) -> Result<()> {
    let contours = points.as_array().context("矢量路径应为轮廓数组")?;
    ensure!(
        !contours.is_empty() && contours.len() <= MAX_CONTOURS,
        "矢量轮廓数量应在 1–65536 之间"
    );
    let mut count = 0usize;
    for contour in contours {
        let contour = contour.as_array().context("矢量轮廓应为控制点数组")?;
        count = count
            .checked_add(contour.len())
            .context("矢量控制点数量溢出")?;
        ensure!(count <= MAX_POINTS, "矢量控制点不能超过 20000 个");
        for pair in contour {
            let pair = pair.as_object().context("矢量控制点应为对象")?;
            coordinate(pair.get("Path").context("矢量控制点缺少 Path")?, false)?;
            for name in ["LowerHandle", "HigherHandle"] {
                if let Some(handle) = pair.get(name) {
                    coordinate(handle, true)?;
                }
            }
            if let Some(segment) = pair.get("HigherSegmentExists") {
                ensure!(segment.is_boolean(), "矢量线段标记应为布尔值");
            }
        }
    }
    ensure!(count > 0, "矢量路径没有控制点");
    bounded_value(points, 0, &mut 0)?;
    Ok(())
}

fn coordinate(value: &Value, nullable: bool) -> Result<Option<[f64; 2]>> {
    if nullable && value.is_null() {
        return Ok(None);
    }
    let point = value.as_object().context("矢量坐标应为 X/Y 对象")?;
    let x = point.get("X").context("矢量坐标缺少 X")?;
    let y = point.get("Y").context("矢量坐标缺少 Y")?;
    if nullable && x.is_null() && y.is_null() {
        return Ok(None);
    }
    let x = x.as_f64().context("矢量 X 坐标应为有限数字")?;
    let y = y.as_f64().context("矢量 Y 坐标应为有限数字")?;
    ensure!(
        x.is_finite() && y.is_finite() && x.abs() <= MAX_COORDINATE && y.abs() <= MAX_COORDINATE,
        "矢量坐标应为 ±1000000 以内的有限数字"
    );
    Ok(Some([x, y]))
}

fn bounded_value(value: &Value, depth: usize, count: &mut usize) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "矢量路径附加数据嵌套过深");
    *count += 1;
    ensure!(*count <= MAX_VALUES, "矢量路径附加数据过多");
    match value {
        Value::Array(values) => {
            for value in values {
                bounded_value(value, depth + 1, count)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                bounded_value(value, depth + 1, count)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Reflect every anchor and nonempty Bézier handle, keeping other data exact.
pub fn mirrored_points(points: &Value, horizontal: bool) -> Result<Value> {
    validate_points(points)?;
    let mut result = points.clone();
    for contour in result.as_array_mut().unwrap() {
        for pair in contour.as_array_mut().unwrap() {
            for name in ["Path", "LowerHandle", "HigherHandle"] {
                if let Some(point) = pair.get_mut(name)
                    && coordinate(point, name != "Path")?.is_some()
                {
                    let component = point.get_mut(if horizontal { "X" } else { "Y" }).unwrap();
                    *component = if let Some(integer) = component.as_i64() {
                        Value::Number(Number::from(1 - integer))
                    } else {
                        Value::Number(
                            Number::from_f64(1.0 - component.as_f64().unwrap())
                                .context("矢量反射结果不是有限数字")?,
                        )
                    };
                }
            }
        }
    }
    validate_points(&result)?;
    Ok(result)
}

/// Recognize a strict horizontal/vertical reflection, including all metadata.
pub fn is_mirror(original: &Value, changed: &Value) -> bool {
    [true, false].into_iter().any(|horizontal| {
        mirrored_points(original, horizontal).is_ok_and(|points| points == *changed)
    })
}

/// Reflect retained geometry and reverse rotation. Optional image previews
/// are flipped before any mutation, keeping a failed operation fully atomic.
pub fn flip_widget(widget: &mut Widget, horizontal: bool) -> Result<()> {
    ensure!(
        widget.rotation.is_finite() && widget.rotation.abs() <= 36_000.0,
        "矢量旋转角度无效"
    );
    let original = widget
        .vector_points
        .as_ref()
        .context("该元件未保留可编辑矢量路径")?;
    let points = mirrored_points(original, horizontal)?;
    let preview = widget
        .image_data
        .as_deref()
        .map(|image| crate::bitmap::flip_data_uri(image, horizontal))
        .transpose()?;
    widget.vector_points = Some(points);
    widget.image_data = preview;
    widget.rotation = (360.0 - widget.rotation).rem_euclid(360.0);
    Ok(())
}

/// First two usable anchors of the first contour, in document coordinates
/// before object rotation. Legacy/simple lines retain their diagonal fallback.
pub fn line_endpoints(widget: &Widget) -> [[f32; 2]; 2] {
    let fallback = [
        [widget.x, widget.y],
        [widget.x + widget.width, widget.y + widget.height],
    ];
    let Some(contour) = widget
        .vector_points
        .as_ref()
        .and_then(Value::as_array)
        .and_then(|contours| contours.first())
        .and_then(Value::as_array)
    else {
        return fallback;
    };
    let mut anchors = contour.iter().filter_map(|pair| {
        let [x, y] = coordinate(pair.get("Path")?, false).ok()??;
        Some([
            widget.x + x as f32 * widget.width,
            widget.y + y as f32 * widget.height,
        ])
    });
    match (anchors.next(), anchors.next()) {
        (Some(first), Some(second)) => [first, second],
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WidgetKind;
    use serde_json::json;

    fn paths() -> Value {
        json!([[{"Path":{"X":0.25,"Y":0.0,"tag":"keep"}, "LowerHandle":null,
            "HigherHandle":{"X":0.75,"Y":0.5},"HigherSegmentExists":true,"custom":[1,{"keep":true}]},
            {"Path":{"X":1.0,"Y":1.0},"LowerHandle":{"X":null,"Y":null},"HigherSegmentExists":false}]])
    }

    #[test]
    fn reflections_preserve_unknown_properties_null_handles_and_segment_flags() {
        let original = paths();
        let reflected = mirrored_points(&original, true).unwrap();
        assert_eq!(reflected[0][0]["Path"]["X"], 0.75);
        assert_eq!(reflected[0][0]["HigherHandle"]["X"], 0.25);
        assert_eq!(reflected[0][0]["custom"], original[0][0]["custom"]);
        assert_eq!(reflected[0][0]["Path"]["tag"], "keep");
        assert_eq!(reflected[0][0]["LowerHandle"], Value::Null);
        assert_eq!(
            reflected[0][1]["LowerHandle"],
            original[0][1]["LowerHandle"]
        );
        assert_eq!(reflected[0][1]["HigherSegmentExists"], false);
        assert!(is_mirror(&original, &reflected));
        let mut edited_metadata = reflected;
        edited_metadata[0][0]["Path"]["tag"] = json!("changed");
        assert!(!is_mirror(&original, &edited_metadata));
    }

    #[test]
    fn double_flip_restores_geometry_rotation_and_line_direction() {
        for horizontal in [true, false] {
            let mut widget = Widget::new(2, WidgetKind::Line, 10.0, 20.0);
            widget.width = 100.0;
            widget.height = 40.0;
            widget.rotation = 37.5;
            widget.vector_points = Some(paths());
            let original = widget.clone();
            flip_widget(&mut widget, horizontal).unwrap();
            assert_eq!(widget.rotation, 322.5);
            let points = line_endpoints(&widget);
            assert_eq!(
                points[0],
                if horizontal {
                    [85.0, 20.0]
                } else {
                    [35.0, 60.0]
                }
            );
            flip_widget(&mut widget, horizontal).unwrap();
            assert_eq!(widget, original);
        }
    }

    #[test]
    fn invalid_geometry_or_preview_leaves_the_entire_widget_unchanged() {
        let mut widget = Widget::new(2, WidgetKind::Image, 10.0, 20.0);
        widget.vector_points = Some(paths());
        widget.rotation = 37.5;
        widget.image_data = Some("data:image/png;base64,broken!".into());
        let original = widget.clone();
        assert!(flip_widget(&mut widget, true).is_err());
        assert_eq!(widget, original);
        widget.image_data = None;
        widget.vector_points.as_mut().unwrap()[0][0]["Path"]["X"] = json!(1_000_001);
        let original = widget.clone();
        assert!(flip_widget(&mut widget, false).is_err());
        assert_eq!(widget, original);
        assert!(validate_points(&json!([[{"Path":{"X":null,"Y":0}}]])).is_err());
        assert!(
            validate_points(&json!([[{"Path":{"X":0,"Y":0},"LowerHandle":{"X":null,"Y":1}}]]))
                .is_err()
        );
    }

    #[test]
    fn legacy_widgets_have_no_vector_path_and_default_line_endpoints() {
        let widget = Widget::new(2, WidgetKind::Line, 10.0, 20.0);
        let mut old = serde_json::to_value(&widget).unwrap();
        old.as_object_mut().unwrap().remove("vector_points");
        let decoded: Widget = serde_json::from_value(old).unwrap();
        assert!(decoded.vector_points.is_none());
        assert_eq!(line_endpoints(&decoded), [[10.0, 20.0], [250.0, 22.0]]);
    }

    #[test]
    fn rejects_excessive_point_count_contour_count_and_nested_metadata() {
        let point = json!({"Path":{"X":0.0,"Y":0.0}});
        assert!(
            validate_points(&Value::Array(vec![Value::Array(vec![
                point;
                MAX_POINTS + 1
            ])]))
            .is_err()
        );
        assert!(validate_points(&Value::Array(vec![json!([]); MAX_CONTOURS + 1])).is_err());
        let mut points = paths();
        let mut nested = Value::Null;
        for _ in 0..MAX_DEPTH {
            nested = json!({"nested":nested});
        }
        points[0][0]["custom"] = nested;
        assert!(validate_points(&points).is_err());
    }
}
