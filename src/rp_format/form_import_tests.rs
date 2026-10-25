use super::*;
use serde_json::json;

#[test]
fn native_forms_keep_types_options_labels_and_radio_groups() {
    let mut graph = Graph::new(
        [11, 0, 0, 0],
        "root".into(),
        "forms".into(),
        BTreeMap::new(),
    );
    for (id, text) in [("one", "First"), ("two", "Second")] {
        graph.objects.insert(
            id.into(),
            Node {
                kind: "Axure:SelectObject".into(),
                properties: json!({"object":text,"selected":false}),
            },
        );
    }
    let roots = BTreeMap::new();
    let styles = Styles::new(&graph);
    let mut converter = Converter {
        graph: &graph,
        roots: &roots,
        styles: &styles,
        next_id: 1,
        warnings: BTreeMap::new(),
        pending_links: Vec::new(),
        bindings: BTreeMap::new(),
        preview_parents: BTreeMap::new(),
        group_bindings: BTreeMap::new(),
    };
    for (native, expected) in [
        ("RadioButton", WidgetKind::Radio),
        ("TextArea", WidgetKind::TextArea),
        ("ComboBox", WidgetKind::Droplist),
        ("ListBox", WidgetKind::ListBox),
    ] {
        let node = Node {
            kind: format!("Axure:DiagramObject:{native}"),
            properties: json!({
                "name-block":{"text":"Component label"}, "TextBlock":{"text":"Line 1\nLine 2"},
                "radio-group":"Actual group", "list-options":[{"$ref":"one"},{"$ref":"two"}]
            }),
        };
        let widget = converter
            .visual_widget(native, &node, &BTreeMap::new(), (4.0, 8.0), false)
            .unwrap();
        assert_eq!(widget.kind, expected);
        assert_eq!(widget.name, "Component label");
        assert_eq!(
            widget.text,
            if expected.is_option_list() {
                "First\nSecond"
            } else {
                "Line 1\nLine 2"
            }
        );
        assert_eq!(
            widget.radio_group.as_deref(),
            if expected == WidgetKind::Radio {
                Some("Actual group")
            } else {
                None
            }
        );
    }
}
