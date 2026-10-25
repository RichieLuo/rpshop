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
mod tests;
