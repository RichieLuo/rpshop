//! Read-only compilation of native event trees into browser preview commands.
//! Unknown guards or actions disable their case rather than running a partial chain.

use super::binary::{Graph, Node, reference_id};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const EMPTY: &str = "00000000000000000000000000000000";

#[derive(Clone, Debug, Serialize)]
pub struct SourceBinding {
    pub key: String,
    pub source: String,
    pub scope: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PreviewProgram {
    pub events: Vec<PreviewEvent>,
    pub diagnostics: Vec<PreviewDiagnostic>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreviewEvent {
    pub owner: String,
    pub event_type: i64,
    pub cases: Vec<PreviewCase>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreviewCase {
    pub source: String,
    pub name: String,
    pub new_if_group: bool,
    pub supported: bool,
    pub actions: Vec<PreviewAction>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreviewDiagnostic {
    pub owner: String,
    pub source: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PageTarget {
    pub key: Option<String>,
    pub native_id: String,
    pub literal: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreviewAction {
    LinkWindow {
        page: PageTarget,
        mode: i64,
    },
    LinkFrame {
        frames: Vec<FrameLink>,
    },
    Fade {
        entries: Vec<FadeTarget>,
    },
    Move {
        entries: Vec<MoveTarget>,
    },
    SetPanelState {
        entries: Vec<StateTarget>,
    },
    SetPanelOrder {
        entries: Vec<OrderTarget>,
    },
    ScrollTo {
        targets: Vec<String>,
        direction: i64,
        easing: i64,
        duration: f32,
    },
    SetChecked {
        targets: Vec<String>,
        value: bool,
    },
    SetText {
        targets: Vec<String>,
        value: String,
    },
    Unsupported {
        kind: String,
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct FrameLink {
    pub targets: Vec<String>,
    pub page: PageTarget,
}

#[derive(Clone, Debug, Serialize)]
pub struct FadeTarget {
    pub targets: Vec<String>,
    pub action: i64,
    pub easing: i64,
    pub duration: f32,
    pub hide_easing: i64,
    pub hide_duration: f32,
    pub show_type: i64,
    pub bring_to_front: bool,
    pub vertical: bool,
    pub compress_easing: i64,
    pub compress_duration: f32,
    pub background: Option<String>,
    pub lightbox_close: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MoveTarget {
    pub targets: Vec<String>,
    pub mode: i64,
    pub x: f32,
    pub y: f32,
    pub easing: i64,
    pub duration: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct StateTarget {
    pub targets: Vec<String>,
    pub mode: i64,
    pub state_key: Option<String>,
    pub value: String,
    pub looping: bool,
    pub show_when_set: bool,
    pub exit_easing: i64,
    pub exit_duration: f32,
    pub enter_easing: i64,
    pub enter_duration: f32,
    pub repeat: bool,
    pub repeat_interval: f32,
    pub repeat_skip_first: bool,
    pub compress: bool,
    pub compress_easing: i64,
    pub compress_duration: f32,
    pub vertical: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OrderTarget {
    pub targets: Vec<String>,
    pub front: bool,
}

pub fn compile(
    graph: &Graph,
    bindings: &[SourceBinding],
    page_keys: &BTreeMap<String, String>,
) -> PreviewProgram {
    let mut compiler = Compiler {
        graph,
        bindings,
        page_keys,
        output: PreviewProgram::default(),
        steps: 0,
    };
    for binding in bindings {
        if compiler.steps > 100_000 {
            compiler.diagnostic(binding, &binding.source, "预览事件数量超过编译上限".into());
            break;
        }
        compiler.owner(binding);
    }
    compiler.output
}

/// Accept either an InteractionTarget reference or its already resolved properties.
pub fn compile_page_target(
    graph: &Graph,
    value: &Value,
    page_keys: &BTreeMap<String, String>,
) -> PageTarget {
    let properties = graph.resolve(value).map_or(value, |node| &node.properties);
    let native_id = reference_id(&properties["target-page-id"])
        .unwrap_or("")
        .to_owned();
    let literal = literal(graph, &properties["target-literal"], 0).unwrap_or_default();
    let key = page_keys.get(&native_id).cloned().or_else(|| {
        graph.node(&native_id).and_then(|node| {
            if node.kind != "Axure:Page" {
                return None;
            }
            native_id
                .rsplit_once('/')
                .and_then(|(package, _)| page_keys.get(package))
                .cloned()
        })
    });
    PageTarget {
        key,
        native_id,
        literal,
    }
}

fn literal(graph: &Graph, value: &Value, depth: usize) -> Result<String, String> {
    if depth > 32 {
        return Err("表达式嵌套超过上限".into());
    }
    if let Some(text) = value.as_str() {
        return Ok(text.to_owned());
    }
    if value.is_number() || value.is_boolean() {
        return Ok(value.to_string());
    }
    let node = graph
        .resolve(value)
        .ok_or_else(|| "表达式引用不存在".to_owned())?;
    if node.kind != "Axure:Expr:StringLiteral" {
        return Err(format!("暂不执行表达式 {}", node.kind));
    }
    if node.properties["variable-identifiers"]
        .as_array()
        .is_some_and(|values| !values.is_empty())
    {
        return Err("暂不执行带变量的表达式".into());
    }
    node.properties["value"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "文字表达式缺少内容".into())
}

fn numeric(graph: &Graph, value: &Value) -> Result<f32, String> {
    let text = literal(graph, value, 0)?;
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| "移动坐标不是有限数字常量".into())
}

fn integer(value: &Value, default: i64) -> i64 {
    value.as_i64().unwrap_or(default)
}
fn number(value: &Value, default: f32) -> f32 {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .map_or(default, |value| value.clamp(0.0, 3_600_000.0) as f32)
}
fn boolean(value: &Value) -> bool {
    value.as_bool().unwrap_or(false)
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}
fn raw(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}
fn disabled(node: &Node) -> bool {
    node.properties["Disabled"].as_bool() == Some(true)
}
fn color(value: &Value) -> Option<String> {
    let argb = value.as_i64()? as u32;
    Some(format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        (argb >> 16) & 255,
        (argb >> 8) & 255,
        argb & 255,
        (argb >> 24) & 255
    ))
}

struct Compiler<'a> {
    graph: &'a Graph,
    bindings: &'a [SourceBinding],
    page_keys: &'a BTreeMap<String, String>,
    output: PreviewProgram,
    steps: usize,
}

impl Compiler<'_> {
    fn diagnostic(&mut self, owner: &SourceBinding, source: &str, message: String) {
        self.output.diagnostics.push(PreviewDiagnostic {
            owner: owner.key.clone(),
            source: source.to_owned(),
            message,
        });
    }

    fn owner(&mut self, owner: &SourceBinding) {
        let Some(node) = self.graph.node(&owner.source) else {
            return;
        };
        let Some(map) = self.graph.resolve(
            node.properties
                .get("InteractionMap")
                .or_else(|| node.properties.get("interactionmap"))
                .unwrap_or(&Value::Null),
        ) else {
            return;
        };
        let Some(table) = map.properties["node-table"].as_object() else {
            return;
        };
        let Some(tree) = self.graph.resolve(&map.properties["tree"]) else {
            return;
        };
        let mut visited = BTreeSet::new();
        for event_ref in array(&tree.properties["children"]) {
            let Some(event_id) = reference_id(event_ref) else {
                continue;
            };
            let Some(event_tree) = self.graph.node(event_id) else {
                continue;
            };
            let Some(event) = table
                .get(event_id)
                .and_then(|value| self.graph.resolve(value))
            else {
                continue;
            };
            if event.kind != "Axure:InteractionEvent" || disabled(event) {
                continue;
            }
            let mut cases = Vec::new();
            for case_ref in array(&event_tree.properties["children"]) {
                let Some(case_id) = reference_id(case_ref) else {
                    continue;
                };
                if !visited.insert(case_id.to_owned()) {
                    self.diagnostic(owner, case_id, "原生事件树重复引用了用例".into());
                    continue;
                }
                let Some(case_tree) = self.graph.node(case_id) else {
                    continue;
                };
                let Some(case) = table
                    .get(case_id)
                    .and_then(|value| self.graph.resolve(value))
                else {
                    continue;
                };
                if case.kind != "Axure:InteractionCase" || disabled(case) {
                    continue;
                }
                let mut actions = Vec::new();
                let guard =
                    reference_id(&case.properties["Condition"]).filter(|id| raw(id) != EMPTY);
                if guard.is_some() {
                    actions.push(PreviewAction::Unsupported {
                        kind: "condition".into(),
                        reason: "暂不执行带条件的原生用例".into(),
                    });
                }
                let mut action_ids = BTreeSet::new();
                for action_ref in array(&case_tree.properties["children"]) {
                    self.steps += 1;
                    if self.steps > 100_000 {
                        actions.push(PreviewAction::Unsupported {
                            kind: "limit".into(),
                            reason: "动作数量超过编译上限".into(),
                        });
                        break;
                    }
                    let Some(action_id) = reference_id(action_ref) else {
                        continue;
                    };
                    if !action_ids.insert(action_id) {
                        actions.push(PreviewAction::Unsupported {
                            kind: "cycle".into(),
                            reason: "动作树重复引用".into(),
                        });
                        continue;
                    }
                    let Some(action) = table
                        .get(action_id)
                        .and_then(|value| self.graph.resolve(value))
                    else {
                        actions.push(PreviewAction::Unsupported {
                            kind: "missing".into(),
                            reason: "动作引用不存在".into(),
                        });
                        continue;
                    };
                    if disabled(action) {
                        continue;
                    }
                    match self.action(owner, action) {
                        Ok(compiled) => actions.extend(compiled),
                        Err(reason) => actions.push(PreviewAction::Unsupported {
                            kind: action.kind.clone(),
                            reason,
                        }),
                    }
                }
                if actions.is_empty()
                    && !table
                        .get(case_id)
                        .and_then(reference_id)
                        .is_some_and(super::native_events::is_authored_case)
                {
                    continue;
                }
                let supported = !actions
                    .iter()
                    .any(|action| matches!(action, PreviewAction::Unsupported { .. }));
                if !supported {
                    for action in &actions {
                        if let PreviewAction::Unsupported { kind, reason } = action {
                            self.diagnostic(
                                owner,
                                case_id,
                                format!("{kind}: {reason}；此用例不执行"),
                            );
                        }
                    }
                }
                let name = case
                    .properties
                    .get("Name")
                    .or_else(|| case.properties.get("name"))
                    .or_else(|| case.properties.get("Description"))
                    .and_then(Value::as_str)
                    .unwrap_or("用例")
                    .to_owned();
                cases.push(PreviewCase {
                    source: case_id.to_owned(),
                    name,
                    new_if_group: boolean(&case.properties["IsNewIfGroup"]),
                    supported,
                    actions,
                });
            }
            if !cases.is_empty() {
                let event_type = integer(&event.properties["event-type"], -1);
                if !matches!(
                    event_type,
                    16 | 52 | 53 | 54 | 256 | 257 | 275 | 276 | 277 | 278 | 288 | 320
                ) {
                    self.diagnostic(owner, event_id, format!("暂不执行原生事件 {event_type}"));
                    for case in &mut cases {
                        case.supported = false;
                    }
                }
                self.output.events.push(PreviewEvent {
                    owner: owner.key.clone(),
                    event_type,
                    cases,
                });
            }
        }
    }

    fn targets(&mut self, owner: &SourceBinding, value: &Value) -> Result<Vec<String>, String> {
        let path = self
            .graph
            .resolve(value)
            .ok_or_else(|| "目标元件路径不存在".to_owned())?;
        if path.kind != "Axure:DiagramObjectPath" {
            return Err(format!("目标不是元件路径：{}", path.kind));
        }
        let p = &path.properties;
        if !array(&p["rdo-path"]).is_empty() || !array(&p["repeater-path"]).is_empty() {
            return Err("暂不执行带母版或Repeater显式路径的动作".into());
        }
        let object =
            reference_id(&p["object-id"]).ok_or_else(|| "元件路径没有对象标识".to_owned())?;
        if raw(object) == EMPTY {
            return Ok(vec![owner.key.clone()]);
        }
        if !self.graph.objects.keys().any(|id| raw(id) == raw(object)) {
            self.diagnostic(
                owner,
                object,
                format!("源元件目标不存在，已跳过：{}", raw(object)),
            );
            return Ok(Vec::new());
        }
        let mut candidates = self
            .bindings
            .iter()
            .filter(|binding| {
                (binding.source == object || raw(&binding.source) == raw(object))
                    && owner.scope.starts_with(&binding.scope)
            })
            .collect::<Vec<_>>();
        let depth = candidates
            .iter()
            .map(|binding| binding.scope.len())
            .max()
            .ok_or_else(|| format!("目标元件不在当前预览实例中：{}", raw(object)))?;
        candidates.retain(|binding| binding.scope.len() == depth);
        if candidates.len() > 1 {
            if object.contains('/') {
                let exact = candidates
                    .iter()
                    .copied()
                    .filter(|binding| binding.source == object)
                    .collect::<Vec<_>>();
                if !exact.is_empty() {
                    candidates = exact;
                }
            } else if let Some((package, _)) = owner.source.rsplit_once('/') {
                let local = candidates
                    .iter()
                    .copied()
                    .filter(|binding| {
                        binding
                            .source
                            .rsplit_once('/')
                            .is_some_and(|(candidate, _)| candidate == package)
                    })
                    .collect::<Vec<_>>();
                if !local.is_empty() {
                    candidates = local;
                }
            }
        }
        let keys = candidates
            .iter()
            .map(|binding| binding.key.clone())
            .collect::<BTreeSet<_>>();
        if keys.len() != 1 {
            return Err(format!("元件路径在当前实例中不唯一：{}", raw(object)));
        }
        Ok(keys.into_iter().collect())
    }

    fn pairs<'a>(
        &self,
        properties: &'a Value,
        path_field: &str,
        info_field: &str,
    ) -> Result<Vec<(&'a Value, &'a Value)>, String> {
        let paths = array(&properties[path_field]);
        let infos = array(&properties[info_field]);
        if paths.is_empty() || paths.len() != infos.len() {
            return Err(format!("{path_field}/{info_field}目标与参数数量不一致"));
        }
        Ok(paths.iter().zip(infos).collect())
    }

    fn page(&mut self, owner: &SourceBinding, value: &Value) -> Result<PageTarget, String> {
        let properties = self
            .graph
            .resolve(value)
            .map_or(value, |node| &node.properties);
        if integer(&properties["target-type"], 1) != 1 {
            return Err("暂不执行非内部页面目标".into());
        }
        let target = compile_page_target(self.graph, value, self.page_keys);
        if target.key.is_none()
            && (!target.literal.is_empty()
                || (!target.native_id.is_empty() && raw(&target.native_id) != EMPTY))
        {
            self.diagnostic(
                owner,
                &target.native_id,
                format!("源链接目标不在当前工程中：{}", target.literal),
            );
        }
        Ok(target)
    }

    fn action(&mut self, owner: &SourceBinding, node: &Node) -> Result<Vec<PreviewAction>, String> {
        let p = &node.properties;
        Ok(vec![match node.kind.as_str() {
            "Axure:InteractionLinkWindowAction" => {
                let mode = integer(&p["link-type"], 1);
                if !matches!(mode, 1 | 2 | 3 | 4 | 6) {
                    return Err(format!("暂不执行打开链接模式 {mode}"));
                }
                PreviewAction::LinkWindow {
                    page: self.page(owner, &p["target"])?,
                    mode,
                }
            }
            "Axure:InteractionLinkFrameAction" => {
                if integer(&p["link-type"], 5) != 5 {
                    return Err("暂不执行非框架模式的内联链接动作".into());
                }
                let values = array(&p["framepathstotargets"]);
                if values.is_empty() || !values.len().is_multiple_of(2) {
                    return Err("内联框架目标列表不是路径和页面的配对序列".into());
                }
                let mut frames = Vec::new();
                for pair in values.chunks_exact(2) {
                    frames.push(FrameLink {
                        targets: self.targets(owner, &pair[0])?,
                        page: self.page(owner, &pair[1])?,
                    });
                }
                PreviewAction::LinkFrame { frames }
            }
            "Axure:InteractionFadeWidgetAction" => {
                let mut entries = Vec::new();
                for (path, info) in self.pairs(p, "WidgetPaths", "FadeInfos")? {
                    let info = &self
                        .graph
                        .resolve(info)
                        .ok_or_else(|| "显示隐藏参数不存在".to_owned())?
                        .properties;
                    if !matches!(integer(&info["fade-action-type"], 0), 0..=2)
                        || !matches!(integer(&info["show-type"], 0), 0 | 2)
                    {
                        return Err("暂不执行该显示隐藏或弹出模式".into());
                    }
                    entries.push(FadeTarget {
                        targets: self.targets(owner, path)?,
                        action: integer(&info["fade-action-type"], 0),
                        easing: integer(&info["easing"], 0),
                        duration: number(&info["duration"], 0.0),
                        hide_easing: integer(&info["EasingHide"], 0),
                        hide_duration: number(&info["DurationHide"], 0.0),
                        show_type: integer(&info["show-type"], 0),
                        bring_to_front: boolean(&info["BringToFront"]),
                        vertical: boolean(&info["vertical"]),
                        compress_easing: integer(&info["compress-easing"], 0),
                        compress_duration: number(&info["compress-duration"], 0.0),
                        background: color(&info["page-background"]),
                        lightbox_close: true,
                    });
                }
                PreviewAction::Fade { entries }
            }
            "Axure:InteractionMoveWidgetAction" => {
                let mut entries = Vec::new();
                for (path, info) in self.pairs(p, "WidgetPaths", "MoveInfos")? {
                    let info = &self
                        .graph
                        .resolve(info)
                        .ok_or_else(|| "移动参数不存在".to_owned())?
                        .properties;
                    let mode = integer(&info["MoveType"], 0);
                    if !matches!(mode, 0 | 1 | 6) {
                        return Err(format!("暂不执行移动模式 {mode}"));
                    }
                    if !matches!(integer(&info["Easing"], 0), 0 | 2) {
                        return Err("移动缓动暂支持无动画与线性".into());
                    }
                    entries.push(MoveTarget {
                        targets: self.targets(owner, path)?,
                        mode,
                        x: numeric(self.graph, &info["XValue"])?,
                        y: numeric(self.graph, &info["YValue"])?,
                        easing: integer(&info["Easing"], 0),
                        duration: number(&info["Duration"], 0.0),
                    });
                }
                PreviewAction::Move { entries }
            }
            "Axure:InteractionSetPanelStateAction" => {
                let mut entries = Vec::new();
                for (path, info) in self.pairs(p, "PanelPaths", "SetStateInfos")? {
                    let info = &self
                        .graph
                        .resolve(info)
                        .ok_or_else(|| "面板状态参数不存在".to_owned())?
                        .properties;
                    if !matches!(integer(&info["set-state-type"], 0), 0..=4)
                        || boolean(&info["repeat"])
                        || boolean(&info["compress"])
                    {
                        return Err("暂不执行该面板状态模式、重复或推拉效果".into());
                    }
                    let state_key = reference_id(&info["diagram-id"])
                        .filter(|id| raw(id) != EMPTY)
                        .map(|id| raw(id).to_owned());
                    let value = literal(self.graph, &info["state-value"], 0)?;
                    entries.push(StateTarget {
                        targets: self.targets(owner, path)?,
                        mode: integer(&info["set-state-type"], 0),
                        state_key,
                        value,
                        looping: boolean(&info["loop"]),
                        show_when_set: boolean(&info["show-when-set"]),
                        exit_easing: integer(&info["exit-easing"], 0),
                        exit_duration: number(&info["exit-duration"], 0.0),
                        enter_easing: integer(&info["enter-easing"], 0),
                        enter_duration: number(&info["enter-duration"], 0.0),
                        repeat: boolean(&info["repeat"]),
                        repeat_interval: number(&info["repeat-interval"], 1000.0),
                        repeat_skip_first: boolean(&info["repeat-skip-first"]),
                        compress: boolean(&info["compress"]),
                        compress_easing: integer(&info["compress-easing"], 0),
                        compress_duration: number(&info["compress-duration"], 0.0),
                        vertical: boolean(&info["vertical"]),
                    });
                }
                PreviewAction::SetPanelState { entries }
            }
            "Axure:InteractionSetPanelOrderAction" => {
                let mut entries = Vec::new();
                for (path, info) in self.pairs(p, "PanelPaths", "SetOrderInfos")? {
                    let info = &self
                        .graph
                        .resolve(info)
                        .ok_or_else(|| "面板层级参数不存在".to_owned())?
                        .properties;
                    entries.push(OrderTarget {
                        targets: self.targets(owner, path)?,
                        front: boolean(&info["bring-to-front"]),
                    });
                }
                PreviewAction::SetPanelOrder { entries }
            }
            "Axure:InteractionScrollToWidgetAction" => {
                if !matches!(integer(&p["easing"], 0), 0 | 2) {
                    return Err("滚动缓动暂支持无动画与线性".into());
                }
                PreviewAction::ScrollTo {
                    targets: self.targets(owner, &p["targetwidgetpath"])?,
                    direction: integer(&p["scrolldirection"], 0),
                    easing: integer(&p["easing"], 0),
                    duration: number(&p["duration"], 0.0),
                }
            }
            "Axure:InterationActionSetFunction" => return self.functions(owner, &p["action"], 0),
            other => return Err(format!("暂不执行原生动作 {other}")),
        }])
    }

    fn functions(
        &mut self,
        owner: &SourceBinding,
        value: &Value,
        depth: usize,
    ) -> Result<Vec<PreviewAction>, String> {
        if depth > 32 {
            return Err("函数表达式嵌套超过上限".into());
        }
        let node = self
            .graph
            .resolve(value)
            .ok_or_else(|| "函数表达式不存在".to_owned())?;
        if node.kind == "Axure:Expr:Block" {
            let mut actions = Vec::new();
            for expression in array(&node.properties["subexpressions"]) {
                actions.extend(self.functions(owner, expression, depth + 1)?);
            }
            return Ok(actions);
        }
        if node.kind != "Axure:Expr:FCall" {
            return Err(format!("暂不执行函数 {}", node.kind));
        }
        let args = array(&node.properties["args"]);
        let function = node.properties["function-name"].as_str().unwrap_or("");
        let expected = match function {
            "SetCheckState" | "SetWidgetFormText" => 2,
            "SetWidgetRichText" => 3,
            _ => return Err(format!("暂不执行函数 {function}")),
        };
        if args.len() != expected {
            return Err(format!("{function}参数数量不正确"));
        }
        let path = self
            .graph
            .resolve(&args[0])
            .filter(|node| node.kind == "Axure:Expr:DiagramObjectPathLiteral")
            .ok_or_else(|| "SetCheckState目标不是元件路径常量".to_owned())?;
        let targets = self.targets(owner, &path.properties["value"])?;
        if function != "SetCheckState" {
            if function == "SetWidgetRichText" {
                let plain = self
                    .graph
                    .resolve(&args[2])
                    .filter(|node| node.kind == "Axure:Expr:BooleanLiteral")
                    .and_then(|node| node.properties["value"].as_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("true"));
                if !plain {
                    return Err("暂不执行富文本HTML表达式，仅支持纯文字".into());
                }
            }
            return Ok(vec![PreviewAction::SetText {
                targets,
                value: literal(self.graph, &args[1], 0)?,
            }]);
        }
        let value = match literal(self.graph, &args[1], 0)?.trim() {
            "true" => true,
            "false" => false,
            _ => return Err("SetCheckState值不是布尔常量".into()),
        };
        Ok(vec![PreviewAction::SetChecked { targets, value }])
    }
}

#[cfg(test)]
#[path = "preview_program_tests.rs"]
mod tests;
