use super::*;

#[test]
fn native_vector_mirror_requires_matching_geometry_and_preview() {
    let points = json!([[{"Path":{"X":0.0,"Y":0.0},"LowerHandle":null,"HigherHandle":null,"HigherSegmentExists":true},
        {"Path":{"X":1.0,"Y":1.0},"LowerHandle":null,"HigherHandle":null,"HigherSegmentExists":false}]]);
    let original = Node {
        kind: "Axure:DiagramObject:VectorShape".into(),
        properties: json!({"ControlPoints":points}),
    };
    let mut old = Widget::new(1, WidgetKind::Image, 0.0, 0.0);
    old.vector_points = Some(points.clone());
    old.rotation = 25.0;
    let image = image::RgbaImage::from_fn(2, 1, |x, _| {
        if x == 0 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 255])
        }
    });
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    old.image_data = Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(encoded.into_inner())
    ));
    let mut changed = old.clone();
    crate::vector_edit::flip_widget(&mut changed, true).unwrap();
    validate_native_vector_edit(&original, &old, &changed).unwrap();
    let mut wrong = changed.clone();
    wrong.image_data = old.image_data.clone();
    assert!(validate_native_vector_edit(&original, &old, &wrong).is_err());
    wrong.vector_points.as_mut().unwrap()[0][0]["Path"]["X"] = json!(0.37);
    assert!(validate_native_vector_edit(&original, &old, &wrong).is_err());
    crate::vector_edit::flip_widget(&mut changed, false).unwrap();
    validate_native_vector_edit(&original, &old, &changed).unwrap();
    crate::vector_edit::flip_widget(&mut changed, true).unwrap();
    validate_native_vector_edit(&original, &old, &changed).unwrap();
}

#[test]
fn unchanged_master_preview_has_exactly_unchanged_floats() {
    let mut parent = Widget::new(1, WidgetKind::Panel, 100.25, 99.8);
    parent.rotation = 27.333;
    let mut child = Widget::new(2, WidgetKind::Rectangle, 121.7, 84.31);
    child.rotation = -12.35;
    assert_eq!(transform_preview(&child, &parent, &parent), child);
}

#[test]
fn updates_legacy_rich_text_without_changing_its_schema() {
    let mut properties = json!({"rtf-block": {
        "*type":"rich-text-block", "property-dictionary":{"type":"text-object"},
        "attributed-string":{"Version":{"Major":8,"Minor":0,"Build":5,"Revision":0},
        "Paragraphs":[{"HorizontalAlignment":1,"Inlines":[{"Text":"Before","Size":10.0}]}]}
    }});
    set_text(&mut properties, "After\nNext");
    assert_eq!(widget_text(&properties), "After\nNext");
    assert!(properties.get("RichTextBlock").is_none());
    assert_eq!(
        properties["rtf-block"]["attributed-string"]["Version"]["Major"],
        8
    );
    assert_eq!(
        properties["rtf-block"]["property-dictionary"]["type"],
        "text-object"
    );
    assert_eq!(
        properties["rtf-block"]["attributed-string"]["Paragraphs"][1]["HorizontalAlignment"],
        1
    );
}
