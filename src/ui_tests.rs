use super::*;
use eframe::App as _;

#[path = "keyboard_tests.rs"]
mod keyboard;
#[path = "navigation_tests.rs"]
mod navigation;

#[path = "sidebar_tests.rs"]
mod sidebar_tests;

#[path = "browser_preview_tests.rs"]
mod browser_preview;

#[path = "ui_interaction_tests.rs"]
mod interaction_editor;

#[derive(Default)]
struct MemoryStorage {
    values: HashMap<String, String>,
    writes: usize,
}

impl eframe::Storage for MemoryStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        self.writes += 1;
        self.values.insert(key.to_owned(), value);
    }

    fn flush(&mut self) {}
}

pub(super) fn fixture(project: Project) -> App {
    App {
        saved: project.clone(),
        project,
        page: 0,
        selected: BTreeSet::new(),
        path: None,
        undo: Vec::new(),
        redo: Vec::new(),
        clipboard: Vec::new(),
        zoom: 0.5,
        view: CanvasView::default(),
        sidebar_layout: sidebar::SidebarLayout::default(),
        fit: false,
        grid: false,
        snap: true,
        tab: 0,
        tool: None,
        drag: None,
        pending_widget: None,
        library_query: String::new(),
        library_category: None,
        browser_preview: None,
        textures: HashMap::new(),
        logo: None,
        status: String::new(),
        last_recovery: Instant::now(),
        screenshot: None,
        // Headless UI checks must never write recovery files into the user's profile.
        screenshot_mode: true,
        history_changed: false,
        editing: None,
        frames: 0,
        confirm: None,
        help: false,
        import_warnings: Vec::new(),
        pending_open: None,
        native: None,
        cached_native_preview_source: None,
    }
}

pub(super) fn frame(
    app: &mut App,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1500.0, 950.0))),
        time: Some(f64::from(app.frames) / 60.0),
        events,
        ..Default::default()
    };
    let mut native_frame = eframe::Frame::_new_kittest();
    ctx.run(input, |ctx| app.update(ctx, &mut native_frame))
}

pub(super) fn text_center(output: &egui::FullOutput, text: &str) -> Pos2 {
    fn find(shape: &egui::epaint::Shape, text: &str) -> Option<Pos2> {
        match shape {
            egui::epaint::Shape::Text(shape) if shape.galley.text() == text => {
                Some(shape.pos + shape.galley.size() / 2.0)
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, text)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, text))
        .unwrap_or_else(|| panic!("UI text {text:?} was not painted"))
}

fn pointer(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

pub(super) fn click(app: &mut App, ctx: &egui::Context, pos: Pos2) -> egui::FullOutput {
    let _ = frame(
        app,
        ctx,
        vec![egui::Event::PointerMoved(pos), pointer(pos, true)],
    );
    frame(app, ctx, vec![pointer(pos, false)])
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn add(project: &mut Project, kind: WidgetKind, x: f32, y: f32) -> u64 {
    let id = project.allocate_id();
    project.pages[0].widgets.push(Widget::new(id, kind, x, y));
    id
}

#[test]
fn layer_icons_toggle_visibility_and_lock_without_selecting_the_row() {
    let mut project = Project::blank();
    add(&mut project, WidgetKind::Text, 80.0, 80.0);
    project.pages[0].widgets[0].name = "layer".into();
    let mut app = fixture(project);
    let ctx = egui::Context::default();
    let output = frame(&mut app, &ctx, Vec::new());
    let row_y = text_center(&output, "T layer").y;
    let visible = Pos2::new(text_center(&output, "显示").x, row_y);
    let locked = Pos2::new(text_center(&output, "锁定").x, row_y);

    let _ = click(&mut app, &ctx, visible);
    assert!(app.project.pages[0].widgets[0].hidden);
    assert!(!app.project.pages[0].widgets[0].locked);
    assert!(app.selected.is_empty());

    let _ = click(&mut app, &ctx, locked);
    assert!(app.project.pages[0].widgets[0].hidden);
    assert!(app.project.pages[0].widgets[0].locked);
    assert!(app.selected.is_empty());
    assert_eq!(app.undo.len(), 2, "each control click is one edit");

    let _ = click(&mut app, &ctx, visible);
    let _ = click(&mut app, &ctx, locked);
    assert!(!app.project.pages[0].widgets[0].hidden);
    assert!(!app.project.pages[0].widgets[0].locked);
}

#[test]
fn many_long_layer_names_do_not_expand_the_default_navigation_panel() {
    let mut project = Project::blank();
    for index in 0..46 {
        add(&mut project, WidgetKind::Text, 80.0, 80.0);
        project.pages[0].widgets[index].name = format!(
            "{index} a very long layer name that must be truncated in the navigation panel"
        );
    }
    let mut app = fixture(project);
    let ctx = egui::Context::default();
    for _ in 0..10 {
        let output = frame(&mut app, &ctx, Vec::new());
        let _ = text_center(&output, "显示");
        let _ = text_center(&output, "锁定");
        let _ = text_center(&output, "眼睛控制显示，锁控制编辑");
        let panel = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("navigator"))
            .expect("the navigation panel must be laid out");
        assert!(
            (panel.rect.width() - 225.0).abs() < 1.0,
            "layer content must not resize the default panel: {}",
            panel.rect.width(),
        );
    }
}

#[test]
fn toolbar_undo_keeps_redo_and_does_not_create_a_reverse_history_entry() {
    let mut app = fixture(Project::blank());
    let original = app.project.clone();
    app.add_widget(WidgetKind::Rectangle, 80.0, 80.0);
    app.remember(original.clone());
    let edited = app.project.clone();
    let ctx = egui::Context::default();
    let output = frame(&mut app, &ctx, Vec::new());

    let undo = text_center(&output, "撤销");
    let output = click(&mut app, &ctx, undo);
    assert_eq!(app.project, original);
    assert!(
        app.undo.is_empty(),
        "the outer frame must not record Undo itself"
    );
    assert_eq!(app.redo.len(), 1, "Undo must leave Redo available");

    let redo = text_center(&output, "重做");
    let _ = click(&mut app, &ctx, redo);
    assert_eq!(app.project, edited);
    assert_eq!(app.undo.len(), 1);
    assert!(app.redo.is_empty());
}

#[test]
fn keyboard_duplicate_is_one_transaction_and_a_new_edit_invalidates_redo() {
    let mut project = Project::blank();
    let id = add(&mut project, WidgetKind::Button, 80.0, 80.0);
    let original = project.clone();
    let mut app = fixture(project);
    app.selected.insert(id);
    let ctx = egui::Context::default();
    let _ = frame(&mut app, &ctx, Vec::new());
    let _ = frame(
        &mut app,
        &ctx,
        vec![key(egui::Key::D, egui::Modifiers::CTRL)],
    );
    assert_eq!(app.project.pages[0].widgets.len(), 2);
    assert_eq!(app.undo.len(), 1);

    app.undo();
    assert_eq!(app.project, original);
    assert_eq!(app.redo.len(), 1);
    let before = app.project.clone();
    app.add_widget(WidgetKind::Text, 12.0, 24.0);
    app.remember(before);
    assert!(
        app.redo.is_empty(),
        "editing after Undo must start a new branch"
    );
}

#[test]
fn pointer_drag_across_multiple_frames_is_one_undoable_transaction() {
    let mut project = Project::blank();
    let id = add(&mut project, WidgetKind::Rectangle, 80.0, 80.0);
    let original = project.clone();
    let mut app = fixture(project);
    let ctx = egui::Context::default();
    let output = frame(&mut app, &ctx, Vec::new());
    let page = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Rect(shape)
                if shape.fill == Color32::WHITE
                    && (shape.rect.width() - 640.0).abs() < 0.1
                    && (shape.rect.height() - 400.0).abs() < 0.1 =>
            {
                Some(shape.rect)
            }
            _ => None,
        })
        .expect("the document rectangle must be painted at the selected zoom");
    let start = page.min + vec2(50.0, 50.0);
    let _ = frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(start), pointer(start, true)],
    );
    let middle = start + vec2(40.0, 24.0);
    let _ = frame(&mut app, &ctx, vec![egui::Event::PointerMoved(middle)]);
    let end = start + vec2(48.0, 32.0);
    let _ = frame(&mut app, &ctx, vec![egui::Event::PointerMoved(end)]);
    assert!(
        app.undo.is_empty(),
        "intermediate drag frames must not fill history"
    );
    let _ = frame(&mut app, &ctx, vec![pointer(end, false)]);

    let widget = &app.project.pages[0].widgets[0];
    assert_eq!(widget.id, id);
    assert_eq!((widget.x, widget.y), (176.0, 144.0));
    assert_eq!(app.undo.len(), 1);
    assert!(app.drag.is_none());
    app.undo();
    assert_eq!(app.project, original);
    app.redo();
    assert_eq!(
        (
            app.project.pages[0].widgets[0].x,
            app.project.pages[0].widgets[0].y
        ),
        (176.0, 144.0)
    );
}

#[test]
fn line_rotation_handle_drag_is_visible_and_one_undoable_edit() {
    let mut project = Project::blank();
    let id = add(&mut project, WidgetKind::Line, 160.0, 160.0);
    let original = project.clone();
    let mut app = fixture(project);
    app.selected.insert(id);
    let ctx = egui::Context::default();
    let output = frame(&mut app, &ctx, Vec::new());
    let start = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Circle(circle)
                if circle.stroke.color == PURPLE && (circle.radius - 4.5).abs() < 0.01 =>
            {
                Some(circle.center)
            }
            _ => None,
        })
        .expect("selected lines must expose a rotation handle");
    let center = start + vec2(0.0, (24.0 + 1.0) * app.zoom);
    let end = center + vec2((24.0 + 1.0) * app.zoom, 0.0);
    let _ = frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(start), pointer(start, true)],
    );
    let _ = frame(&mut app, &ctx, vec![egui::Event::PointerMoved(end)]);
    assert!(app.undo.is_empty());
    let _ = frame(&mut app, &ctx, vec![pointer(end, false)]);
    let line = &app.project.pages[0].widgets[0];
    assert!(
        (line.rotation - 90.0).abs() < 0.01,
        "angle {}",
        line.rotation
    );
    assert_eq!(
        (line.x, line.y, line.width, line.height),
        (160.0, 160.0, 240.0, 2.0)
    );
    assert_eq!(app.undo.len(), 1);
    app.undo();
    assert_eq!(app.project, original);
    app.redo();
    assert!((app.project.pages[0].widgets[0].rotation - 90.0).abs() < 0.01);
}

#[test]
fn drag_constraints_preserve_relative_offsets_and_rotated_resize_anchor() {
    let mut project = Project::blank();
    let first = add(&mut project, WidgetKind::Rectangle, 13.0, 25.0);
    let second = add(&mut project, WidgetKind::Line, 248.0, 103.0);
    let before = project.pages[0].clone();
    let ids = BTreeSet::from([first, second]);
    apply_drag(
        &mut project.pages[0],
        &before,
        &ids,
        &DragMode::Move,
        [13.0, 7.0],
        true,
        false,
        true,
    );
    assert_eq!(
        (project.pages[0].widgets[0].x, project.pages[0].widgets[0].y),
        (29.0, 25.0)
    );
    assert_eq!(
        (project.pages[0].widgets[1].x, project.pages[0].widgets[1].y),
        (264.0, 103.0)
    );
    let mut old = before.clone();
    old.widgets[0].rotation = 37.0;
    let bounds = old.widgets[0].clone();
    let anchor = manipulation::handle_positions(&bounds)[0].1;
    let mut page = old.clone();
    apply_drag(
        &mut page,
        &old,
        &BTreeSet::from([first]),
        &DragMode::Resize {
            handle: Handle::SouthEast,
            bounds: Box::new(bounds),
        },
        [33.0, 29.0],
        true,
        false,
        false,
    );
    let resized = &page.widgets[0];
    let actual = manipulation::handle_positions(resized)[0].1;
    assert!((actual[0] - anchor[0]).abs() < 0.001 && (actual[1] - anchor[1]).abs() < 0.001);
    assert!(
        (resized.width / resized.height - old.widgets[0].width / old.widgets[0].height).abs()
            < 0.001
    );
}

#[test]
fn all_arrangement_buttons_preserve_locks_and_undo_as_one_action() {
    let mut project = Project::blank();
    let ids: Vec<_> = (0..4)
        .map(|i| {
            add(
                &mut project,
                WidgetKind::Rectangle,
                40.0 + i as f32 * 81.0,
                30.0 + i as f32 * 39.0,
            )
        })
        .collect();
    project.pages[0].widgets[1].locked = true;
    let original = project.clone();
    for mode in 0..14 {
        let mut app = fixture(project.clone());
        app.selected = ids.iter().copied().collect();
        app.align(mode);
        assert_eq!(
            app.project.pages[0]
                .widgets
                .iter()
                .find(|w| w.id == ids[1])
                .unwrap(),
            &original.pages[0].widgets[1],
            "mode {mode}"
        );
        if app.project != original {
            assert_eq!(app.undo.len(), 1, "mode {mode}");
            app.undo();
            assert_eq!(app.project, original);
        }
    }
}

#[test]
fn grouping_shortcuts_select_as_a_unit_and_paste_allocates_new_group_ids() {
    let mut project = Project::blank();
    let a = add(&mut project, WidgetKind::Rectangle, 80.0, 80.0);
    let b = add(&mut project, WidgetKind::Line, 200.0, 80.0);
    let mut app = fixture(project);
    app.selected = BTreeSet::from([a, b]);
    let ctx = egui::Context::default();
    let _ = frame(
        &mut app,
        &ctx,
        vec![key(egui::Key::G, egui::Modifiers::CTRL)],
    );
    let group = app.project.pages[0].widgets[0].groups[0];
    assert_eq!(app.project.pages[0].widgets[1].groups, [group]);
    assert_eq!(app.undo.len(), 1);
    app.select_hit(a, false, false);
    assert_eq!(app.selected, BTreeSet::from([a, b]));
    app.select_hit(a, false, true);
    assert_eq!(app.selected, BTreeSet::from([a]));
    app.select_hit(b, false, false);
    app.copy();
    app.paste();
    let copied = &app.project.pages[0].widgets[2..];
    assert_ne!(copied[0].groups[0], group);
    assert_eq!(copied[0].groups, copied[1].groups);
    app.project.validate().unwrap();
    let _ = frame(
        &mut app,
        &ctx,
        vec![key(
            egui::Key::G,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        )],
    );
    assert!(
        app.project.pages[0].widgets[2..]
            .iter()
            .all(|w| w.groups.is_empty())
    );
    assert_eq!(app.project.pages[0].widgets[0].groups, [group]);
}

#[test]
fn reflected_line_geometry_matches_canvas_hits_and_both_exports() {
    let mut project = Project::blank();
    let id = add(&mut project, WidgetKind::Line, 10.0, 20.0);
    let line = &mut project.pages[0].widgets[0];
    line.width = 120.0;
    line.height = 60.0;
    line.vector_points = Some(
        serde_json::json!([[{"Path":{"X":0,"Y":0},"HigherSegmentExists":true},{"Path":{"X":1,"Y":1},"HigherSegmentExists":false}]]),
    );
    let original = line.clone();
    let mut app = fixture(project);
    app.selected.insert(id);
    app.flip_selection(true);
    let line = &app.project.pages[0].widgets[0];
    assert!(manipulation::hit_test(line, [130.0, 20.0], 1.0));
    assert!(!manipulation::hit_test(line, [10.0, 20.0], 1.0));
    let html = rpshop::export::html(&app.project).unwrap();
    let svg = rpshop::export::svg(&app.project.pages[0]).unwrap();
    assert!(html.contains("x1=\"120\" y1=\"0\" x2=\"0\" y2=\"60\""));
    assert!(svg.contains("x1=\"130\" y1=\"20\" x2=\"10\" y2=\"80\""));
    assert_eq!(app.undo.len(), 1);
    app.undo();
    assert_eq!(app.project.pages[0].widgets[0], original);
}

#[test]
fn drawing_tool_drag_uses_document_coordinates_and_escape_cancels_creation() {
    let mut app = fixture(Project::blank());
    app.snap = false;
    app.tool = Some(WidgetKind::Rectangle);
    let ctx = egui::Context::default();
    let output = frame(&mut app, &ctx, Vec::new());
    let page = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Rect(shape)
                if shape.fill == Color32::WHITE
                    && (shape.rect.width() - 640.0).abs() < 0.1
                    && (shape.rect.height() - 400.0).abs() < 0.1 =>
            {
                Some(shape.rect)
            }
            _ => None,
        })
        .unwrap();
    let start = page.min + vec2(50.0, 40.0);
    let end = start + vec2(75.0, 50.0);
    let _ = frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(start), pointer(start, true)],
    );
    let _ = frame(&mut app, &ctx, vec![egui::Event::PointerMoved(end)]);
    assert!(app.undo.is_empty());
    let _ = frame(&mut app, &ctx, vec![pointer(end, false)]);
    let widget = &app.project.pages[0].widgets[0];
    assert_eq!(
        (widget.x, widget.y, widget.width, widget.height),
        (100.0, 80.0, 150.0, 100.0)
    );
    assert_eq!(app.undo.len(), 1);
    let before = app.project.clone();
    app.tool = Some(WidgetKind::Line);
    let start = page.min + vec2(200.0, 160.0);
    let end = start + vec2(50.0, -30.0);
    let _ = frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(start), pointer(start, true)],
    );
    let _ = frame(&mut app, &ctx, vec![egui::Event::PointerMoved(end)]);
    assert_eq!(app.project.pages[0].widgets.len(), 2);
    let line = app.project.pages[0].widgets.last().unwrap();
    assert_eq!(
        rpshop::vector_edit::line_endpoints(line),
        [[400.0, 320.0], [500.0, 260.0]]
    );
    let _ = frame(
        &mut app,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    let _ = frame(&mut app, &ctx, vec![pointer(end, false)]);
    assert_eq!(app.project, before);
    assert_eq!(app.undo.len(), 1);
}

#[test]
fn paste_remaps_internal_targets_but_drops_unavailable_cross_page_targets() {
    let mut project = Project::blank();
    let target = add(&mut project, WidgetKind::Text, 80.0, 80.0);
    let trigger = add(&mut project, WidgetKind::Button, 80.0, 160.0);
    project.pages[0].widgets[1].interaction.action = Action::SetText;
    project.pages[0].widgets[1].interaction.target_widget = Some(target);
    project.pages[0].widgets[1].interaction.value = "已更新".into();
    let mut app = fixture(project);
    app.selected = BTreeSet::from([target, trigger]);
    app.copy();
    app.paste();
    let copies = &app.project.pages[0].widgets[2..];
    assert_ne!(copies[0].id, target);
    assert_ne!(copies[1].id, trigger);
    assert_eq!(copies[1].interaction.target_widget, Some(copies[0].id));
    assert_eq!(app.selected, copies.iter().map(|w| w.id).collect());
    assert_eq!(
        app.project.pages[0].widgets[1].interaction.target_widget,
        Some(target)
    );
    app.project.validate().unwrap();

    // Copying only the trigger to a different page cannot retain its old local target.
    app.selected = BTreeSet::from([trigger]);
    app.copy();
    let mut page = app.project.pages[0].clone();
    page.id = app.project.allocate_id();
    page.name = "目标页面".into();
    page.widgets.clear();
    app.project.pages.push(page);
    app.page = 1;
    app.paste();
    let pasted = &app.project.pages[1].widgets[0];
    assert_eq!(pasted.interaction.action, Action::None);
    assert_eq!(pasted.interaction.target_widget, None);
    app.project.validate().unwrap();
}

#[test]
fn deleting_a_target_cleans_references_on_other_pages_and_preserves_locked_widgets() {
    let mut project = Project::blank();
    let target = add(&mut project, WidgetKind::Text, 80.0, 80.0);
    let locked = add(&mut project, WidgetKind::Rectangle, 80.0, 160.0);
    project.pages[0].widgets[1].locked = true;
    let page_id = project.allocate_id();
    let trigger_id = project.allocate_id();
    let mut trigger = Widget::new(trigger_id, WidgetKind::Button, 0.0, 0.0);
    trigger.interaction.action = Action::ToggleVisibility;
    trigger.interaction.target_widget = Some(target);
    let mut second = project.pages[0].clone();
    second.id = page_id;
    second.widgets = vec![trigger];
    project.pages.push(second);
    let original = project.clone();
    let mut app = fixture(project);
    app.selected = BTreeSet::from([target, locked]);
    app.delete_selection();
    app.remember(original.clone());

    assert_eq!(app.project.pages[0].widgets.len(), 1);
    assert_eq!(app.project.pages[0].widgets[0].id, locked);
    assert_eq!(
        app.project.pages[1].widgets[0].interaction.action,
        Action::None
    );
    assert_eq!(
        app.project.pages[1].widgets[0].interaction.target_widget,
        None
    );
    app.project.validate().unwrap();
    app.undo();
    assert_eq!(
        app.project, original,
        "Undo must restore references as well as the target"
    );
}

#[test]
fn finishing_screenshot_capture_does_not_overwrite_the_real_user_session() {
    let mut storage = MemoryStorage::default();
    let mut user_project = Project::blank();
    user_project.name = "用户现有工程".into();
    eframe::set_value(
        &mut storage,
        "rpshop-session",
        &Session {
            project: user_project.clone(),
            saved: user_project,
            path: None,
            import_warnings: Vec::new(),
            native_state: None,
        },
    );
    let original = storage.values.clone();
    let original_writes = storage.writes;
    let mut app = fixture(Project::default());
    app.screenshot = Some(PathBuf::from("capture.png"));
    // This is the transition performed when Event::Screenshot consumes its output path.
    let _ = app.screenshot.take();
    app.save(&mut storage);
    assert_eq!(storage.values, original);
    assert_eq!(storage.writes, original_writes);
}

#[test]
fn session_roundtrip_restores_unsaved_edits_and_repairs_transient_blank_names() {
    let mut project = Project::blank();
    add(&mut project, WidgetKind::Button, 123.0, 234.0);
    let mut app = fixture(project);
    app.screenshot_mode = false;
    app.project.name.clear();
    app.project.pages[0].name = "  ".into();
    app.project.pages[0].widgets[0].text = "未保存的修改".into();
    app.path = Some(PathBuf::from("existing.rpshop"));
    let saved = app.saved.clone();
    let mut storage = MemoryStorage::default();
    app.save(&mut storage);

    let ctx = egui::Context::default();
    let mut creation = eframe::CreationContext::_new_kittest(ctx);
    creation.storage = Some(&storage);
    let restored = App::new(&creation, None);
    assert_eq!(restored.project.pages[0].widgets[0].text, "未保存的修改");
    assert_eq!(
        (
            restored.project.pages[0].widgets[0].x,
            restored.project.pages[0].widgets[0].y
        ),
        (123.0, 234.0)
    );
    assert!(!restored.project.name.trim().is_empty());
    assert!(!restored.project.pages[0].name.trim().is_empty());
    assert_eq!(restored.saved, saved);
    assert_ne!(
        restored.project, restored.saved,
        "recovery must preserve the dirty state"
    );
    assert_eq!(restored.path, Some(PathBuf::from("existing.rpshop")));
    restored.project.validate().unwrap();
}

#[test]
fn escape_clears_the_active_drawing_tool_without_closing_browser_preview() {
    let mut app = fixture(Project::blank());
    app.tool = Some(WidgetKind::Rectangle);
    let ctx = egui::Context::default();
    app.start_preview(&ctx);
    let _ = frame(&mut app, &ctx, Vec::new());
    let _ = frame(
        &mut app,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(app.browser_preview.is_some());
    assert!(app.tool.is_none());
}

#[test]
fn save_cannot_overwrite_native_files_without_a_native_source_graph() {
    let path = std::env::temp_dir().join(format!(
        "rpshop-ui-save-guard-{}-{}.rp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let bytes = b"original-native-document";
    std::fs::write(&path, bytes).unwrap();
    let mut app = fixture(Project::blank());
    assert!(app.write_document(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn native_ctrl_s_keeps_rp_format_and_session_recovers_unsaved_native_edits() {
    let source = std::path::Path::new(
        r"D:\Program Files (x86)\Axure\Axure RP 11\DefaultSettings\Training\Quick Win.rp",
    );
    if !source.is_file() {
        return;
    }
    let source_bytes = std::fs::read(source).unwrap();
    let path = std::env::temp_dir().join(format!(
        "rpshop-ui-native-{}-{}.rp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, &source_bytes).unwrap();
    let document = load_document(&path).unwrap();
    assert_eq!(document.save_path.as_ref(), Some(&path));
    assert!(document.native.is_some());
    let mut app = fixture(document.project);
    app.path = document.save_path;
    app.native = document.native;
    app.screenshot_mode = false;
    let widget = app.project.pages[0]
        .widgets
        .iter_mut()
        .find(|w| !w.text.is_empty() && w.kind != WidgetKind::Image)
        .unwrap();
    widget.text = "Ctrl+S 原生保存成功".into();
    assert!(app.save_project(false), "{}", app.status);
    assert_eq!(std::fs::read(&path).unwrap()[..4], [0xac, 0xef, 11, 0]);
    let reopened = load_document(&path).unwrap();
    assert!(
        reopened.project.pages[0]
            .widgets
            .iter()
            .any(|w| w.text == "Ctrl+S 原生保存成功")
    );
    assert_eq!(app.project, app.saved);
    let widget = app.project.pages[0]
        .widgets
        .iter_mut()
        .find(|w| w.text == "Ctrl+S 原生保存成功")
        .unwrap();
    widget.text = "恢复尚未保存的原生修改".into();
    let mut storage = MemoryStorage::default();
    app.save(&mut storage);
    let mut creation = eframe::CreationContext::_new_kittest(egui::Context::default());
    creation.storage = Some(&storage);
    let mut restored = App::new(&creation, None);
    assert!(restored.native.is_some(), "{}", restored.status);
    assert_ne!(restored.project, restored.saved);
    assert!(restored.save_project(false), "{}", restored.status);
    assert!(
        load_document(&path).unwrap().project.pages[0]
            .widgets
            .iter()
            .any(|w| w.text == "恢复尚未保存的原生修改")
    );
    assert_eq!(std::fs::read(source).unwrap(), source_bytes);
    std::fs::remove_file(path).unwrap();
}

fn native_tree_upgrade_fixture() -> Option<(PathBuf, Vec<u8>, OpenedDocument)> {
    let source = std::path::Path::new(
        r"D:\Program Files (x86)\Axure\Axure RP 11\DefaultSettings\Training\Quick Win.rp",
    );
    if !source.is_file() {
        return None;
    }
    let bytes = std::fs::read(source).unwrap();
    let path = std::env::temp_dir().join(format!(
        "rpshop-tree-upgrade-{}-{}.rp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, &bytes).unwrap();
    let document = load_document(&path).unwrap();
    Some((path, bytes, document))
}

fn old_tree_session(document: &OpenedDocument, path: PathBuf) -> Session {
    let mut previous = document.project.clone();
    previous.page_tree.clear();
    for page in &mut previous.pages {
        page.name = format!("Legacy folder / {}", page.name);
    }
    let mut state = document.native.as_ref().unwrap().session_state();
    state["project"] = serde_json::to_value(&previous).unwrap();
    state["project"]
        .as_object_mut()
        .unwrap()
        .remove("page_tree");
    Session {
        project: previous.clone(),
        saved: previous,
        path: Some(path),
        import_warnings: vec![],
        native_state: Some(state),
    }
}

fn recover_test_session(session: &Session) -> App {
    let mut storage = MemoryStorage::default();
    eframe::set_value(&mut storage, "rpshop-session", session);
    let mut creation = eframe::CreationContext::_new_kittest(egui::Context::default());
    creation.storage = Some(&storage);
    App::new(&creation, None)
}

fn remove_legacy_view_metadata(session: &mut Session) {
    let state = session.native_state.as_mut().unwrap();
    for page in state["project"]["pages"].as_array_mut().unwrap() {
        let object = page.as_object_mut().unwrap();
        object.remove("canvas");
        object["width"] = serde_json::json!(object["width"].as_f64().unwrap().max(1280.0));
        object["height"] = serde_json::json!(object["height"].as_f64().unwrap().max(800.0));
        for widget in object["widgets"].as_array_mut().unwrap() {
            widget.as_object_mut().unwrap().remove("text_layout");
        }
    }
    session.saved = serde_json::from_value(state["project"].clone()).unwrap();
    session.project = session.saved.clone();
}

#[test]
fn native_view_metadata_upgrade_preserves_unsaved_edits_and_old_tree_upgrade() {
    let Some((path, original, document)) = native_tree_upgrade_fixture() else {
        return;
    };
    for missing_tree in [false, true] {
        let mut session = if missing_tree {
            old_tree_session(&document, path.clone())
        } else {
            Session {
                project: document.project.clone(),
                saved: document.project.clone(),
                path: Some(path.clone()),
                import_warnings: vec![],
                native_state: Some(document.native.as_ref().unwrap().session_state()),
            }
        };
        remove_legacy_view_metadata(&mut session);
        session.project.pages[0].name = "unsaved view upgrade page name".into();
        let widget = &mut session.project.pages[0].widgets[0];
        let id = widget.id;
        widget.text = "unsaved view upgrade text".into();
        widget.x += 7.0;
        let x = widget.x;
        let restored = recover_test_session(&session);
        assert!(restored.native.is_some(), "{}", restored.status);
        assert_eq!(restored.saved, document.project);
        assert_eq!(restored.project.page_tree, document.project.page_tree);
        assert_eq!(
            restored.project.pages[0].canvas,
            document.project.pages[0].canvas
        );
        assert_eq!(
            restored.project.pages[0].width,
            document.project.pages[0].width
        );
        assert_eq!(
            restored.project.pages[0].height,
            document.project.pages[0].height
        );
        assert_eq!(
            restored.project.pages[0].name,
            "unsaved view upgrade page name"
        );
        let widget = restored.project.pages[0]
            .widgets
            .iter()
            .find(|w| w.id == id)
            .unwrap();
        assert_eq!(widget.text, "unsaved view upgrade text");
        assert_eq!(widget.x, x);
        assert_eq!(
            widget.text_layout,
            document.project.pages[0].widgets[0].text_layout
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn native_view_upgrade_rejects_unrelated_baseline_changes_without_losing_draft() {
    let Some((path, original, document)) = native_tree_upgrade_fixture() else {
        return;
    };
    let mut session = old_tree_session(&document, path.clone());
    remove_legacy_view_metadata(&mut session);
    session.saved.pages[0].widgets[0].x += 30.0;
    session.native_state.as_mut().unwrap()["project"]["pages"][0]["widgets"][0]["x"] =
        serde_json::json!(session.saved.pages[0].widgets[0].x);
    session.project = session.saved.clone();
    session.project.pages[0].widgets[0].text = "retain incompatible draft".into();
    let restored = recover_test_session(&session);
    assert!(restored.native.is_none());
    assert_eq!(restored.project, session.project);
    assert_eq!(restored.saved, session.saved);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn upgrading_legacy_native_session_keeps_unsaved_edits_and_restores_tree() {
    let Some((path, original, document)) = native_tree_upgrade_fixture() else {
        return;
    };
    assert!(!document.project.page_tree.is_empty());
    let mut session = old_tree_session(&document, path.clone());
    let widget = session.project.pages[0].widgets.first_mut().unwrap();
    let id = widget.id;
    widget.text = "unsaved native draft during tree upgrade".into();
    session.project.pages[0].name = "unsaved page rename".into();
    let restored = recover_test_session(&session);
    assert!(restored.native.is_some(), "{}", restored.status);
    assert_eq!(restored.project.page_tree, document.project.page_tree);
    assert_eq!(restored.saved, document.project);
    assert_eq!(restored.project.pages[0].name, "unsaved page rename");
    assert_eq!(
        restored.project.pages[0]
            .widgets
            .iter()
            .find(|w| w.id == id)
            .unwrap()
            .text,
        "unsaved native draft during tree upgrade"
    );
    for (page, fresh) in restored
        .project
        .pages
        .iter()
        .zip(&document.project.pages)
        .skip(1)
    {
        assert_eq!(page.name, fresh.name);
    }
    assert_ne!(restored.project, restored.saved);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn clean_legacy_session_reopens_source_but_unmatched_dirty_draft_is_preserved() {
    let Some((path, original, document)) = native_tree_upgrade_fixture() else {
        return;
    };
    let mut session = old_tree_session(&document, path.clone());
    session.native_state.as_mut().unwrap()["fingerprint"] =
        serde_json::json!("old fingerprint cannot rebind");
    let restored = recover_test_session(&session);
    assert!(restored.native.is_some());
    assert_eq!(restored.project, document.project);
    assert_eq!(restored.saved, document.project);
    session.project.pages[0].widgets[0].text = "must retain unmatched dirty draft".into();
    let expected = session.project.clone();
    let restored = recover_test_session(&session);
    assert!(restored.native.is_none());
    assert_eq!(restored.project, expected);
    assert_eq!(restored.saved, session.saved);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn customer_master_preview_groups_save_rebind_and_ungroup_without_touching_source() {
    use rpshop::rp_format::{binary, container};
    use std::collections::BTreeMap;

    let Some(source) = std::env::var_os("RPSHOP_NATIVE_FIXTURE").map(PathBuf::from) else {
        return;
    };
    let source_bytes = std::fs::read(&source).unwrap();
    let document = load_document(&source).unwrap();
    let native = document.native.expect("fixture must be a native document");
    let source_container = container::from_bytes(&source_bytes).unwrap();
    let state = native.session_state();

    fn package_offset(node: &serde_json::Value, package: &str) -> Option<u64> {
        let children = node.get("children")?.as_object()?;
        if let Some(offset) = children
            .get(package)
            .and_then(|child| child.pointer("/parts/package"))
            .and_then(serde_json::Value::as_u64)
        {
            return Some(offset);
        }
        children
            .values()
            .find_map(|child| package_offset(child, package))
    }

    // Prove both groups belong to the same native root diagram. Equal editor
    // paths alone cannot distinguish a root layer from a dynamic-panel state.
    let mut root_objects: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut root_groups = BTreeMap::new();
    for (id, binding) in state["group_bindings"].as_object().unwrap() {
        let Some((package, object)) = binding["object"]
            .as_str()
            .and_then(|object| object.rsplit_once('/'))
        else {
            continue;
        };
        if !root_objects.contains_key(package) {
            let Some(offset) = package_offset(&source_container.index["root"], package) else {
                continue;
            };
            let graph = binary::parse(&source_container.part(offset).unwrap()).unwrap();
            let roots = graph
                .root()
                .and_then(|root| root.properties.get("diagram"))
                .and_then(binary::reference_id)
                .and_then(|diagram| graph.node(diagram))
                .and_then(|diagram| diagram.properties["diagram-objects"].as_array())
                .map(|objects| {
                    objects
                        .iter()
                        .filter_map(binary::reference_id)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            root_objects.insert(package.to_owned(), roots);
        }
        if root_objects[package].contains(object) {
            root_groups.insert(id.parse::<u64>().unwrap(), package.to_owned());
        }
    }

    let eligible = |widget: &Widget| {
        native.selection_owner(widget.id) != widget.id
            || (!widget.locked && native.is_editable_widget(widget.id))
    };
    let mut fallback = None;
    let mut preferred = None;
    for (page_index, page) in document.project.pages.iter().enumerate() {
        for preview in &page.widgets {
            let owner = native.selection_owner(preview.id);
            if owner == preview.id {
                continue;
            }
            let Some(instance) = page
                .widgets
                .iter()
                .find(|widget| widget.id == owner && eligible(widget))
            else {
                continue;
            };
            let Some(group) = instance.groups.first().copied() else {
                continue;
            };
            let Some(package) = root_groups.get(&group) else {
                continue;
            };
            let members = page
                .widgets
                .iter()
                .filter(|widget| widget.groups.first() == Some(&group))
                .collect::<Vec<_>>();
            if !members.iter().all(|widget| eligible(widget)) {
                continue;
            }
            let selected = members
                .iter()
                .map(|widget| widget.id)
                .collect::<BTreeSet<_>>();
            if fallback.is_none() {
                fallback = Some((page_index, owner, selected.clone(), true));
            }
            let sibling = root_groups.iter().find_map(|(id, sibling_package)| {
                if *id == group || sibling_package != package {
                    return None;
                }
                let members = page
                    .widgets
                    .iter()
                    .filter(|widget| widget.groups.first() == Some(id))
                    .collect::<Vec<_>>();
                (!members.is_empty() && members.iter().all(|widget| eligible(widget))).then(|| {
                    members
                        .iter()
                        .map(|widget| widget.id)
                        .collect::<BTreeSet<_>>()
                })
            });
            if let Some(sibling) = sibling {
                preferred = Some((
                    page_index,
                    owner,
                    selected.union(&sibling).copied().collect(),
                    false,
                ));
                break;
            }
        }
        if preferred.is_some() {
            break;
        }
    }
    let (page_index, owner, selected, regroup_existing) = preferred
        .or(fallback)
        .expect("fixture needs an editable native root group containing a master instance preview");
    eprintln!(
        "native preview group regression: {} widgets on page {}, regroup existing: {regroup_existing}",
        selected.len(),
        page_index + 1
    );
    let path = std::env::temp_dir().join(format!(
        "rpshop-ui-preview-group-{}-{}.rp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert_ne!(path, source);
    struct TemporaryFile(PathBuf);
    impl Drop for TemporaryFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _temporary = TemporaryFile(path.clone());
    let mut app = fixture(document.project);
    app.native = Some(native);
    app.page = page_index;
    app.path = Some(path.clone());
    app.selected = selected;
    let marker = "RPShop master preview group regression";
    app.project.pages[page_index]
        .widgets
        .iter_mut()
        .find(|widget| widget.id == owner)
        .unwrap()
        .name = marker.into();
    if regroup_existing {
        app.group_selection(true);
        assert_eq!(app.status, "已解组");
    }
    app.group_selection(false);
    assert_eq!(app.status, "已组合");
    app.project.validate().unwrap();
    let grouped = app.project.pages[page_index]
        .widgets
        .iter()
        .find(|widget| widget.id == owner)
        .unwrap()
        .groups[0];
    assert!(
        app.project.pages[page_index]
            .widgets
            .iter()
            .filter(|widget| app.selected.contains(&widget.id))
            .all(|widget| widget.groups.first() == Some(&grouped))
    );
    let assert_preview_locks = |app: &App| {
        let native = app.native.as_ref().unwrap();
        let previews = app
            .project
            .pages
            .iter()
            .flat_map(|page| &page.widgets)
            .filter(|widget| native.selection_owner(widget.id) != widget.id)
            .collect::<Vec<_>>();
        assert!(!previews.is_empty());
        assert!(
            previews.iter().all(|widget| widget.locked),
            "synthetic preview locks must survive grouping and saving"
        );
    };
    assert_preview_locks(&app);
    assert!(app.save_project(false), "{}", app.status);
    assert_eq!(app.project, app.native.as_ref().unwrap().project);
    assert_eq!(app.project, app.saved);
    assert_preview_locks(&app);
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);

    // Native saving reopens the result and can allocate different widget IDs.
    // A second Ctrl+S must use those refreshed bindings and preview ownership.
    let before_move = app.project.clone();
    let instance = app
        .project
        .pages
        .iter_mut()
        .flat_map(|page| &mut page.widgets)
        .find(|widget| widget.name == marker)
        .unwrap();
    instance.x += 7.0;
    let expected_x = instance.x;
    app.remember(before_move);
    assert!(app.save_project(false), "second Ctrl+S: {}", app.status);
    assert_eq!(app.project, app.native.as_ref().unwrap().project);
    assert_preview_locks(&app);
    let instance = app
        .project
        .pages
        .iter()
        .flat_map(|page| &page.widgets)
        .find(|widget| widget.name == marker)
        .unwrap();
    assert!((instance.x - expected_x).abs() < 0.01);
    let grouped_depth = instance.groups.len();
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);

    // Exercise a true disk reopen, rather than reusing the first native graph.
    let reopened = load_document(&path).unwrap();
    let mut reopened_app = fixture(reopened.project);
    reopened_app.native = reopened.native;
    reopened_app.path = Some(path.clone());
    let (page_index, id) = reopened_app
        .project
        .pages
        .iter()
        .enumerate()
        .find_map(|(index, page)| {
            page.widgets
                .iter()
                .find(|widget| widget.name == marker)
                .map(|widget| (index, widget.id))
        })
        .unwrap();
    reopened_app.page = page_index;
    reopened_app.selected = rpshop::grouping::expanded_selection(
        &reopened_app.project.pages[page_index],
        &BTreeSet::from([id]),
        false,
    );
    let previous_paths = reopened_app.project.pages[page_index]
        .widgets
        .iter()
        .filter(|widget| reopened_app.selected.contains(&widget.id))
        .map(|widget| (widget.id, widget.groups.clone()))
        .collect::<BTreeMap<_, _>>();
    reopened_app.group_selection(true);
    assert_eq!(reopened_app.status, "已解组");
    reopened_app.project.validate().unwrap();
    for widget in reopened_app.project.pages[page_index]
        .widgets
        .iter()
        .filter(|widget| previous_paths.contains_key(&widget.id))
    {
        assert_eq!(widget.groups, previous_paths[&widget.id][1..]);
    }
    assert_preview_locks(&reopened_app);
    assert!(
        reopened_app.save_project(false),
        "save after reopened ungroup: {}",
        reopened_app.status
    );
    assert_eq!(
        reopened_app.project,
        reopened_app.native.as_ref().unwrap().project
    );
    let instance = reopened_app
        .project
        .pages
        .iter()
        .flat_map(|page| &page.widgets)
        .find(|widget| widget.name == marker)
        .unwrap();
    assert_eq!(instance.groups.len() + 1, grouped_depth);
    assert_preview_locks(&reopened_app);
    assert_eq!(
        std::fs::read(&source).unwrap(),
        source_bytes,
        "the user-selected source must remain byte-exact"
    );
}
