use super::*;

#[test]
fn old_documents_default_to_fixed_and_auto_output_tracks_content() {
    let mut project = Project::blank();
    let mut raw = serde_json::to_value(&project).unwrap();
    raw["pages"][0].as_object_mut().unwrap().remove("canvas");
    let old: Project = serde_json::from_value(raw).unwrap();
    assert_eq!(old.pages[0].canvas, CanvasMode::Fixed);
    let id = project.allocate_id();
    project.pages[0]
        .widgets
        .push(Widget::new(id, WidgetKind::Rectangle, 1600.0, 1200.0));
    project.pages[0].canvas = CanvasMode::Auto;
    assert_eq!(project.pages[0].size_for_output(), [1780.0, 1300.0]);
    project.pages[0].canvas = CanvasMode::FixedWidth;
    assert_eq!(project.pages[0].size_for_output(), [1280.0, 1300.0]);
    project.pages[0].widgets[0].hidden = true;
    assert_eq!(project.pages[0].size_for_output(), [1280.0, 64.0]);
    project.validate().unwrap();
}
