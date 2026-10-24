use super::*;
use serde_json::json;

fn id(n: u64) -> String {
    format!("{n:032x}")
}
fn node(kind: &str, properties: Value) -> Node {
    Node {
        kind: kind.into(),
        properties,
    }
}

type Fixture = (
    Project,
    BTreeMap<u64, Graph>,
    BTreeMap<u64, native::Binding>,
    BTreeMap<u64, String>,
);

fn fixture() -> Fixture {
    let package = id(100);
    let root = id(101);
    let actor = id(102);
    let target = id(103);
    let mut project = Project::blank();
    project.pages[0].widgets = vec![
        Widget::new(2, WidgetKind::Button, 10.0, 10.0),
        Widget::new(3, WidgetKind::Text, 10.0, 80.0),
    ];
    let mut other = project.pages[0].clone();
    other.id = 4;
    other.name = "Other".into();
    other.widgets.clear();
    project.pages.push(other);
    project.next_id = 5;
    let mut objects = BTreeMap::from([
        (root.clone(), node("Axure:Page", json!({}))),
        (
            actor.clone(),
            node(
                "Axure:DiagramObject:VectorShape",
                json!({"InteractionMap":{"$ref":id(104)}}),
            ),
        ),
        (
            target.clone(),
            node("Axure:DiagramObject:VectorShape", json!({})),
        ),
        (
            id(104),
            node(
                "Axure:InteractionTreeMap",
                json!({"tree":{"$ref":id(105)},"node-table":{id(106):{"$ref":id(107)},id(108):{"$ref":id(109)},id(110):{"$ref":id(111)}}}),
            ),
        ),
        (
            id(105),
            node("Axure:Tree", json!({"children":[{"$ref":id(106)}]})),
        ),
        (
            id(106),
            node("Axure:Node", json!({"children":[{"$ref":id(108)}]})),
        ),
        (
            id(107),
            node("Axure:InteractionEvent", json!({"event-type":16})),
        ),
        (
            id(108),
            node("Axure:Node", json!({"children":[{"$ref":id(110)}]})),
        ),
        (
            id(109),
            node(
                "Axure:InteractionCase",
                json!({"Description":"original guarded case","Condition":{"$ref":id(112)}}),
            ),
        ),
        (id(110), node("Axure:Node", json!({"children":[]}))),
        (
            id(111),
            node("Axure:InteractionWaitAction", json!({"duration":123.0})),
        ),
        (
            id(112),
            node("Axure:Expr:StringLiteral", json!({"value":"unknown guard"})),
        ),
    ]);
    // Reference targets must be original native GUIDs, never model numeric IDs.
    objects.get_mut(&actor).unwrap().properties["unrelated-source-field"] = json!("preserve me");
    let mut graph = Graph::new([11, 0, 0, 0], root, package.clone(), objects);
    graph
        .set_dictionary_key_token(&id(104), "/node-table", 3)
        .unwrap();
    (
        project,
        BTreeMap::from([(10, graph)]),
        BTreeMap::from([
            (
                2,
                native::Binding {
                    object: format!("{package}/{actor}"),
                    scale: 1.0,
                },
            ),
            (
                3,
                native::Binding {
                    object: format!("{package}/{target}"),
                    scale: 1.0,
                },
            ),
        ]),
        BTreeMap::from([(1, package), (4, id(200))]),
    )
}

fn roundtrip_read(
    parts: &BTreeMap<u64, Graph>,
    project: &Project,
    bindings: &BTreeMap<u64, native::Binding>,
    pages: &BTreeMap<u64, String>,
) -> Vec<AuthoredEvent> {
    let raw = &parts[&10];
    let encoded = binary::encode(raw).unwrap();
    let parsed = binary::parse(&encoded).unwrap();
    let graph = scope_package(&parsed);
    let roots = BTreeMap::from([(parsed.package.clone(), graph.root.clone())]);
    let pages = pages.iter().map(|(id, key)| (key.clone(), *id)).collect();
    read(&graph, &roots, project, bindings, &pages)
}

#[test]
fn native_authored_cases_append_roundtrip_update_and_delete_without_replacing_source_cases() {
    let (baseline, mut parts, bindings, pages) = fixture();
    let original = parts[&10].clone();
    let mut project = baseline.clone();
    let mut event = AuthoredEvent::new(1, Some(2), EventTrigger::Click);
    event.name = "文字→显隐→跳转 <case>".into();
    event.actions = vec![
        Interaction {
            action: Action::SetText,
            target_widget: Some(3),
            value: "<script>not executable</script>\n中文".into(),
            ..Default::default()
        },
        Interaction {
            action: Action::ToggleVisibility,
            target_widget: Some(3),
            ..Default::default()
        },
        Interaction {
            action: Action::Navigate,
            target_page: Some(4),
            ..Default::default()
        },
    ];
    let blank = AuthoredEvent::new(1, None, EventTrigger::PageLoad);
    project.interaction_events = vec![event.clone(), blank.clone()];
    project.validate().unwrap();
    patch(&mut parts, &baseline, &project, &bindings, &pages).unwrap();
    for n in [109, 110, 111, 112] {
        assert_eq!(
            parts[&10].objects[&id(n)].properties,
            original.objects[&id(n)].properties
        );
    }
    for n in [106, 108, 110] {
        assert_eq!(
            parts[&10].objects[&id(104)].properties["node-table"][id(n)],
            original.objects[&id(104)].properties["node-table"][id(n)]
        );
    }
    assert_eq!(
        parts[&10].objects[&id(102)].properties["unrelated-source-field"],
        json!("preserve me")
    );
    let read = roundtrip_read(&parts, &project, &bindings, &pages);
    assert_eq!(read.len(), 2);
    assert_eq!(read.iter().find(|case| case.id == event.id), Some(&event));
    assert_eq!(read.iter().find(|case| case.id == blank.id), Some(&blank));
    let mut updated = project.clone();
    updated.interaction_events[0].name = "renamed".into();
    updated.interaction_events[0].actions.remove(2);
    patch(&mut parts, &project, &updated, &bindings, &pages).unwrap();
    let read = roundtrip_read(&parts, &updated, &bindings, &pages);
    assert_eq!(read.iter().filter(|case| case.id == event.id).count(), 1);
    assert_eq!(
        read.iter().find(|case| case.id == event.id),
        Some(&updated.interaction_events[0])
    );
    let mut removed = updated.clone();
    removed.interaction_events.clear();
    patch(&mut parts, &updated, &removed, &bindings, &pages).unwrap();
    assert!(roundtrip_read(&parts, &removed, &bindings, &pages).is_empty());
    assert_eq!(
        parts[&10].objects[&id(104)].properties["node-table"][id(108)],
        original.objects[&id(104)].properties["node-table"][id(108)]
    );
}

#[test]
fn blank_and_draft_navigation_cases_are_valid_and_widget_deletion_prunes_only_affected_actions() {
    let (mut project, _, _, _) = fixture();
    let mut event = AuthoredEvent::new(1, Some(2), EventTrigger::Click);
    event.actions = vec![
        Interaction {
            action: Action::Navigate,
            ..Default::default()
        },
        Interaction {
            action: Action::ToggleVisibility,
            target_widget: Some(3),
            ..Default::default()
        },
    ];
    project.interaction_events = vec![
        event.clone(),
        AuthoredEvent::new(1, None, EventTrigger::PageLoad),
    ];
    project.validate().unwrap();
    let json = serde_json::to_string(&project).unwrap();
    let restored: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, project);
    let new = project.allocate_id();
    project.pages[0]
        .widgets
        .push(Widget::new(new, WidgetKind::Button, 0.0, 0.0));
    project.duplicate_interaction_events(1, &BTreeMap::from([(2, new)]));
    assert_eq!(project.interaction_events.len(), 3);
    assert_eq!(project.interaction_events[2].owner, Some(new));
    assert_ne!(project.interaction_events[2].id, event.id);
    project.pages[0].widgets.retain(|widget| widget.id != 3);
    project.prune_interaction_references();
    project.validate().unwrap();
    assert_eq!(project.interaction_events[0].actions.len(), 1);
    project.pages[0].widgets.retain(|widget| widget.id != 2);
    project.prune_interaction_references();
    project.validate().unwrap();
    assert_eq!(project.interaction_events.len(), 2);
}

#[test]
fn authored_case_does_not_hide_an_existing_simple_navigation_and_form_text_roundtrips() {
    let (mut baseline, mut parts, bindings, pages) = fixture();
    baseline.pages[0].widgets[0].interaction = Interaction {
        action: Action::Navigate,
        target_page: Some(4),
        ..Default::default()
    };
    baseline.pages[0].widgets[1].kind = WidgetKind::Input;
    let graph = parts.get_mut(&10).unwrap();
    graph.objects.get_mut(&id(109)).unwrap().properties["Condition"] = json!({"$ref":EMPTY_ID});
    graph.objects.insert(
        id(111),
        node(
            "Axure:InteractionLinkWindowAction",
            json!({"link-type":1,"target":{"$ref":id(113)}}),
        ),
    );
    graph.objects.insert(id(113),node("Axure:InteractionTarget",json!({"target-type":1,"target-page-id":{"$ref":id(200),"$type":"Axure:Page","$token":19},"target-literal":{"$ref":id(114)}})));
    graph.objects.insert(
        id(114),
        node("Axure:Expr:StringLiteral", json!({"value":"Other.html"})),
    );
    let original = graph.objects[&id(111)].properties.clone();
    let mut edited = baseline.clone();
    let mut case = AuthoredEvent::new(1, Some(2), EventTrigger::Click);
    case.actions.push(Interaction {
        action: Action::SetText,
        target_widget: Some(3),
        value: "plain input value".into(),
        ..Default::default()
    });
    edited.interaction_events.push(case.clone());
    patch(&mut parts, &baseline, &edited, &bindings, &pages).unwrap();
    let raw = &parts[&10];
    assert!(
        raw.objects
            .values()
            .any(|node| node.kind == "Axure:Expr:FCall"
                && node.properties["function-name"] == "SetWidgetFormText")
    );
    let canonical = binary::parse(&binary::encode(raw).unwrap()).unwrap();
    let graph = scope_package(&canonical);
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
    assert_eq!(
        converter.navigation(&graph.objects[&format!("{}/{}", id(100), id(102))].properties),
        Some(id(200))
    );
    let read = roundtrip_read(&parts, &edited, &bindings, &pages);
    assert_eq!(read, vec![case]);
    assert_eq!(parts[&10].objects[&id(111)].properties, original);
}

#[test]
fn draft_navigation_target_stays_empty_after_native_encoding() {
    let (baseline, mut parts, bindings, pages) = fixture();
    let mut project = baseline.clone();
    let mut case = AuthoredEvent::new(1, None, EventTrigger::PageLoad);
    case.actions.push(Interaction {
        action: Action::Navigate,
        ..Default::default()
    });
    project.interaction_events.push(case.clone());
    patch(&mut parts, &baseline, &project, &bindings, &pages).unwrap();
    assert_eq!(
        roundtrip_read(&parts, &project, &bindings, &pages),
        vec![case]
    );
}

#[test]
fn caller_selected_native_authoring_previews_and_saves_to_a_copy_without_touching_source() {
    for variable in ["RPSHOP_NATIVE_FIXTURE", "RPSHOP_NATIVE_V11_FIXTURE"] {
        let Some(path) = std::env::var_os(variable) else {
            continue;
        };
        let source = std::path::PathBuf::from(path);
        let bytes = std::fs::read(&source).unwrap();
        let mut document = super::super::open(&source).unwrap();
        let mut project = document.project.clone();
        let page = project.pages[0].id;
        let target = project.allocate_id();
        let owner = project.allocate_id();
        project.pages[0]
            .widgets
            .push(Widget::new(target, WidgetKind::Text, 10.0, 10.0));
        project.pages[0]
            .widgets
            .push(Widget::new(owner, WidgetKind::Button, 10.0, 70.0));
        let mut click = AuthoredEvent::new(page, Some(owner), EventTrigger::Click);
        click.name = "saved authored click".into();
        click.actions = vec![
            Interaction {
                action: Action::SetText,
                target_widget: Some(target),
                value: "saved native <text>".into(),
                ..Default::default()
            },
            Interaction {
                action: Action::ToggleVisibility,
                target_widget: Some(target),
                ..Default::default()
            },
        ];
        let blank = AuthoredEvent::new(page, None, EventTrigger::PageLoad);
        project
            .interaction_events
            .extend([click.clone(), blank.clone()]);
        project.validate().unwrap();
        let preview = document.preview_scene(&project).unwrap();
        assert!(
            preview
                .program
                .events
                .iter()
                .flat_map(|event| &event.cases)
                .any(|case| case.name == click.name && case.supported && case.actions.len() == 2)
        );
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        let dir = std::path::PathBuf::from("target/qa").join(format!(
            "authored-{}-{}",
            std::process::id(),
            click.id
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join("roundtrip.rp");
        document.save(&project, &output).unwrap();
        let mut reopened = super::super::open(&output).unwrap();
        let authored = reopened
            .project
            .interaction_events
            .iter()
            .find(|event| event.id == click.id)
            .unwrap();
        assert_eq!(authored.actions.len(), 2);
        assert_eq!(authored.actions[0].value, "saved native <text>");
        assert_eq!(authored.page, reopened.project.pages[0].id);
        assert!(
            reopened
                .project
                .interaction_events
                .iter()
                .any(|event| event.id == blank.id && event.actions.is_empty())
        );
        let mut edited = reopened.project.clone();
        edited
            .interaction_events
            .iter_mut()
            .find(|event| event.id == click.id)
            .unwrap()
            .actions[0]
            .value = "edited after reopen".into();
        reopened.save(&edited, &output).unwrap();
        assert_eq!(
            reopened
                .project
                .interaction_events
                .iter()
                .find(|event| event.id == click.id)
                .unwrap()
                .actions[0]
                .value,
            "edited after reopen"
        );
        edited = reopened.project.clone();
        edited
            .interaction_events
            .retain(|event| event.id != click.id);
        reopened.save(&edited, &output).unwrap();
        assert!(
            !reopened
                .project
                .interaction_events
                .iter()
                .any(|event| event.id == click.id)
        );
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
    }
}
