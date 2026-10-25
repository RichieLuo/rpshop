use super::*;
use serde_json::json;

fn node(kind: &str, properties: Value) -> Node {
    Node {
        kind: kind.into(),
        properties,
    }
}
fn local(id: &str) -> Value {
    json!({"$ref":format!("object-{id}"),"$token":3})
}
fn package(id: &str) -> Value {
    json!({"$ref":id,"$token":19})
}
fn graph(package: &str, root: &str, objects: Vec<(&str, Node)>) -> Graph {
    Graph::new(
        [11, 0, 0, 0],
        format!("object-{root}"),
        package.into(),
        objects
            .into_iter()
            .map(|(id, node)| (format!("object-{id}"), node))
            .collect(),
    )
}
fn fixture() -> (BTreeMap<u64, Graph>, BTreeMap<u64, String>, Project) {
    let parts = BTreeMap::from([
        (
            0,
            graph(
                "doc-package",
                "doc",
                vec![("doc", node("Axure:DesignDocument", json!({})))],
            ),
        ),
        (
            1,
            graph(
                "page-package",
                "page",
                vec![
                    (
                        "page",
                        node("Axure:Page", json!({"diagram":local("diagram")})),
                    ),
                    (
                        "diagram",
                        node(
                            "Axure:PageDiagram",
                            json!({"diagram-objects":[local("panel"),local("frame"),local("label"),local("master-one"),local("master-two")]}),
                        ),
                    ),
                    (
                        "panel",
                        node(
                            "Axure:DynamicPanel",
                            json!({"ContainedLocation":{"X":100,"Y":200},"VisibleFromSettings":false,"panel-diagrams":[package("state-a"),package("state-b")]}),
                        ),
                    ),
                    (
                        "frame",
                        node(
                            "Axure:InlineFrame",
                            json!({"DefaultTarget":local("target"),"ShowBorder":true,"Scrollbars":1}),
                        ),
                    ),
                    (
                        "target",
                        node(
                            "Axure:InteractionTarget",
                            json!({"target-type":1,"target-page-id":package("page-package")}),
                        ),
                    ),
                    (
                        "label",
                        node(
                            "Axure:VectorShape",
                            json!({"MyStyle":local("base-style"),"Selected":true,"SelectedStyleGroup":"navigation","RichTextBlock":{"attributed-string":{"Paragraphs":[{"Inlines":[{"Text":"Choice","Size":10,"Color":-16777216,"Weight":400}]}]}}}),
                        ),
                    ),
                    (
                        "base-style",
                        node(
                            "Axure:Style",
                            json!({"PropList":[{"DOAName":21,"Value":{"Selected":local("selected-style"),"MouseOver":local("hover-style")}}]}),
                        ),
                    ),
                    (
                        "selected-style",
                        node(
                            "Axure:Style",
                            json!({"PropList":[{"DOAName":6,"Value":{"fill-type":"solid","fill-color":-65536}},{"DOAName":7,"Value":{"fill-type":"solid","fill-color":-16711936}},{"DOAName":2,"Value":18},{"DOAName":3,"Value":true}]}),
                        ),
                    ),
                    (
                        "hover-style",
                        node(
                            "Axure:Style",
                            json!({"PropList":[{"DOAName":1001,"Value":700}]}),
                        ),
                    ),
                    (
                        "master-one",
                        node(
                            "Axure:ReferenceDiagramObject",
                            json!({"ReferredMaster":package("master-package"),"ContainedLocation":{"X":300,"Y":20}}),
                        ),
                    ),
                    (
                        "master-two",
                        node(
                            "Axure:ReferenceDiagramObject",
                            json!({"ReferredMaster":package("master-package"),"ContainedLocation":{"X":600,"Y":20}}),
                        ),
                    ),
                ],
            ),
        ),
        (
            2,
            graph(
                "state-a",
                "state",
                vec![
                    (
                        "state",
                        node("Axure:PanelState", json!({"diagram":local("diagram")})),
                    ),
                    (
                        "diagram",
                        node(
                            "Axure:PanelDiagram",
                            json!({"Name":"First","diagram-objects":[local("child")]}),
                        ),
                    ),
                    (
                        "child",
                        node(
                            "Axure:VectorShape",
                            json!({"ContainedLocation":{"X":7,"Y":9},"VisibleFromSettings":true}),
                        ),
                    ),
                ],
            ),
        ),
        (
            3,
            graph(
                "state-b",
                "state",
                vec![
                    (
                        "state",
                        node("Axure:PanelState", json!({"diagram":local("diagram")})),
                    ),
                    (
                        "diagram",
                        node(
                            "Axure:PanelDiagram",
                            json!({"Name":"Second","diagram-objects":[local("child")]}),
                        ),
                    ),
                    (
                        "child",
                        node(
                            "Axure:VectorShape",
                            json!({"ContainedLocation":{"X":17,"Y":19}}),
                        ),
                    ),
                ],
            ),
        ),
        (
            4,
            graph(
                "master-package",
                "master",
                vec![
                    (
                        "master",
                        node("Axure:Master", json!({"diagram":local("diagram")})),
                    ),
                    (
                        "diagram",
                        node(
                            "Axure:MasterDiagram",
                            json!({"diagram-objects":[local("child")]}),
                        ),
                    ),
                    (
                        "child",
                        node(
                            "Axure:VectorShape",
                            json!({"ContainedLocation":{"X":11,"Y":13}}),
                        ),
                    ),
                ],
            ),
        ),
    ]);
    (
        parts,
        BTreeMap::from([(1, "page-package".into())]),
        Project::blank(),
    )
}

#[test]
fn collects_all_panel_states_with_local_geometry_and_own_visibility() {
    let (parts, pages, project) = fixture();
    let before = parts
        .iter()
        .map(|(offset, graph)| {
            (
                *offset,
                graph
                    .objects
                    .iter()
                    .map(|(id, node)| (id.clone(), node.properties.clone()))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let (scene, _, _) = collect(&parts, &pages, &project).unwrap();
    let panel = &scene.pages[0].nodes[0];
    assert!(panel.kind == SceneNodeKind::Panel);
    assert!(panel.widget.hidden);
    assert_eq!((panel.widget.x, panel.widget.y), (100.0, 200.0));
    assert_eq!(panel.states.len(), 2);
    assert_eq!(panel.initial_state.as_deref(), Some("state-a"));
    assert_eq!(
        (
            panel.states[0].nodes[0].widget.x,
            panel.states[0].nodes[0].widget.y
        ),
        (7.0, 9.0)
    );
    assert!(!panel.states[0].nodes[0].widget.hidden);
    assert_eq!(panel.states[1].nodes[0].source, "state-b/object-child");
    assert!(
        scene
            .bindings
            .iter()
            .any(|binding| binding.source == "state-b/object-state")
    );
    let after = parts
        .iter()
        .map(|(offset, graph)| {
            (
                *offset,
                graph
                    .objects
                    .iter()
                    .map(|(id, node)| (id.clone(), node.properties.clone()))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert!(before == after);
    assert!(project == Project::blank());
}

#[test]
fn preserves_local_frame_targets_and_isolates_master_instances() {
    let (parts, pages, project) = fixture();
    let (scene, _, _) = collect(&parts, &pages, &project).unwrap();
    let nodes = &scene.pages[0].nodes;
    assert!(nodes[1].kind == SceneNodeKind::Frame);
    assert_eq!(
        nodes[1].initial_page.as_ref().unwrap().key.as_deref(),
        Some("page-package")
    );
    assert_eq!(nodes[1].scrollbars, 1);
    assert!(nodes[1].show_border);
    let first = &nodes[3].children[0];
    let second = &nodes[4].children[0];
    assert_eq!(first.source, second.source);
    assert_ne!(first.key, second.key);
    assert_ne!(first.scope, second.scope);
    assert_eq!((first.widget.x, first.widget.y), (11.0, 13.0));
    assert_eq!((second.widget.x, second.widget.y), (11.0, 13.0));
}

#[test]
fn explicit_interaction_text_styles_override_base_inline_formatting() {
    let (parts, pages, project) = fixture();
    let (scene, _, _) = collect(&parts, &pages, &project).unwrap();
    let label = &scene.pages[0].nodes[2];
    assert!(label.selected);
    assert_eq!(label.selection_group, "navigation");
    assert_eq!(label.widget.text_color, "#000000");
    assert_eq!(label.widget.font_size, 10.0 * 4.0 / 3.0);
    assert!(!label.widget.text_layout.as_ref().unwrap().bold);
    let selected = label.selected_style.as_ref().unwrap();
    assert_eq!(selected.text_color, "#FF0000");
    assert_eq!(selected.fill, "#00FF00");
    assert_eq!(selected.font_size, 24.0);
    assert!(selected.text_layout.as_ref().unwrap().bold);
    assert!(
        label
            .hover_style
            .as_ref()
            .unwrap()
            .text_layout
            .as_ref()
            .unwrap()
            .bold
    );
}

#[test]
fn nested_layers_use_bounds_as_move_origin_and_localize_children_once() {
    let (mut parts, pages, project) = fixture();
    let graph = parts.get_mut(&1).unwrap();
    graph.objects.get_mut("object-diagram").unwrap().properties["diagram-objects"]
        .as_array_mut()
        .unwrap()
        .push(local("outer"));
    for (id, source) in [
        (
            "outer",
            node(
                "Axure:Layer",
                json!({"DiagramObjects":[local("inner"),local("other")]}),
            ),
        ),
        (
            "inner",
            node(
                "Axure:Layer",
                json!({"DiagramObjects":[local("nested-child")]}),
            ),
        ),
        (
            "nested-child",
            node(
                "Axure:VectorShape",
                json!({"ContainedLocation":{"X":110,"Y":220},"ContainedSize":{"Width":10,"Height":10}}),
            ),
        ),
        (
            "other",
            node(
                "Axure:VectorShape",
                json!({"ContainedLocation":{"X":200,"Y":250},"ContainedSize":{"Width":10,"Height":10}}),
            ),
        ),
    ] {
        graph.objects.insert(format!("object-{id}"), source);
    }
    let (scene, _, _) = collect(&parts, &pages, &project).unwrap();
    let group = scene.pages[0].nodes.last().unwrap();
    assert_eq!(
        (
            group.widget.x,
            group.widget.y,
            group.widget.width,
            group.widget.height
        ),
        (110.0, 220.0, 100.0, 40.0)
    );
    let inner = &group.children[0];
    assert_eq!((inner.widget.x, inner.widget.y), (0.0, 0.0));
    assert_eq!(
        (inner.children[0].widget.x, inner.children[0].widget.y),
        (0.0, 0.0)
    );
    assert_eq!(
        (group.children[1].widget.x, group.children[1].widget.y),
        (90.0, 30.0)
    );
    let moved = (
        609.0 + inner.widget.x + inner.children[0].widget.x,
        80.0 + inner.widget.y + inner.children[0].widget.y,
    );
    assert_eq!(moved, (609.0, 80.0));
}

#[test]
fn staged_local_references_without_binary_tokens_are_scoped_before_collecting() {
    let (mut parts, pages, project) = fixture();
    let graph = parts.get_mut(&1).unwrap();
    graph.objects.get_mut("object-diagram").unwrap().properties["diagram-objects"]
        .as_array_mut()
        .unwrap()
        .push(json!({"$ref":"object-added"}));
    graph.objects.insert(
        "object-added".into(),
        node(
            "Axure:VectorShape",
            json!({
                "MyStyle":{"$ref":"object-base-style"},"TextBlock":{"text":"Staged local node"}
            }),
        ),
    );
    let (scene, _, _) = collect(&parts, &pages, &project).unwrap();
    let added = scene.pages[0].nodes.last().unwrap();
    assert_eq!(added.source, "page-package/object-added");
    assert_eq!(added.widget.text, "Staged local node");
    assert!(added.selected_style.is_some());
}

fn walk<'a>(nodes: &'a [SceneNode], output: &mut Vec<&'a SceneNode>) {
    for node in nodes {
        output.push(node);
        walk(&node.children, output);
        for state in &node.states {
            walk(&state.nodes, output);
        }
    }
}

#[test]
#[ignore = "requires caller-selected native fixtures; reads original files only"]
fn caller_selected_native_preview_stages_unsaved_edits_without_mutating_source() {
    for variable in ["RPSHOP_NATIVE_FIXTURE", "RPSHOP_NATIVE_V11_FIXTURE"] {
        let Some(path) = std::env::var_os(variable) else {
            continue;
        };
        let path = std::path::PathBuf::from(path);
        let original = std::fs::read(&path).unwrap();
        let document = super::super::open(&path).unwrap();
        let baseline = document.project.clone();
        let session = document.session_state();
        let start = std::time::Instant::now();
        let preview = document.preview_scene(&baseline).unwrap();
        eprintln!(
            "native scene: {} pages, {} bindings, {} events, {}ms",
            preview.scene.pages.len(),
            preview.scene.bindings.len(),
            preview.program.events.len(),
            start.elapsed().as_millis()
        );
        let mut event_counts = BTreeMap::new();
        for event in &preview.program.events {
            *event_counts.entry(event.event_type).or_insert(0_usize) += 1;
        }
        let unsupported = preview
            .program
            .events
            .iter()
            .flat_map(|event| &event.cases)
            .filter(|case| !case.supported)
            .count();
        eprintln!("native scene event types: {event_counts:?}; unsupported cases: {unsupported}");
        let container = super::super::container::open(&path).unwrap();
        let root = &container.index["root"];
        let raw = super::super::binary::parse(
            &container
                .part(root["parts"]["DesignDocument"].as_u64().unwrap())
                .unwrap(),
        )
        .unwrap();
        let names = package_names(&raw);
        let mut graph = raw.clone();
        let mut roots = BTreeMap::new();
        let mut bytes = 0;
        let mut raw_parts = BTreeMap::new();
        load_packages(
            &container,
            root,
            &names,
            &mut graph,
            &mut roots,
            &mut bytes,
            &mut raw_parts,
            0,
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
        let page_keys = roots
            .iter()
            .filter(|(_, root)| {
                graph
                    .node(root)
                    .is_some_and(|node| node.kind == "Axure:Page")
            })
            .map(|(package, _)| (package.clone(), package.clone()))
            .collect();
        let all = super::super::preview_program::compile(&graph, &bindings, &page_keys);
        let visible_sources = preview
            .scene
            .bindings
            .iter()
            .map(|binding| binding.source.as_str())
            .collect::<BTreeSet<_>>();
        let mut unreachable = BTreeMap::new();
        for event in all
            .events
            .iter()
            .filter(|event| !visible_sources.contains(event.owner.as_str()))
        {
            let kind = event
                .owner
                .rsplit_once('/')
                .and_then(|(package, _)| roots.get(package))
                .and_then(|id| graph.node(id))
                .map(|node| node.kind.as_str())
                .unwrap_or("unknown");
            *unreachable
                .entry((kind, event.event_type))
                .or_insert(0_usize) += 1;
        }
        eprintln!("native events outside rendered instances by package kind: {unreachable:?}");
        assert_eq!(preview.scene.pages.len(), baseline.pages.len());
        let mut nodes = Vec::new();
        for page in &preview.scene.pages {
            walk(&page.nodes, &mut nodes);
        }
        assert!(!nodes.is_empty());
        let mut draft = baseline.clone();
        let selected = draft
            .pages
            .iter()
            .enumerate()
            .find_map(|(pi, page)| {
                page.widgets.iter().enumerate().find_map(|(wi, widget)| {
                    (document.is_editable_widget(widget.id) && !widget.text.is_empty())
                        .then_some((pi, wi))
                })
            })
            .expect("fixture needs editable text");
        draft.pages[selected.0].widgets[selected.1].text = "Native runtime unsaved text".into();
        let id = draft.allocate_id();
        let mut added = Widget::new(id, WidgetKind::Rectangle, 321.0, 432.0);
        added.text = "Native runtime added widget".into();
        draft.pages[0].widgets.push(added);
        let start = std::time::Instant::now();
        let staged = document.preview_scene(&draft).unwrap();
        eprintln!(
            "native staged scene: {} bindings, {}ms",
            staged.scene.bindings.len(),
            start.elapsed().as_millis()
        );
        let mut staged_nodes = Vec::new();
        for page in &staged.scene.pages {
            walk(&page.nodes, &mut staged_nodes);
        }
        assert!(
            staged_nodes
                .iter()
                .any(|node| node.widget.text == "Native runtime unsaved text")
        );
        assert!(
            staged_nodes
                .iter()
                .any(|node| node.widget.text == "Native runtime added widget"
                    && node.widget.x == 321.0
                    && node.widget.y == 432.0)
        );
        let removed = draft.pages[selected.0].widgets.remove(selected.1);
        assert!(removed.text == "Native runtime unsaved text");
        let deleted = document.preview_scene(&draft).unwrap();
        let mut deleted_nodes = Vec::new();
        for page in &deleted.scene.pages {
            walk(&page.nodes, &mut deleted_nodes);
        }
        assert!(
            !deleted_nodes
                .iter()
                .any(|node| node.widget.text == "Native runtime unsaved text")
        );
        assert!(document.project == baseline);
        assert!(document.session_state() == session);
        assert!(std::fs::read(&path).unwrap() == original);
    }
}
