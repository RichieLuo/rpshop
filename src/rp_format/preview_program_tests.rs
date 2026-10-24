use super::*;
use serde_json::json;

fn node(kind: &str, properties: Value) -> Node {
    Node {
        kind: kind.into(),
        properties,
    }
}
fn reference(id: &str) -> Value {
    json!({"$ref":id})
}

fn fixture() -> (Graph, Vec<SourceBinding>) {
    let mut objects = BTreeMap::new();
    for (id, kind, properties) in [
        (
            "pkg/actor",
            "Axure:DiagramObject:VectorShape",
            json!({"InteractionMap":reference("map")}),
        ),
        ("pkg/target", "Axure:DiagramObject:DynamicPanel", json!({})),
        (
            "map",
            "Axure:InteractionTreeMap",
            json!({"tree":reference("tree"),"node-table":{"event-tree":reference("event"),"case-tree":reference("case"),"z-first":reference("move"),"a-last":reference("fade")}}),
        ),
        (
            "tree",
            "Axure:Tree",
            json!({"children":[reference("event-tree")]}),
        ),
        (
            "event-tree",
            "Axure:Node",
            json!({"children":[reference("case-tree")]}),
        ),
        ("event", "Axure:InteractionEvent", json!({"event-type":16})),
        (
            "case-tree",
            "Axure:Node",
            json!({"children":[reference("z-first"),reference("a-last")]}),
        ),
        (
            "case",
            "Axure:InteractionCase",
            json!({"Condition":reference(EMPTY),"IsNewIfGroup":false,"Description":"显示详情"}),
        ),
        ("z-first", "Axure:Node", json!({"children":[]})),
        ("a-last", "Axure:Node", json!({"children":[]})),
        (
            "move",
            "Axure:InteractionMoveWidgetAction",
            json!({"WidgetPaths":[reference("path")],"MoveInfos":[reference("move-info")]}),
        ),
        (
            "fade",
            "Axure:InteractionFadeWidgetAction",
            json!({"WidgetPaths":[reference("path")],"FadeInfos":[reference("fade-info")]}),
        ),
        (
            "path",
            "Axure:DiagramObjectPath",
            json!({"object-id":reference("target"),"rdo-path":[],"repeater-path":[]}),
        ),
        (
            "self",
            "Axure:DiagramObjectPath",
            json!({"object-id":reference(EMPTY),"rdo-path":[],"repeater-path":[]}),
        ),
        (
            "move-info",
            "Axure:MoveWidgetInfo",
            json!({"MoveType":0,"XValue":reference("x"),"YValue":reference("y"),"Duration":500.0,"Easing":0}),
        ),
        (
            "fade-info",
            "Axure:FadeWidgetInfo",
            json!({"fade-action-type":1,"show-type":2,"page-background":-1691398321i64,"easing":0,"duration":500.0}),
        ),
        (
            "x",
            "Axure:Expr:StringLiteral",
            json!({"value":"-3.5","variable-identifiers":[]}),
        ),
        (
            "y",
            "Axure:Expr:StringLiteral",
            json!({"value":"120","variable-identifiers":[]}),
        ),
    ] {
        objects.insert(id.into(), node(kind, properties));
    }
    let graph = Graph::new([8, 0, 5, 0], "pkg/actor".into(), "pkg".into(), objects);
    let bindings = vec![
        SourceBinding {
            key: "actor-a".into(),
            source: "pkg/actor".into(),
            scope: vec!["page".into(), "instance-a".into()],
        },
        SourceBinding {
            key: "target-a".into(),
            source: "pkg/target".into(),
            scope: vec!["page".into(), "instance-a".into()],
        },
        SourceBinding {
            key: "target-b".into(),
            source: "pkg/target".into(),
            scope: vec!["page".into(), "instance-b".into()],
        },
    ];
    (graph, bindings)
}

#[test]
fn event_tree_order_and_instance_targets_are_preserved() {
    let (graph, bindings) = fixture();
    let result = compile(&graph, &bindings, &BTreeMap::new());
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.events.len(), 1);
    let case = &result.events[0].cases[0];
    assert!(case.supported);
    assert_eq!(case.name, "显示详情");
    let PreviewAction::Move { entries } = &case.actions[0] else {
        panic!("tree order must precede dictionary key order")
    };
    assert_eq!(entries[0].targets, vec!["target-a"]);
    assert_eq!((entries[0].x, entries[0].y), (-3.5, 120.0));
    let PreviewAction::Fade { entries } = &case.actions[1] else {
        panic!("fade must remain the second action")
    };
    assert_eq!(entries[0].background.as_deref(), Some("#2F4F4F9B"));
    assert_eq!(entries[0].show_type, 2);
    assert!(entries[0].lightbox_close);
}

#[test]
fn inline_frame_pairs_use_their_own_target_instead_of_default_target() {
    let (mut graph, bindings) = fixture();
    graph.objects.get_mut("move").unwrap().kind = "Axure:InteractionLinkFrameAction".into();
    graph.objects.get_mut("move").unwrap().properties = json!({"link-type":5,"target":reference("missing"),"framepathstotargets":[reference("path"),reference("page-target")]});
    graph.objects.insert("page-target".into(),node("Axure:InteractionTarget",json!({"target-type":1,"target-page-id":reference("page-b"),"target-literal":reference("url")})));
    graph.objects.insert(
        "url".into(),
        node("Axure:Expr:StringLiteral", json!({"value":"B.html"})),
    );
    let result = compile(
        &graph,
        &bindings,
        &BTreeMap::from([("page-b".into(), "stable-B".into())]),
    );
    let PreviewAction::LinkFrame { frames } = &result.events[0].cases[0].actions[0] else {
        panic!("expected frame command")
    };
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].targets, vec!["target-a"]);
    assert_eq!(frames[0].page.key.as_deref(), Some("stable-B"));
    assert_eq!(frames[0].page.literal, "B.html");
}

#[test]
fn function_blocks_expand_in_order_and_empty_path_means_this_instance() {
    let (mut graph, bindings) = fixture();
    graph.objects.get_mut("move").unwrap().kind = "Axure:InterationActionSetFunction".into();
    graph.objects.get_mut("move").unwrap().properties = json!({"action":reference("block")});
    for (id, kind, properties) in [
        (
            "block",
            "Axure:Expr:Block",
            json!({"subexpressions":[reference("call-true"),reference("call-false")]}),
        ),
        (
            "call-true",
            "Axure:Expr:FCall",
            json!({"function-name":"SetCheckState","args":[reference("self-literal"),reference("true")]}),
        ),
        (
            "call-false",
            "Axure:Expr:FCall",
            json!({"function-name":"SetCheckState","args":[reference("path-literal"),reference("false")]}),
        ),
        (
            "self-literal",
            "Axure:Expr:DiagramObjectPathLiteral",
            json!({"value":reference("self")}),
        ),
        (
            "path-literal",
            "Axure:Expr:DiagramObjectPathLiteral",
            json!({"value":reference("path")}),
        ),
        ("true", "Axure:Expr:StringLiteral", json!({"value":"true"})),
        (
            "false",
            "Axure:Expr:StringLiteral",
            json!({"value":"false"}),
        ),
    ] {
        graph.objects.insert(id.into(), node(kind, properties));
    }
    let result = compile(&graph, &bindings, &BTreeMap::new());
    assert!(result.events[0].cases[0].supported);
    let actions = &result.events[0].cases[0].actions;
    assert!(
        matches!(&actions[0],PreviewAction::SetChecked{targets,value:true} if targets==&vec!["actor-a"])
    );
    assert!(
        matches!(&actions[1],PreviewAction::SetChecked{targets,value:false} if targets==&vec!["target-a"])
    );
}

#[test]
fn unsupported_case_does_not_disable_another_case_or_guess_a_missing_page() {
    let (mut graph, bindings) = fixture();
    graph.objects.get_mut("case").unwrap().properties["Condition"] = reference("condition");
    graph.objects.get_mut("event-tree").unwrap().properties["children"] =
        json!([reference("case-tree"), reference("case-two-tree")]);
    graph.objects.get_mut("map").unwrap().properties["node-table"]["case-two-tree"] =
        reference("case-two");
    graph.objects.get_mut("map").unwrap().properties["node-table"]["link-tree"] = reference("link");
    for (id, kind, properties) in [
        (
            "condition",
            "Axure:Expr:StringLiteral",
            json!({"value":"true"}),
        ),
        (
            "case-two-tree",
            "Axure:Node",
            json!({"children":[reference("link-tree")]}),
        ),
        (
            "case-two",
            "Axure:InteractionCase",
            json!({"IsNewIfGroup":true}),
        ),
        ("link-tree", "Axure:Node", json!({"children":[]})),
        (
            "link",
            "Axure:InteractionLinkWindowAction",
            json!({"link-type":1,"target":reference("missing-page")}),
        ),
        (
            "missing-page",
            "Axure:InteractionTarget",
            json!({"target-type":1,"target-page-id":reference("not-in-document"),"target-literal":reference("missing-url")}),
        ),
        (
            "missing-url",
            "Axure:Expr:StringLiteral",
            json!({"value":"Absent.html"}),
        ),
    ] {
        graph.objects.insert(id.into(), node(kind, properties));
    }
    let result = compile(&graph, &bindings, &BTreeMap::new());
    assert!(!result.events[0].cases[0].supported);
    assert!(result.events[0].cases[1].supported);
    assert!(result.events[0].cases[1].new_if_group);
    let PreviewAction::LinkWindow { page, .. } = &result.events[0].cases[1].actions[0] else {
        panic!("missing target remains an explicit link result")
    };
    assert!(page.key.is_none());
    assert_eq!(page.native_id, "not-in-document");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Absent.html"))
    );
}

#[test]
fn dynamic_and_nonfinite_coordinates_and_unimplemented_effects_are_not_executed() {
    for expression in ["[[Widget.x]]", "NaN", "Infinity"] {
        let (mut graph, bindings) = fixture();
        graph.objects.get_mut("x").unwrap().properties["value"] = json!(expression);
        let result = compile(&graph, &bindings, &BTreeMap::new());
        assert!(!result.events[0].cases[0].supported);
        assert!(!result.diagnostics.is_empty());
    }
    let (mut graph, bindings) = fixture();
    graph.objects.get_mut("fade-info").unwrap().properties["show-type"] = json!(3);
    let result = compile(&graph, &bindings, &BTreeMap::new());
    assert!(!result.events[0].cases[0].supported);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("弹出模式"))
    );
}

#[test]
fn foreign_instance_targets_and_unknown_move_modes_disable_the_whole_case() {
    let (graph, mut bindings) = fixture();
    bindings.retain(|binding| binding.key != "target-a");
    let result = compile(&graph, &bindings, &BTreeMap::new());
    assert!(!result.events[0].cases[0].supported);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("目标元件不在当前预览实例"))
    );
    let (mut graph, bindings) = fixture();
    graph.objects.get_mut("move-info").unwrap().properties["MoveType"] = json!(3);
    let result = compile(&graph, &bindings, &BTreeMap::new());
    assert!(!result.events[0].cases[0].supported);
    assert!(
        result.events[0].cases[0]
            .actions
            .iter()
            .any(|action| matches!(action, PreviewAction::Fade { .. })),
        "another supported action remains inspectable without executing a partial case"
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("移动模式 3"))
    );
}

#[test]
fn missing_targets_are_diagnosed_without_disabling_valid_targets_or_later_actions() {
    let (mut graph, bindings) = fixture();
    graph.objects.insert(
        "missing-path".into(),
        node(
            "Axure:DiagramObjectPath",
            json!({"object-id":reference("deleted-object"),"rdo-path":[],"repeater-path":[]}),
        ),
    );
    graph.objects.get_mut("fade").unwrap().properties = json!({"WidgetPaths":[reference("missing-path"),reference("path")],"FadeInfos":[reference("fade-info"),reference("fade-info")]});
    graph.objects.get_mut("case-tree").unwrap().properties["children"] =
        json!([reference("a-last"), reference("z-first")]);
    let program = compile(&graph, &bindings, &BTreeMap::new());
    let case = &program.events[0].cases[0];
    assert!(case.supported);
    let PreviewAction::Fade { entries } = &case.actions[0] else {
        panic!("fade order")
    };
    assert!(entries[0].targets.is_empty());
    assert_eq!(entries[1].targets, vec!["target-a"]);
    assert!(matches!(&case.actions[1], PreviewAction::Move { .. }));
    assert_eq!(program.diagnostics.len(), 1);
    assert!(
        program.diagnostics[0]
            .message
            .contains("源元件目标不存在，已跳过")
    );
}

#[test]
fn caller_selected_native_click_program_is_read_only_and_keeps_missing_links_explicit() {
    let Some(path) = std::env::var_os("RPSHOP_NATIVE_FIXTURE") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let original = std::fs::read(&path).unwrap();
    let container = crate::rp_format::container::open(&path).unwrap();
    let root = &container.index["root"];
    let offset = root["parts"]["DesignDocument"].as_u64().unwrap();
    let document = crate::rp_format::binary::parse(&container.part(offset).unwrap()).unwrap();
    let names = super::super::package_names(&document);
    let mut graph = document.clone();
    let mut roots = BTreeMap::new();
    let mut bytes = 0;
    let mut parts = BTreeMap::new();
    super::super::load_packages(
        &container, root, &names, &mut graph, &mut roots, &mut bytes, &mut parts, 0,
    )
    .unwrap();
    let bindings = graph
        .objects
        .iter()
        .filter(|(_, node)| {
            node.kind.starts_with("Axure:DiagramObject:")
                || matches!(node.kind.as_str(), "Axure:Page" | "Axure:PanelDiagram")
        })
        .map(|(id, _)| SourceBinding {
            key: id.clone(),
            source: id.clone(),
            scope: vec![],
        })
        .collect::<Vec<_>>();
    let pages = roots
        .iter()
        .filter(|(_, root)| {
            graph
                .node(root)
                .is_some_and(|node| node.kind == "Axure:Page")
        })
        .map(|(package, _)| (package.clone(), package.clone()))
        .collect();
    let program = compile(&graph, &bindings, &pages);
    assert_eq!(
        program.events.len(),
        66,
        "all 66 nonempty native event trees must survive compilation"
    );
    let click_count = program
        .events
        .iter()
        .filter(|event| event.event_type == 16)
        .count();
    assert!(
        click_count >= 60,
        "fixture native click cases must not be reduced to the three simple navigation commands: {click_count}"
    );
    let mut counts = BTreeMap::<&str, usize>::new();
    for event in &program.events {
        for case in &event.cases {
            for action in &case.actions {
                let kind = match action {
                    PreviewAction::LinkWindow { .. } => "window",
                    PreviewAction::LinkFrame { .. } => "frame",
                    PreviewAction::Fade { .. } => "fade",
                    PreviewAction::Move { .. } => "move",
                    PreviewAction::SetPanelState { .. } => "state",
                    PreviewAction::SetPanelOrder { .. } => "order",
                    PreviewAction::ScrollTo { .. } => "scroll",
                    PreviewAction::SetChecked { .. } => "checked",
                    PreviewAction::SetText { .. } => "text",
                    PreviewAction::Unsupported { .. } => "unsupported",
                };
                *counts.entry(kind).or_default() += 1;
            }
        }
    }
    assert_eq!(counts.get("window"), Some(&3));
    assert_eq!(counts.get("frame"), Some(&27));
    assert_eq!(
        counts.get("fade"),
        Some(&26),
        "{counts:?}\n{:?}",
        program.diagnostics
    );
    assert_eq!(counts.get("move"), Some(&17));
    assert_eq!(counts.get("state"), Some(&6));
    assert_eq!(counts.get("order"), Some(&3));
    assert_eq!(counts.get("scroll"), Some(&2));
    assert_eq!(counts.get("checked"), Some(&39));
    assert!(
        !counts.contains_key("unsupported"),
        "fixture actions must all compile: {counts:?}"
    );
    assert_eq!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("源链接目标不在当前工程中"))
            .count(),
        30
    );
    let missing_widgets = program
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("源元件目标不存在，已跳过"))
        .collect::<Vec<_>>();
    assert_eq!(missing_widgets.len(), 12);
    assert_eq!(
        missing_widgets
            .iter()
            .map(|diagnostic| raw(&diagnostic.source))
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn unsupported_preview_messages_use_neutral_type_names() {
    for scenario in 0..4 {
        let (mut graph, bindings) = fixture();
        match scenario {
            0 => {
                graph.objects.get_mut("move").unwrap().kind =
                    "Axure:InteractionUnknownAction".into()
            }
            1 => graph.objects.get_mut("x").unwrap().kind = "Axure:Expr:FCall".into(),
            2 => graph.objects.get_mut("path").unwrap().kind = "Axure:UnexpectedPath".into(),
            3 => {
                let action = graph.objects.get_mut("move").unwrap();
                action.kind = "Axure:InterationActionSetFunction".into();
                action.properties = json!({"action":reference("x")});
            }
            _ => unreachable!(),
        }
        let result = compile(&graph, &bindings, &BTreeMap::new());
        assert!(!result.events[0].cases[0].supported);
        assert!(!result.diagnostics.is_empty());
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.to_lowercase().contains("axure"), "{serialized}");
    }
}
