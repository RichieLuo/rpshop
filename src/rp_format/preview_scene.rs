//! Independent native runtime scenes. Editor models and native parts are never mutated.

use super::preview_program::{PageTarget, PreviewProgram, SourceBinding};
use super::*;
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct NativePreview {
    pub scene: PreviewScene,
    pub program: PreviewProgram,
}

#[derive(Clone, Default, Serialize)]
pub struct PreviewScene {
    pub pages: Vec<ScenePage>,
    #[serde(skip)]
    pub bindings: Vec<SourceBinding>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct ScenePage {
    pub key: String,
    pub event_key: String,
    pub name: String,
    pub width: f32,
    pub height: f32,
    pub canvas: CanvasMode,
    pub background: String,
    pub nodes: Vec<SceneNode>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneNodeKind {
    Element,
    Group,
    Panel,
    Frame,
}

#[derive(Clone, Serialize)]
pub struct SceneNode {
    pub key: String,
    pub source: String,
    pub scope: Vec<String>,
    pub widget: Widget,
    pub kind: SceneNodeKind,
    pub children: Vec<SceneNode>,
    pub states: Vec<SceneState>,
    pub initial_state: Option<String>,
    pub initial_page: Option<PageTarget>,
    pub selected: bool,
    pub selection_group: String,
    pub selected_style: Option<Widget>,
    pub hover_style: Option<Widget>,
    pub mouse_down_style: Option<Widget>,
    pub scrollbars: i64,
    pub show_border: bool,
    pub fit_to_content: bool,
}

#[derive(Clone, Serialize)]
pub struct SceneState {
    pub key: String,
    pub event_key: String,
    pub name: String,
    pub nodes: Vec<SceneNode>,
}

pub(super) fn collect(
    parts: &BTreeMap<u64, Graph>,
    page_packages: &BTreeMap<u64, String>,
    edited: &Project,
) -> Result<(PreviewScene, Graph, BTreeMap<String, String>)> {
    let mut graph = parts
        .values()
        .find(|graph| {
            graph
                .root()
                .is_some_and(|node| node.kind == "Axure:DesignDocument")
        })
        .context("原生预览缺少文档根图")?
        .clone();
    let mut roots = BTreeMap::new();
    for part in parts.values().filter(|part| {
        part.root()
            .is_none_or(|node| node.kind != "Axure:DesignDocument")
    }) {
        let mut scoped = scope_package(part);
        // In-memory native additions do not have their binary token metadata
        // until encoded. References to objects present in this same package are
        // local; scope these additions just like parsed token-3 references.
        fn scope_staged_references(value: &mut Value, part: &Graph) {
            match value {
                Value::Object(map) => {
                    if !map.contains_key("$token")
                        && let Some(id) = map
                            .get("$ref")
                            .and_then(Value::as_str)
                            .filter(|id| part.objects.contains_key(*id))
                            .map(str::to_owned)
                    {
                        map.insert(
                            "$ref".into(),
                            Value::String(format!("{}/{id}", part.package)),
                        );
                    }
                    for value in map.values_mut() {
                        scope_staged_references(value, part);
                    }
                }
                Value::Array(array) => {
                    for value in array {
                        scope_staged_references(value, part);
                    }
                }
                _ => {}
            }
        }
        for node in scoped.objects.values_mut() {
            scope_staged_references(&mut node.properties, part);
        }
        roots.insert(part.package.clone(), scoped.root);
        graph.objects.extend(scoped.objects);
        ensure!(
            graph.objects.len() <= MAX_OBJECTS,
            "原生预览对象数量超过限制"
        );
    }
    let page_keys = page_packages
        .values()
        .map(|package| (package.clone(), package.clone()))
        .collect::<BTreeMap<_, _>>();
    let styles = Styles::new(&graph);
    let converter = Converter {
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
    let mut collector = Collector {
        converter,
        page_keys: &page_keys,
        bindings: Vec::new(),
        diagnostics: BTreeSet::new(),
        keys: BTreeSet::new(),
        count: 0,
    };
    let mut pages = Vec::new();
    for page in &edited.pages {
        let package = page_packages
            .get(&page.id)
            .context("原生预览页面绑定不存在")?;
        let source = roots.get(package).context("原生预览页面图不存在")?;
        let root = graph.node(source).context("原生预览页面根对象不存在")?;
        let diagram = root
            .properties
            .get("diagram")
            .and_then(reference_id)
            .context("原生预览页面缺少画布")?;
        let scope = vec![package.clone()];
        let event_key = format!("page:{package}");
        collector.bind(&event_key, source, &scope)?;
        let nodes = collector.diagram(diagram, &scope, &mut BTreeSet::new(), 0)?;
        let size = page.size_for_output();
        pages.push(ScenePage {
            key: package.clone(),
            event_key,
            name: page.name.clone(),
            width: size[0],
            height: size[1],
            canvas: page.canvas,
            background: page.background.clone(),
            nodes,
        });
    }
    let scene = PreviewScene {
        pages,
        bindings: collector.bindings,
        diagnostics: collector.diagnostics.into_iter().collect(),
    };
    Ok((scene, graph, page_keys))
}

struct Collector<'a> {
    converter: Converter<'a>,
    page_keys: &'a BTreeMap<String, String>,
    bindings: Vec<SourceBinding>,
    diagnostics: BTreeSet<String>,
    keys: BTreeSet<String>,
    count: usize,
}

impl Collector<'_> {
    fn bind(&mut self, key: &str, source: &str, scope: &[String]) -> Result<()> {
        ensure!(self.keys.insert(key.to_owned()), "原生预览存在重复场景节点");
        self.bindings.push(SourceBinding {
            key: key.to_owned(),
            source: source.to_owned(),
            scope: scope.to_vec(),
        });
        Ok(())
    }

    fn diagram(
        &mut self,
        id: &str,
        scope: &[String],
        ancestors: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<Vec<SceneNode>> {
        ensure!(
            depth <= 64 && ancestors.insert(id.to_owned()),
            "原生预览画布存在循环或嵌套过深"
        );
        let node = self
            .converter
            .graph
            .node(id)
            .context("原生预览画布引用不存在")?;
        let references = direct_diagram_references(
            self.converter.graph,
            array(&node.properties["diagram-objects"]),
        )?;
        let mut nodes = Vec::new();
        for reference in references {
            if let Some(id) = reference_id(&reference) {
                nodes.push(self.node(id, scope, ancestors, depth + 1)?);
            }
        }
        ancestors.remove(id);
        Ok(nodes)
    }

    fn node(
        &mut self,
        id: &str,
        scope: &[String],
        ancestors: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<SceneNode> {
        ensure!(
            depth <= 64 && ancestors.insert(id.to_owned()),
            "原生预览元件存在循环或嵌套过深"
        );
        self.count += 1;
        ensure!(self.count <= 20_000, "原生预览全部状态超过 20000 个元件");
        let source = self
            .converter
            .graph
            .node(id)
            .context("原生预览元件引用不存在")?;
        let p = &source.properties;
        let style = self.converter.styles.for_node(self.converter.graph, source);
        let location = style
            .get(&101)
            .or_else(|| p.get("ContainedLocation"))
            .unwrap_or(&Value::Null);
        let position = (
            number(&location["X"]).unwrap_or(0.0),
            number(&location["Y"]).unwrap_or(0.0),
        );
        let hidden = p["VisibleFromSettings"].as_bool() == Some(false);
        let key = format!("node:{}/{}", scope.join("/"), id);
        self.bind(&key, id, scope)?;
        let layer = source.kind.ends_with(":Layer");
        let widget = if layer {
            let mut visual = Widget::new(self.converter.allocate(), WidgetKind::Panel, 0.0, 0.0);
            visual.fill = "#FFFFFF00".into();
            visual.stroke = "#FFFFFF00".into();
            visual.hidden = hidden;
            visual.name = truncate(
                p.pointer("/name-block/text")
                    .and_then(Value::as_str)
                    .unwrap_or("组合"),
                1024,
            );
            visual
        } else {
            self.converter
                .visual_widget(id, source, &style, position, hidden)?
        };
        let selected_style = self.state_style(source, &style, position, hidden, "Selected")?;
        let hover_style = self.state_style(source, &style, position, hidden, "MouseOver")?;
        let mouse_down_style = self.state_style(source, &style, position, hidden, "MouseDown")?;
        let mut node = SceneNode {
            key,
            source: id.to_owned(),
            scope: scope.to_vec(),
            widget,
            kind: SceneNodeKind::Element,
            children: Vec::new(),
            states: Vec::new(),
            initial_state: None,
            initial_page: None,
            selected: p["Selected"].as_bool().unwrap_or(false),
            selection_group: truncate(p["SelectedStyleGroup"].as_str().unwrap_or(""), 1024),
            selected_style,
            hover_style,
            mouse_down_style,
            scrollbars: p["Scrollbars"]
                .as_i64()
                .or_else(|| p["scrollbars"].as_i64())
                .unwrap_or(0),
            show_border: p["ShowBorder"].as_bool().unwrap_or(false),
            fit_to_content: p["fit-to-content"].as_bool().unwrap_or(false),
        };
        if layer || source.kind.ends_with(":Table") || source.kind.ends_with(":MenuObject") {
            node.kind = SceneNodeKind::Group;
            let references = if layer {
                array(&p["DiagramObjects"])
            } else {
                array(&p["diagram-objects"])
            };
            for reference in references {
                if let Some(child) = reference_id(reference) {
                    node.children
                        .push(self.node(child, scope, ancestors, depth + 1)?);
                }
            }
            if layer {
                // Native Layer children are stored in the diagram's coordinates.
                // Give the runtime group its actual bounds and move direct
                // children to local coordinates so absolute Move uses that box.
                if !node.children.is_empty() {
                    let bounds = node
                        .children
                        .iter()
                        .map(|child| crate::manipulation::rotated_bounds(&child.widget))
                        .fold(
                            [
                                f32::INFINITY,
                                f32::INFINITY,
                                f32::NEG_INFINITY,
                                f32::NEG_INFINITY,
                            ],
                            |a, b| {
                                [
                                    a[0].min(b[0]),
                                    a[1].min(b[1]),
                                    a[2].max(b[2]),
                                    a[3].max(b[3]),
                                ]
                            },
                        );
                    node.widget.x = finite(bounds[0], -100_000.0, 100_000.0);
                    node.widget.y = finite(bounds[1], -100_000.0, 100_000.0);
                    node.widget.width = finite(bounds[2] - bounds[0], 1.0, 20_000.0);
                    node.widget.height = finite(bounds[3] - bounds[1], 1.0, 20_000.0);
                    for child in &mut node.children {
                        child.widget.x -= node.widget.x;
                        child.widget.y -= node.widget.y;
                    }
                } else {
                    node.widget.width = 1.0;
                    node.widget.height = 1.0;
                }
            }
        } else if source.kind.ends_with(":ReferenceDiagramObject") {
            node.kind = SceneNodeKind::Group;
            let master = p
                .get("ReferredMaster")
                .and_then(reference_id)
                .and_then(|package| self.converter.roots.get(package))
                .and_then(|root| self.converter.graph.node(root));
            if let Some(diagram) = master
                .and_then(|root| root.properties.get("diagram"))
                .and_then(reference_id)
            {
                let mut child_scope = scope.to_vec();
                child_scope.push(node.key.clone());
                node.children = self.diagram(diagram, &child_scope, ancestors, depth + 1)?;
            } else {
                self.diagnostics
                    .insert("无法读取的母版实例暂显示为空容器".into());
            }
        } else if source.kind.ends_with(":DynamicPanel") {
            node.kind = SceneNodeKind::Panel;
            for reference in array(&p["panel-diagrams"]) {
                let Some(package) = reference_id(reference) else {
                    continue;
                };
                let root_id = self
                    .converter
                    .roots
                    .get(package)
                    .map(String::as_str)
                    .unwrap_or(package);
                let state = self
                    .converter
                    .graph
                    .node(root_id)
                    .context("原生预览动态面板状态不存在")?;
                let diagram_id = state
                    .properties
                    .get("diagram")
                    .and_then(reference_id)
                    .unwrap_or(root_id);
                let diagram = self
                    .converter
                    .graph
                    .node(diagram_id)
                    .context("原生预览动态面板画布不存在")?;
                let event_key = format!("{}/state:{package}", node.key);
                self.bind(&event_key, root_id, scope)?;
                let children = self.diagram(diagram_id, scope, ancestors, depth + 1)?;
                let name = diagram
                    .properties
                    .get("Name")
                    .or_else(|| diagram.properties.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("状态");
                node.states.push(SceneState {
                    key: package.to_owned(),
                    event_key,
                    name: truncate(name, 1024),
                    nodes: children,
                });
            }
            node.initial_state = node.states.first().map(|state| state.key.clone());
        } else if source.kind.ends_with(":InlineFrame") {
            node.kind = SceneNodeKind::Frame;
            node.initial_page = p.get("DefaultTarget").map(|value| {
                super::preview_program::compile_page_target(
                    self.converter.graph,
                    value,
                    self.page_keys,
                )
            });
            if node
                .initial_page
                .as_ref()
                .is_some_and(|target| target.key.is_none() && !target.literal.is_empty())
            {
                self.diagnostics
                    .insert("外部内联框架目标未加载，仅支持本工程内页面".into());
            }
            if !node.show_border {
                node.widget.stroke = "#FFFFFF00".into();
            }
        } else if source.kind.ends_with(":Repeater") {
            node.kind = SceneNodeKind::Group;
            if let Some(diagram) = p.get("BaseDiagram").and_then(reference_id) {
                node.children = self.diagram(diagram, scope, ancestors, depth + 1)?;
            }
            self.diagnostics
                .insert("Repeater 当前预览一份模板，未执行数据筛选和重复逻辑".into());
        }
        ancestors.remove(id);
        Ok(node)
    }

    fn state_style(
        &mut self,
        source: &Node,
        base: &Properties,
        position: (f32, f32),
        hidden: bool,
        name: &str,
    ) -> Result<Option<Widget>> {
        let Some(reference) = base.get(&21).and_then(|value| value.get(name)) else {
            return Ok(None);
        };
        let Some(state) = self.converter.graph.resolve(reference) else {
            return Ok(None);
        };
        let overrides = props(&state.properties["PropList"]);
        let mut explicit = Properties::new();
        if let Some(id) = overrides.get(&0).and_then(reference_id) {
            self.converter
                .styles
                .extend_named(id, &mut explicit, &mut BTreeSet::new());
        }
        explicit.extend(overrides);
        let mut style = base.clone();
        style.extend(explicit.clone());
        // State overrides alter visual properties; a source's text content and
        // source identity remain the same, and the caller owns runtime selection.
        let mut widget = self.converter.visual_widget(
            "preview-state-style",
            source,
            &style,
            position,
            hidden,
        )?;
        // Inline rich text describes the base appearance. An interaction style's
        // explicit text properties take precedence while that state is active.
        if let Some(color) = explicit.get(&6).and_then(fill_color) {
            widget.text_color = color;
        }
        if let Some(size) = explicit.get(&2).and_then(number) {
            widget.font_size = finite(size * 4.0 / 3.0, 4.0, 512.0);
        }
        if let Some(bold) = explicit
            .get(&1001)
            .and_then(number)
            .map(|weight| weight >= 600.0)
            .or_else(|| explicit.get(&3).and_then(Value::as_bool))
            && let Some(layout) = &mut widget.text_layout
        {
            layout.bold = bold;
        }
        Ok(Some(widget))
    }
}

#[cfg(test)]
mod tests {
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
            eprintln!(
                "native scene event types: {event_counts:?}; unsupported cases: {unsupported}"
            );
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
}
