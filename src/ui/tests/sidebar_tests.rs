use super::*;
use rpshop::model::PageTreeNode;

fn populated_sidebar() -> App {
    let mut project = Project::blank();
    project.pages[0].name = "sidebar-page-0".into();
    for index in 1..35 {
        let mut page = project.pages[0].clone();
        page.id = project.allocate_id();
        page.name = format!("sidebar-page-{index}");
        project.pages.push(page);
    }
    project.page_tree = vec![PageTreeNode::Folder {
        id: "sidebar-root".into(),
        name: "sidebar-root".into(),
        children: vec![
            PageTreeNode::Page {
                page_id: project.pages[0].id,
                children: vec![],
            },
            PageTreeNode::Folder {
                id: "sidebar-folder".into(),
                name: "sidebar-folder".into(),
                children: project.pages[1..]
                    .iter()
                    .map(|p| PageTreeNode::Page {
                        page_id: p.id,
                        children: vec![],
                    })
                    .collect(),
            },
        ],
    }];
    for index in 0..35 {
        add(&mut project, WidgetKind::Text, 80.0, 80.0);
        project.pages[0].widgets[index].name = format!("sidebar-layer-{index}");
    }
    fixture(project)
}

fn rects(app: &App, ctx: &egui::Context) -> sidebar::PaneRects {
    let panel = egui::containers::panel::PanelState::load(ctx, egui::Id::new("navigator")).unwrap();
    app.sidebar_layout.rects(panel.rect.shrink(14.0))
}

fn drag_divider(app: &mut App, ctx: &egui::Context, index: usize, delta: f32) {
    let start = rects(app, ctx).dividers[index].center();
    let end = start + vec2(0.0, delta);
    frame(
        app,
        ctx,
        vec![egui::Event::PointerMoved(start), pointer(start, true)],
    );
    frame(app, ctx, vec![egui::Event::PointerMoved(end)]);
    frame(app, ctx, vec![pointer(end, false)]);
    frame(app, ctx, vec![]);
}

#[test]
fn expanding_and_scrolling_pages_never_moves_other_sidebar_sections() {
    let mut app = populated_sidebar();
    let before_project = app.project.clone();
    let ctx = egui::Context::default();
    let initial = frame(&mut app, &ctx, vec![]);
    let library_y = text_center(&initial, "组件库").y;
    let layers_y = text_center(&initial, "图层").y;
    let folder = text_center(&initial, "sidebar-folder");
    click(&mut app, &ctx, folder);
    let expanded = frame(&mut app, &ctx, vec![]);
    assert_eq!(library_y, text_center(&expanded, "组件库").y);
    assert_eq!(layers_y, text_center(&expanded, "图层").y);
    let child_y = text_center(&expanded, "sidebar-page-1").y;
    let pointer = rects(&app, &ctx).panes[0].center() + vec2(0.0, 40.0);
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -190.0),
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let mut scrolled = frame(&mut app, &ctx, vec![]);
    for _ in 0..15 {
        scrolled = frame(&mut app, &ctx, vec![]);
    }
    // Rows leave the clipped page region while the other panes stay anchored.
    assert!(text_center(&scrolled, "sidebar-page-1").y < child_y - 50.0);
    assert_eq!(library_y, text_center(&scrolled, "组件库").y);
    assert_eq!(layers_y, text_center(&scrolled, "图层").y);
    assert_eq!(before_project, app.project);
    assert!(app.undo.is_empty());
}

#[test]
fn clicking_visible_pages_keeps_the_scrolled_list_in_place() {
    let mut app = populated_sidebar();
    let ctx = egui::Context::default();
    let initial = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, text_center(&initial, "sidebar-folder"));
    for _ in 0..60 {
        frame(&mut app, &ctx, vec![]);
    }
    let pane = rects(&app, &ctx).panes[0];
    let pointer = pane.center() + vec2(0.0, 40.0);
    let mut output = frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -190.0),
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    for _ in 0..60 {
        output = frame(&mut app, &ctx, vec![]);
    }
    let pages: Vec<_> = (1..app.project.pages.len())
        .filter_map(|index| {
            let name = format!("sidebar-page-{index}");
            let position = text_center(&output, &name);
            let fully_visible = output.shapes.iter().any(|shape| {
                matches!(
                    &shape.shape,
                    egui::epaint::Shape::Text(text) if text.galley.text() == name
                ) && shape
                    .clip_rect
                    .contains_rect(Rect::from_center_size(position, vec2(1.0, 27.0)))
            });
            (fully_visible && (position.y - pane.center().y).abs() > 25.0)
                .then_some((index, name, position))
        })
        .take(2)
        .collect();
    assert_eq!(
        pages.len(),
        2,
        "two off-center page rows must be fully visible"
    );
    for (index, name, position) in pages {
        // Move before pressing so a single input frame cannot look like a scroll drag.
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(position)]);
        click(&mut app, &ctx, position);
        assert_eq!(app.page, index, "clicking {name} must activate it");
        // Cover both the selection update and the full scroll animation duration.
        for _ in 0..60 {
            output = frame(&mut app, &ctx, vec![]);
            assert!(
                (text_center(&output, &name).y - position.y).abs() < 0.1,
                "clicking {name} must preserve its list position: expected {}, actual {}",
                position.y,
                text_center(&output, &name).y
            );
        }
    }
}

#[test]
fn external_page_navigation_expands_ancestors_and_reveals_the_page() {
    let mut app = populated_sidebar();
    let ctx = egui::Context::default();
    let initial = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, text_center(&initial, "sidebar-root"));
    let collapsed = frame(&mut app, &ctx, vec![]);
    assert!(!collapsed.shapes.iter().any(|shape| {
        matches!(
            &shape.shape,
            egui::epaint::Shape::Text(text) if text.galley.text() == "sidebar-folder"
        )
    }));
    app.page = 34;
    let mut output = frame(&mut app, &ctx, vec![]);
    for _ in 0..60 {
        output = frame(&mut app, &ctx, vec![]);
    }
    let position = text_center(&output, "sidebar-page-34");
    assert!(rects(&app, &ctx).panes[0].contains(position));
    assert!(
        output.shapes.iter().any(|shape| {
            if let egui::epaint::Shape::Text(text) = &shape.shape {
                text.galley.text() == "sidebar-page-34"
                    && shape.clip_rect.height() >= 26.5
                    && shape
                        .clip_rect
                        .contains_rect(Rect::from_min_size(text.pos, text.galley.size()))
            } else {
                false
            }
        }),
        "external navigation must reveal the complete selected row"
    );
}

#[test]
fn splitters_resize_adjacent_panes_and_remember_view_preferences_only() {
    let mut app = populated_sidebar();
    let project = app.project.clone();
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, vec![]);
    let initial = rects(&app, &ctx);
    drag_divider(&mut app, &ctx, 0, 65.0);
    let first = rects(&app, &ctx);
    assert!((first.panes[0].height() - initial.panes[0].height() - 65.0).abs() < 1.0);
    assert!((first.panes[1].height() - initial.panes[1].height() + 65.0).abs() < 1.0);
    assert!((first.panes[2].top() - initial.panes[2].top()).abs() < 1.0);
    drag_divider(&mut app, &ctx, 1, -45.0);
    let second = rects(&app, &ctx);
    assert!((second.panes[0].height() - first.panes[0].height()).abs() < 1.0);
    assert!((second.panes[2].height() - first.panes[2].height() - 45.0).abs() < 1.0);
    drag_divider(&mut app, &ctx, 0, 900.0);
    let constrained = rects(&app, &ctx);
    assert!(constrained.panes[1].height() >= 119.0);
    assert!((constrained.panes[2].top() - second.panes[2].top()).abs() < 1.0);
    drag_divider(&mut app, &ctx, 0, -20.0);
    assert!(
        (rects(&app, &ctx).panes[0].height() - constrained.panes[0].height() + 20.0).abs() < 1.0
    );
    assert_eq!(app.project, project);
    assert_eq!(app.project, app.saved);
    assert!(app.undo.is_empty());
    assert!(app.redo.is_empty());

    let mut storage = MemoryStorage::default();
    app.screenshot_mode = false;
    app.save(&mut storage);
    let restored: sidebar::SidebarLayout =
        eframe::get_value(&storage, sidebar::STORAGE_KEY).unwrap();
    assert_eq!(restored, app.sidebar_layout);
    let session: Session = eframe::get_value(&storage, "rpshop-session").unwrap();
    assert_eq!(session.project, project);
    assert_eq!(session.saved, project);
}

#[test]
fn resizing_window_preserves_sidebar_proportions_and_reachable_panes() {
    let mut app = populated_sidebar();
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, vec![]);
    drag_divider(&mut app, &ctx, 0, 50.0);
    let layout = app.sidebar_layout;
    let large = rects(&app, &ctx);
    let mut native_frame = eframe::Frame::_new_kittest();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 720.0))),
            time: Some(f64::from(app.frames) / 60.0),
            ..Default::default()
        },
        |ctx| app.update(ctx, &mut native_frame),
    );
    let small = rects(&app, &ctx);
    for (pane, minimum) in small.panes.iter().zip([130.0, 120.0, 110.0]) {
        assert!(pane.height() >= minimum - 0.1);
    }
    assert!(small.panes[0].contains(text_center(&output, "页面")));
    assert!(small.panes[1].contains(text_center(&output, "组件库")));
    assert!(small.panes[2].contains(text_center(&output, "图层")));
    assert_eq!(layout, app.sidebar_layout);
    frame(&mut app, &ctx, vec![]);
    let restored = rects(&app, &ctx);
    assert!((restored.panes[0].height() - large.panes[0].height()).abs() < 1.0);
    assert!((restored.panes[1].height() - large.panes[1].height()).abs() < 1.0);
}

#[test]
fn narrow_library_scrolls_independently_and_last_component_is_clickable() {
    let mut app = populated_sidebar();
    app.library_category = Some(Category::Basic);
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, vec![]);
    let mut panel =
        egui::containers::panel::PanelState::load(&ctx, egui::Id::new("navigator")).unwrap();
    panel.rect.set_width(195.0);
    ctx.data_mut(|data| data.insert_persisted(egui::Id::new("navigator"), panel));
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    let panes = rects(&app, &ctx);
    let rectangle = text_center(&output, "矩形");
    let ellipse = text_center(&output, "椭圆");
    let text = text_center(&output, "文本");
    assert!(panes.panes[1].contains(rectangle));
    assert!(panes.panes[1].contains(ellipse));
    assert!(
        text.y > rectangle.y + 30.0,
        "the third button wraps to keep the narrow library accessible"
    );
    let page_y = text_center(&output, "sidebar-page-0").y;
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(panes.panes[1].center() + vec2(0.0, 40.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -2400.0),
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let mut output = frame(&mut app, &ctx, vec![]);
    for _ in 0..20 {
        output = frame(&mut app, &ctx, vec![]);
    }
    assert_eq!(page_y, text_center(&output, "sidebar-page-0").y);
    let container = text_center(&output, "容器");
    assert!(panes.panes[1].contains(container));
    let count = app.project.pages[0].widgets.len();
    click(&mut app, &ctx, container);
    assert_eq!(app.project.pages[0].widgets.len(), count + 1);
    assert_eq!(
        app.project.pages[0].widgets.last().unwrap().kind,
        WidgetKind::Panel
    );
}

#[test]
fn library_search_adds_real_controls_and_presets_preserve_geometry_on_undo() {
    let mut app = fixture(Project::blank());
    let ctx = egui::Context::default();
    app.library_query = "多行".into();
    let output = frame(&mut app, &ctx, vec![]);
    let text_area = text_center(&output, "文本域");
    click(&mut app, &ctx, text_area);
    assert_eq!(app.project.pages[0].widgets[0].kind, WidgetKind::TextArea);
    app.library_query = "flow decision".into();
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    let decision = text_center(&output, "判断");
    click(&mut app, &ctx, decision);
    let shape = app.project.pages[0].widgets.last().unwrap();
    assert!(shape.vector_points.is_some());
    let geometry = shape.vector_points.clone();
    app.undo();
    assert_eq!(app.project.pages[0].widgets.len(), 1);
    app.redo();
    assert_eq!(app.project.pages[0].widgets[1].vector_points, geometry);
    app.library_query = "no-such-widget".into();
    let output = frame(&mut app, &ctx, vec![]);
    assert!(rects(&app, &ctx).panes[1].contains(text_center(&output, "没有匹配的元件")));
}

#[test]
fn library_drag_keeps_the_selected_preset_and_composites_insert_as_one_group() {
    let mut app = fixture(Project::blank());
    let ctx = egui::Context::default();
    app.library_query = "star".into();
    let output = frame(&mut app, &ctx, vec![]);
    let star = text_center(&output, "星形");
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(star), pointer(star, true)],
    );
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(star + vec2(15.0, 0.0))],
    );
    assert_eq!(app.pending_widget.unwrap().id, "star");
    let canvas = Pos2::new(650.0, 400.0);
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(canvas), pointer(canvas, false)],
    );
    assert!(app.pending_widget.is_none());
    assert!(app.project.pages[0].widgets[0].vector_points.is_some());
    app.library_query = "table grid".into();
    let output = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, text_center(&output, "表格"));
    assert_eq!(app.selected.len(), 12);
    let group = app.project.pages[0].widgets[1].groups[0];
    assert!(
        app.project.pages[0].widgets[1..]
            .iter()
            .all(|w| w.groups == [group])
    );
    app.undo();
    assert_eq!(app.project.pages[0].widgets.len(), 1);
}
