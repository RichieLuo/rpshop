use super::*;
use serde_json::json;

#[test]
fn resolves_contained_text_and_its_font() {
    let child = json!({
        "rtf-block": {"attributed-string": {"Paragraphs": [
            {"Inlines": [{"Text": "First", "Size": 12.0}]},
            {"Inlines": [{"Text": "Second", "Size": 12.0}]}
        ]}}
    });
    let graph = Graph::new(
        [8, 0, 5, 0],
        "shape".into(),
        "page".into(),
        BTreeMap::from([(
            "text".into(),
            Node {
                kind: "Axure:DiagramObject:RichTextPanel".into(),
                properties: child,
            },
        )]),
    );
    let properties = json!({"diagram-objects": [{"$ref": "text"}]});
    let resolved = text_properties(&properties, &graph);
    assert_eq!(widget_text(resolved), "First\nSecond");
    assert_eq!(first_inline(resolved).unwrap()["Size"], 12.0);
}

#[test]
fn keeps_current_text_ahead_of_contained_text() {
    let graph = Graph::new([11, 0, 0, 0], "root".into(), "page".into(), BTreeMap::new());
    let properties = json!({
        "RichTextBlock": {"attributed-string": {"Paragraphs": [
            {"Inlines": [{"Text": "Current"}]}
        ]}},
        "diagram-objects": [{"$ref": "missing"}]
    });
    assert_eq!(widget_text(text_properties(&properties, &graph)), "Current");
}
