use super::*;

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
