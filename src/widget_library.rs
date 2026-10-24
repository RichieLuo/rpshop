//! Built-in, independently authored widget presets and reusable groups.
use crate::model::{Project, TextLayout, Widget, WidgetKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Basic,
    Forms,
    Markup,
    Flow,
    Icons,
}
impl Category {
    pub const ALL: [Self; 5] = [
        Self::Basic,
        Self::Forms,
        Self::Markup,
        Self::Flow,
        Self::Icons,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Basic => "基本元件",
            Self::Forms => "表单与菜单",
            Self::Markup => "标注",
            Self::Flow => "流程图",
            Self::Icons => "常用图标",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Preset {
    Kind(WidgetKind),
    Heading(u8),
    Paragraph,
    Link,
    OutlineButton,
    Rounded,
    Box(u8),
    Sticky(u8),
    Marker(bool),
    VerticalLine,
    Placeholder,
    Hotspot,
    Shape(&'static str),
    Table,
    Menu(bool),
    Tree,
    Icon(&'static str),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LibraryItem {
    pub id: &'static str,
    pub label: &'static str,
    pub category: Category,
    pub keywords: &'static str,
    preset: Preset,
}
macro_rules! item {
    ($id:literal,$label:literal,$category:ident,$preset:expr,$keys:literal) => {
        LibraryItem {
            id: $id,
            label: $label,
            category: Category::$category,
            preset: $preset,
            keywords: $keys,
        }
    };
}
pub const ITEMS: &[LibraryItem] = &[
    item!(
        "rectangle",
        "矩形",
        Basic,
        Preset::Kind(WidgetKind::Rectangle),
        "box rectangle"
    ),
    item!(
        "ellipse",
        "椭圆",
        Basic,
        Preset::Kind(WidgetKind::Ellipse),
        "circle ellipse 圆形"
    ),
    item!(
        "text",
        "文本",
        Basic,
        Preset::Kind(WidgetKind::Text),
        "label text 文字"
    ),
    item!(
        "button",
        "按钮",
        Basic,
        Preset::Kind(WidgetKind::Button),
        "primary button 主要按钮"
    ),
    item!(
        "heading1",
        "一级标题",
        Basic,
        Preset::Heading(1),
        "h1 heading title"
    ),
    item!(
        "heading2",
        "二级标题",
        Basic,
        Preset::Heading(2),
        "h2 heading title"
    ),
    item!(
        "heading3",
        "三级标题",
        Basic,
        Preset::Heading(3),
        "h3 heading title"
    ),
    item!(
        "paragraph",
        "段落",
        Basic,
        Preset::Paragraph,
        "paragraph 段落文本"
    ),
    item!("link", "文本链接", Basic, Preset::Link, "link 链接"),
    item!(
        "outline-button",
        "次要按钮",
        Basic,
        Preset::OutlineButton,
        "secondary outline button"
    ),
    item!("rounded", "圆角矩形", Basic, Preset::Rounded, "rounded box"),
    item!("box2", "矩形 2", Basic, Preset::Box(2), "box2 box 2"),
    item!("box3", "矩形 3", Basic, Preset::Box(3), "box3 box 3"),
    item!(
        "line",
        "直线",
        Basic,
        Preset::Kind(WidgetKind::Line),
        "line 分隔线"
    ),
    item!(
        "vertical-line",
        "垂直线",
        Basic,
        Preset::VerticalLine,
        "vertical line 竖线"
    ),
    item!(
        "image",
        "图片",
        Basic,
        Preset::Kind(WidgetKind::Image),
        "image picture 上传"
    ),
    item!(
        "placeholder",
        "占位符",
        Basic,
        Preset::Placeholder,
        "placeholder 占位图"
    ),
    item!(
        "hotspot",
        "热区",
        Basic,
        Preset::Hotspot,
        "hot spot hotspot 点击区域"
    ),
    item!(
        "triangle",
        "三角形",
        Basic,
        Preset::Shape("triangle"),
        "triangle"
    ),
    item!(
        "diamond",
        "菱形",
        Basic,
        Preset::Shape("diamond"),
        "diamond"
    ),
    item!(
        "pentagon",
        "五边形",
        Basic,
        Preset::Shape("pentagon"),
        "pentagon"
    ),
    item!(
        "hexagon",
        "六边形",
        Basic,
        Preset::Shape("hexagon"),
        "hexagon"
    ),
    item!(
        "octagon",
        "八边形",
        Basic,
        Preset::Shape("octagon"),
        "octagon"
    ),
    item!("star", "星形", Basic, Preset::Shape("star"), "star 五角星"),
    item!(
        "arrow-right",
        "右箭头",
        Basic,
        Preset::Shape("arrow"),
        "right arrow"
    ),
    item!(
        "arrow-left",
        "左箭头",
        Basic,
        Preset::Shape("left-arrow"),
        "left arrow"
    ),
    item!(
        "arrow-double",
        "双向箭头",
        Basic,
        Preset::Shape("double-arrow"),
        "double arrow"
    ),
    item!(
        "arrow-up",
        "垂直箭头",
        Basic,
        Preset::Shape("up-arrow"),
        "vertical arrow 上箭头"
    ),
    item!(
        "container",
        "容器",
        Basic,
        Preset::Kind(WidgetKind::Panel),
        "panel container 基础容器"
    ),
    item!(
        "input",
        "输入框",
        Forms,
        Preset::Kind(WidgetKind::Input),
        "text field input 文本框"
    ),
    item!(
        "textarea",
        "文本域",
        Forms,
        Preset::Kind(WidgetKind::TextArea),
        "textarea text area 多行输入框"
    ),
    item!(
        "checkbox",
        "复选框",
        Forms,
        Preset::Kind(WidgetKind::Checkbox),
        "checkbox check box"
    ),
    item!(
        "radio",
        "单选按钮",
        Forms,
        Preset::Kind(WidgetKind::Radio),
        "radio button 单选框"
    ),
    item!(
        "droplist",
        "下拉列表",
        Forms,
        Preset::Kind(WidgetKind::Droplist),
        "select dropdown droplist 下拉框"
    ),
    item!(
        "listbox",
        "列表框",
        Forms,
        Preset::Kind(WidgetKind::ListBox),
        "listbox list box"
    ),
    item!("table", "表格", Forms, Preset::Table, "table grid 数据表格"),
    item!(
        "horizontal-menu",
        "横向菜单",
        Forms,
        Preset::Menu(false),
        "horizontal menu 导航栏"
    ),
    item!(
        "vertical-menu",
        "纵向菜单",
        Forms,
        Preset::Menu(true),
        "vertical menu 菜单栏"
    ),
    item!("tree", "树", Forms, Preset::Tree, "tree 树形菜单 目录"),
    item!(
        "sticky1",
        "便签 1",
        Markup,
        Preset::Sticky(1),
        "sticky note 注释 标注"
    ),
    item!(
        "sticky2",
        "便签 2",
        Markup,
        Preset::Sticky(2),
        "sticky note 注释 标注"
    ),
    item!(
        "sticky3",
        "便签 3",
        Markup,
        Preset::Sticky(3),
        "sticky note 注释 标注"
    ),
    item!(
        "sticky4",
        "便签 4",
        Markup,
        Preset::Sticky(4),
        "sticky note 注释 标注"
    ),
    item!(
        "circle-marker",
        "圆形标记",
        Markup,
        Preset::Marker(false),
        "circle marker 序号"
    ),
    item!(
        "drop-marker",
        "水滴标记",
        Markup,
        Preset::Marker(true),
        "drop marker 序号"
    ),
    item!(
        "flow-process",
        "流程",
        Flow,
        Preset::Shape("process"),
        "flow process 过程"
    ),
    item!(
        "flow-decision",
        "判断",
        Flow,
        Preset::Shape("decision"),
        "flow decision 条件"
    ),
    item!(
        "flow-start",
        "开始/结束",
        Flow,
        Preset::Shape("start"),
        "flow terminator start end"
    ),
    item!(
        "flow-data",
        "数据",
        Flow,
        Preset::Shape("data"),
        "flow data 输入输出 平行四边形"
    ),
    item!(
        "flow-document",
        "文档",
        Flow,
        Preset::Shape("document"),
        "flow document"
    ),
    item!(
        "flow-database",
        "数据库",
        Flow,
        Preset::Shape("database"),
        "flow database cylinder 圆柱"
    ),
    item!(
        "flow-predefined",
        "子流程",
        Flow,
        Preset::Shape("predefined"),
        "flow predefined subprocess"
    ),
    item!(
        "flow-manual",
        "手动输入",
        Flow,
        Preset::Shape("manual"),
        "flow manual input"
    ),
    item!(
        "flow-preparation",
        "准备",
        Flow,
        Preset::Shape("preparation"),
        "flow preparation"
    ),
    item!(
        "flow-connector",
        "连接点",
        Flow,
        Preset::Shape("connector"),
        "flow connector 圆"
    ),
    item!(
        "flow-actor",
        "人物",
        Flow,
        Preset::Shape("actor"),
        "flow actor"
    ),
    item!(
        "flow-beveled",
        "斜角矩形",
        Flow,
        Preset::Shape("beveled"),
        "flow beveled rectangle"
    ),
    item!(
        "flow-bracket",
        "括号",
        Flow,
        Preset::Shape("bracket"),
        "flow bracket"
    ),
    item!(
        "flow-collate",
        "校对",
        Flow,
        Preset::Shape("collate"),
        "flow collate"
    ),
    item!(
        "flow-delay",
        "延迟",
        Flow,
        Preset::Shape("delay"),
        "flow delay"
    ),
    item!(
        "flow-display",
        "显示",
        Flow,
        Preset::Shape("display"),
        "flow display"
    ),
    item!(
        "flow-file",
        "文件",
        Flow,
        Preset::Shape("file"),
        "flow file"
    ),
    item!(
        "flow-ram",
        "内部存储",
        Flow,
        Preset::Shape("ram"),
        "flow internal storage ram"
    ),
    item!(
        "flow-keying",
        "键控",
        Flow,
        Preset::Shape("keying"),
        "flow keying"
    ),
    item!(
        "flow-or",
        "逻辑或",
        Flow,
        Preset::Shape("or"),
        "flow logical or"
    ),
    item!(
        "flow-tape",
        "磁带",
        Flow,
        Preset::Shape("tape"),
        "flow magnetic tape"
    ),
    item!(
        "flow-multidoc",
        "多文档",
        Flow,
        Preset::Shape("multidoc"),
        "flow multi document"
    ),
    item!(
        "flow-offpage",
        "离页连接",
        Flow,
        Preset::Shape("offpage"),
        "flow off page connector"
    ),
    item!(
        "flow-offline",
        "离线存储",
        Flow,
        Preset::Shape("offline"),
        "flow offline storage"
    ),
    item!(
        "flow-card",
        "穿孔卡",
        Flow,
        Preset::Shape("card"),
        "flow punched card"
    ),
    item!(
        "flow-punched",
        "穿孔带",
        Flow,
        Preset::Shape("punched"),
        "flow punched tape"
    ),
    item!(
        "flow-semicircle",
        "半圆",
        Flow,
        Preset::Shape("semicircle"),
        "flow semicircle"
    ),
    item!(
        "flow-sort",
        "排序",
        Flow,
        Preset::Shape("sort"),
        "flow sort"
    ),
    item!(
        "flow-stack-file",
        "堆叠文件",
        Flow,
        Preset::Shape("stack-file"),
        "flow stacked file"
    ),
    item!(
        "flow-stack-rect",
        "堆叠矩形",
        Flow,
        Preset::Shape("stack-rect"),
        "flow stacked rectangle"
    ),
    item!(
        "flow-stack-rounded",
        "堆叠圆角",
        Flow,
        Preset::Shape("stack-rounded"),
        "flow stacked rounded"
    ),
    item!(
        "flow-stored",
        "存储数据",
        Flow,
        Preset::Shape("stored"),
        "flow stored data"
    ),
    item!(
        "flow-sum",
        "求和连接",
        Flow,
        Preset::Shape("sum"),
        "flow summing junction"
    ),
    item!(
        "flow-trapezoid",
        "梯形",
        Flow,
        Preset::Shape("trapezoid"),
        "flow trapezoid"
    ),
    item!(
        "icon-search",
        "搜索",
        Icons,
        Preset::Icon("search"),
        "search find 搜索 放大镜"
    ),
    item!(
        "icon-close",
        "关闭",
        Icons,
        Preset::Icon("close"),
        "close x 删除"
    ),
    item!(
        "icon-menu",
        "菜单",
        Icons,
        Preset::Icon("menu"),
        "menu hamburger"
    ),
    item!(
        "icon-home",
        "首页",
        Icons,
        Preset::Icon("home"),
        "home house"
    ),
    item!(
        "icon-user",
        "用户",
        Icons,
        Preset::Icon("user"),
        "user person 人像"
    ),
    item!(
        "icon-check",
        "勾选",
        Icons,
        Preset::Icon("check"),
        "check tick 对勾"
    ),
    item!(
        "icon-plus",
        "添加",
        Icons,
        Preset::Icon("plus"),
        "add plus 加号"
    ),
    item!(
        "icon-chevron",
        "下一步",
        Icons,
        Preset::Icon("chevron"),
        "chevron next 向右"
    ),
];

impl LibraryItem {
    pub fn matches(self, query: &str) -> bool {
        let haystack = format!("{} {} {}", self.label, self.keywords, self.id).to_lowercase();
        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
    pub fn kind(self) -> WidgetKind {
        match self.preset {
            Preset::Kind(kind) => kind,
            Preset::Heading(_) | Preset::Paragraph | Preset::Link => WidgetKind::Text,
            _ => WidgetKind::Rectangle,
        }
    }
    pub fn create(self, project: &mut Project, x: f32, y: f32) -> Vec<Widget> {
        let mut widget = Widget::new(project.allocate_id(), self.kind(), x, y);
        widget.name = format!("{} {}", self.label, widget.id);
        match self.preset {
            Preset::Kind(_) => {}
            Preset::Heading(level) => {
                widget.font_size = match level {
                    1 => 32.0,
                    2 => 24.0,
                    _ => 18.0,
                };
                widget.width = 300.0;
                widget.height = widget.font_size * 1.6;
                widget.text = self.label.into();
                widget.text_layout = Some(TextLayout {
                    horizontal: 0,
                    vertical: 1,
                    padding: [0.0; 4],
                    wrap: false,
                    bold: true,
                });
            }
            Preset::Paragraph => {
                widget.width = 300.0;
                widget.height = 100.0;
                widget.text = "在这里输入段落内容。\n用清晰的文字描述你的设计。".into();
                widget.text_layout = Some(TextLayout {
                    horizontal: 0,
                    vertical: 0,
                    padding: [0.0; 4],
                    wrap: true,
                    bold: false,
                });
            }
            Preset::Link => {
                widget.text = "文本链接".into();
                widget.text_color = "#6759E8".into();
                widget.width = 120.0;
            }
            Preset::OutlineButton => {
                widget.width = 140.0;
                widget.height = 44.0;
                widget.text = "按钮".into();
                widget.fill = "#FFFFFF".into();
                widget.stroke = "#6759E8".into();
                widget.text_color = "#6759E8".into();
                widget.radius = 6.0;
            }
            Preset::Rounded => {
                widget.radius = 24.0;
            }
            Preset::Box(style) => {
                widget.radius = 0.0;
                widget.fill = if style == 2 { "#F0EDFF" } else { "#E8EAF0" }.into();
                widget.stroke = if style == 2 { "#AFA5E8" } else { "#FFFFFF00" }.into();
            }
            Preset::Sticky(style) => {
                widget.width = 200.0;
                widget.height = 130.0;
                widget.radius = 0.0;
                widget.fill = match style {
                    1 => "#FFF3B0",
                    2 => "#E3F3D4",
                    3 => "#DBEDFF",
                    _ => "#FFE0EB",
                }
                .into();
                widget.stroke = "#FFFFFF00".into();
                widget.text = "在这里添加说明".into();
                widget.text_layout = Some(TextLayout {
                    horizontal: 0,
                    vertical: 0,
                    padding: [16.0; 4],
                    wrap: true,
                    bold: false,
                });
            }
            Preset::Marker(drop) => {
                widget.width = 40.0;
                widget.height = if drop { 52.0 } else { 40.0 };
                widget.text = "1".into();
                widget.fill = "#6759E8".into();
                widget.stroke = "#FFFFFF00".into();
                widget.text_color = "#FFFFFF".into();
                if drop {
                    widget.vector_points = Some(crate::shape_geometry::polygon(&[
                        [0.5, 1.0],
                        [0.05, 0.45],
                        [0.02, 0.25],
                        [0.15, 0.08],
                        [0.5, 0.0],
                        [0.85, 0.08],
                        [0.98, 0.25],
                        [0.95, 0.45],
                    ]));
                } else {
                    widget.kind = WidgetKind::Ellipse;
                }
            }
            Preset::VerticalLine => {
                widget.kind = WidgetKind::Line;
                widget.width = 2.0;
                widget.height = 160.0;
                widget.fill = "#A9AFBE".into();
                widget.vector_points = Some(paths(&[vec![[0.5, 0.0], [0.5, 1.0]]], &[false]));
            }
            Preset::Placeholder => {
                widget.width = 240.0;
                widget.height = 160.0;
                widget.fill = "#F3F4F8".into();
                widget.text = "图片占位符".into();
                widget.vector_points = Some(paths(
                    &[
                        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                        vec![[0.0, 0.0], [1.0, 1.0]],
                        vec![[1.0, 0.0], [0.0, 1.0]],
                    ],
                    &[true, false, false],
                ));
            }
            Preset::Hotspot => {
                widget.fill = "#FFFFFF00".into();
                widget.stroke = "#FFFFFF00".into();
                widget.radius = 0.0;
            }
            Preset::Shape(name) => {
                widget.width = 160.0;
                widget.height = 100.0;
                widget.radius = 0.0;
                widget.text = if self.category == Category::Flow {
                    self.label.into()
                } else {
                    String::new()
                };
                if name == "start" {
                    widget.radius = 50.0;
                    widget.height = 60.0;
                } else if name == "connector" {
                    widget.kind = WidgetKind::Ellipse;
                    widget.width = 56.0;
                    widget.height = 56.0;
                    widget.text.clear();
                } else {
                    widget.vector_points = Some(shape(name));
                }
                if name == "up-arrow" {
                    widget.width = 100.0;
                    widget.height = 160.0;
                }
                if name == "actor" {
                    widget.width = 64.0;
                    widget.height = 100.0;
                    widget.text.clear();
                    widget.fill = "#FFFFFF00".into();
                }
                if name == "bracket" {
                    widget.width = 30.0;
                    widget.text.clear();
                }
            }
            Preset::Icon(name) => {
                widget.width = 32.0;
                widget.height = 32.0;
                widget.fill = "#FFFFFF00".into();
                widget.stroke = "#243047".into();
                widget.radius = 0.0;
                widget.vector_points = Some(icon(name));
            }
            Preset::Table | Preset::Menu(_) | Preset::Tree => {
                return self.composite(project, widget);
            }
        }
        vec![widget]
    }
    fn composite(self, project: &mut Project, frame: Widget) -> Vec<Widget> {
        let group = frame.id;
        let mut widgets = Vec::new();
        let (rows, columns) = match self.preset {
            Preset::Table => (4, 3),
            Preset::Menu(true) => (4, 1),
            Preset::Menu(false) => (1, 4),
            _ => (5, 1),
        };
        for row in 0..rows {
            for col in 0..columns {
                let tree = matches!(self.preset, Preset::Tree);
                let mut w = Widget::new(
                    project.allocate_id(),
                    if tree {
                        WidgetKind::Text
                    } else {
                        WidgetKind::Rectangle
                    },
                    frame.x + col as f32 * 100.0,
                    frame.y + row as f32 * 36.0,
                );
                w.width = if tree { 220.0 } else { 100.0 };
                w.height = 36.0;
                w.radius = 0.0;
                w.groups.push(group);
                w.name = format!("{} {}-{}", self.label, row + 1, col + 1);
                w.text = if tree {
                    match row {
                        0 => "▾ 目录",
                        1 => "    页面一",
                        2 => "    页面二",
                        3 => "▾ 文件夹",
                        _ => "    子页面",
                    }
                    .into()
                } else if matches!(self.preset, Preset::Table) {
                    if row == 0 {
                        format!("列 {}", col + 1)
                    } else {
                        format!("单元格 {}-{}", row, col + 1)
                    }
                } else {
                    format!("菜单 {}", row + col + 1)
                };
                if row == 0 && !tree {
                    w.fill = "#F0EDFF".into();
                }
                widgets.push(w);
            }
        }
        widgets
    }
}

fn paths(contours: &[Vec<[f32; 2]>], closed: &[bool]) -> serde_json::Value {
    serde_json::Value::Array(contours.iter().zip(closed).map(|(c,closed)| serde_json::Value::Array(c.iter().enumerate().map(|(i,p)|serde_json::json!({"Path":{"X":p[0],"Y":p[1]},"HigherSegmentExists":*closed||i+1<c.len()})).collect())).collect())
}
fn regular(n: usize, inner: Option<f32>) -> Vec<[f32; 2]> {
    let count = if inner.is_some() { n * 2 } else { n };
    (0..count)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / count as f32 - std::f32::consts::FRAC_PI_2;
            let r = if i % 2 == 1 {
                inner.unwrap_or(0.5)
            } else {
                0.5
            };
            [0.5 + a.cos() * r, 0.5 + a.sin() * r]
        })
        .collect()
}
fn shape(name: &str) -> serde_json::Value {
    let rect = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let arc = |cx: f32, cy: f32, rx: f32, ry: f32, start: f32, end: f32| {
        (0..=24)
            .map(|i| {
                let a = start + (end - start) * i as f32 / 24.0;
                [cx + rx * a.cos(), cy + ry * a.sin()]
            })
            .collect::<Vec<_>>()
    };
    let pi = std::f32::consts::PI;
    let points = match name {
        "triangle" => vec![[0.5, 0.0], [1.0, 1.0], [0.0, 1.0]],
        "diamond" | "decision" => vec![[0.5, 0.0], [1.0, 0.5], [0.5, 1.0], [0.0, 0.5]],
        "pentagon" => regular(5, None),
        "hexagon" => regular(6, None),
        "octagon" => regular(8, None),
        "star" => regular(5, Some(0.22)),
        "arrow" | "left-arrow" | "up-arrow" => {
            let mut p = vec![
                [0.0, 0.25],
                [0.62, 0.25],
                [0.62, 0.0],
                [1.0, 0.5],
                [0.62, 1.0],
                [0.62, 0.75],
                [0.0, 0.75],
            ];
            if name == "left-arrow" {
                for v in &mut p {
                    v[0] = 1.0 - v[0];
                }
            }
            if name == "up-arrow" {
                for v in &mut p {
                    *v = [v[1], 1.0 - v[0]];
                }
            }
            p
        }
        "double-arrow" => vec![
            [0.0, 0.5],
            [0.3, 0.0],
            [0.3, 0.25],
            [0.7, 0.25],
            [0.7, 0.0],
            [1.0, 0.5],
            [0.7, 1.0],
            [0.7, 0.75],
            [0.3, 0.75],
            [0.3, 1.0],
        ],
        "data" => vec![[0.2, 0.0], [1.0, 0.0], [0.8, 1.0], [0.0, 1.0]],
        "manual" => vec![[0.0, 0.2], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        "preparation" => vec![
            [0.2, 0.0],
            [0.8, 0.0],
            [1.0, 0.5],
            [0.8, 1.0],
            [0.2, 1.0],
            [0.0, 0.5],
        ],
        "beveled" => vec![
            [0.1, 0.0],
            [0.9, 0.0],
            [1.0, 0.15],
            [1.0, 0.85],
            [0.9, 1.0],
            [0.1, 1.0],
            [0.0, 0.85],
            [0.0, 0.15],
        ],
        "offpage" => vec![[0.0, 0.0], [1.0, 0.0], [1.0, 0.65], [0.5, 1.0], [0.0, 0.65]],
        "offline" => vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]],
        "card" => vec![[0.15, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.2]],
        "keying" => vec![[0.0, 0.0], [1.0, 0.0], [0.8, 1.0], [0.2, 1.0]],
        "trapezoid" => vec![[0.2, 0.0], [0.8, 0.0], [1.0, 1.0], [0.0, 1.0]],
        "bracket" => {
            return paths(
                &[vec![[1.0, 0.0], [0.0, 0.0], [0.0, 1.0], [1.0, 1.0]]],
                &[false],
            );
        }
        "collate" => {
            return paths(
                &[
                    vec![[0.0, 0.0], [1.0, 0.0], [0.5, 0.5]],
                    vec![[0.5, 0.5], [1.0, 1.0], [0.0, 1.0]],
                ],
                &[true, true],
            );
        }
        "delay" => {
            let mut p = vec![[0.0, 0.0]];
            p.extend(arc(0.5, 0.5, 0.5, 0.5, -pi / 2.0, pi / 2.0));
            p.push([0.0, 1.0]);
            p
        }
        "display" => {
            let mut p = vec![[0.0, 0.5], [0.2, 0.0]];
            p.extend(arc(0.75, 0.5, 0.25, 0.5, -pi / 2.0, pi / 2.0));
            p.push([0.2, 1.0]);
            p
        }
        "semicircle" => arc(0.5, 1.0, 0.5, 1.0, pi, 2.0 * pi),
        "file" => {
            return paths(
                &[
                    vec![[0.0, 0.0], [0.75, 0.0], [1.0, 0.25], [1.0, 1.0], [0.0, 1.0]],
                    vec![[0.75, 0.0], [0.75, 0.25], [1.0, 0.25]],
                ],
                &[true, false],
            );
        }
        "ram" => {
            return paths(
                &[
                    rect,
                    vec![[0.12, 0.0], [0.12, 1.0]],
                    vec![[0.0, 0.18], [1.0, 0.18]],
                ],
                &[true, false, false],
            );
        }
        "sort" => {
            return paths(
                &[
                    vec![[0.5, 0.0], [1.0, 0.5], [0.5, 1.0], [0.0, 0.5]],
                    vec![[0.0, 0.5], [1.0, 0.5]],
                ],
                &[true, false],
            );
        }
        "or" | "sum" => {
            let circle = arc(0.5, 0.5, 0.5, 0.5, 0.0, 2.0 * pi);
            let lines = if name == "or" {
                vec![
                    vec![[0.15, 0.15], [0.85, 0.85]],
                    vec![[0.85, 0.15], [0.15, 0.85]],
                ]
            } else {
                vec![vec![[0.0, 0.5], [1.0, 0.5]], vec![[0.5, 0.0], [0.5, 1.0]]]
            };
            return paths(
                &[circle, lines[0].clone(), lines[1].clone()],
                &[false, false, false],
            );
        }
        "tape" => {
            return paths(
                &[
                    arc(0.45, 0.45, 0.45, 0.45, 0.0, 2.0 * pi),
                    vec![[0.45, 0.9], [1.0, 0.9]],
                ],
                &[false, false],
            );
        }
        "actor" => {
            return paths(
                &[
                    arc(0.5, 0.16, 0.18, 0.14, 0.0, 2.0 * pi),
                    vec![[0.5, 0.3], [0.5, 0.65]],
                    vec![[0.05, 0.4], [0.95, 0.4]],
                    vec![[0.1, 1.0], [0.5, 0.65], [0.9, 1.0]],
                ],
                &[false, false, false, false],
            );
        }
        "punched" => {
            let mut p = Vec::new();
            for i in 0..=20 {
                let x = i as f32 / 20.0;
                p.push([x, 0.1 + (x * 2.0 * pi).sin() * 0.08]);
            }
            for i in 0..=20 {
                let x = 1.0 - i as f32 / 20.0;
                p.push([x, 0.9 + (x * 2.0 * pi).sin() * 0.08]);
            }
            p
        }
        "stack-rect" | "stack-rounded" | "stack-file" | "multidoc" => {
            let mut front = if name == "multidoc" {
                let mut p = vec![[0.0, 0.15], [0.85, 0.15], [0.85, 0.85]];
                for i in 1..=20 {
                    let x = 0.85 - i as f32 / 20.0 * 0.85;
                    p.push([x, 0.85 + (x * 2.0 * pi / 0.85).sin() * 0.08]);
                }
                p
            } else if name == "stack-file" {
                vec![
                    [0.0, 0.15],
                    [0.65, 0.15],
                    [0.85, 0.35],
                    [0.85, 1.0],
                    [0.0, 1.0],
                ]
            } else {
                vec![[0.0, 0.15], [0.85, 0.15], [0.85, 1.0], [0.0, 1.0]]
            };
            if name == "stack-rounded" {
                front = vec![
                    [0.05, 0.15],
                    [0.8, 0.15],
                    [0.85, 0.2],
                    [0.85, 0.95],
                    [0.8, 1.0],
                    [0.05, 1.0],
                    [0.0, 0.95],
                    [0.0, 0.2],
                ];
            }
            return paths(
                &[
                    vec![
                        [0.15, 0.08],
                        [0.15, 0.0],
                        [1.0, 0.0],
                        [1.0, 0.85],
                        [0.93, 0.85],
                    ],
                    vec![
                        [0.07, 0.15],
                        [0.07, 0.08],
                        [0.93, 0.08],
                        [0.93, 0.93],
                        [0.85, 0.93],
                    ],
                    front,
                ],
                &[false, false, true],
            );
        }
        "stored" => {
            let mut p = arc(0.85, 0.5, 0.15, 0.5, -pi / 2.0, pi / 2.0);
            p.extend(arc(0.0, 0.5, 0.15, 0.5, pi / 2.0, -pi / 2.0));
            p
        }
        "document" => {
            let mut p = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 0.85]];
            for i in 1..=20 {
                let x = 1.0 - i as f32 / 20.0;
                p.push([x, 0.85 + (x * std::f32::consts::TAU).sin() * 0.1]);
            }
            p
        }
        "database" => {
            let mut p = Vec::new();
            for i in 0..=20 {
                let a = std::f32::consts::PI + i as f32 * std::f32::consts::PI / 20.0;
                p.push([0.5 + a.cos() * 0.5, 0.13 + a.sin() * 0.13]);
            }
            for i in 0..=20 {
                let a = i as f32 * std::f32::consts::PI / 20.0;
                p.push([0.5 + a.cos() * 0.5, 0.87 + a.sin() * 0.13]);
            }
            return paths(
                &[
                    p,
                    (0..=32)
                        .map(|i| {
                            let a = i as f32 * std::f32::consts::TAU / 32.0;
                            [0.5 + a.cos() * 0.5, 0.13 + a.sin() * 0.13]
                        })
                        .collect(),
                ],
                &[true, false],
            );
        }
        "predefined" => {
            return paths(
                &[
                    vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    vec![[0.12, 0.0], [0.12, 1.0]],
                    vec![[0.88, 0.0], [0.88, 1.0]],
                ],
                &[true, false, false],
            );
        }
        _ => vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    };
    crate::shape_geometry::polygon(&points)
}
fn icon(name: &str) -> serde_json::Value {
    let circle = |cx: f32, cy: f32, r: f32| {
        (0..=24)
            .map(|i| {
                let a = i as f32 * std::f32::consts::TAU / 24.0;
                [cx + a.cos() * r, cy + a.sin() * r]
            })
            .collect()
    };
    let lines = match name {
        "search" => vec![circle(0.4, 0.4, 0.28), vec![[0.6, 0.6], [0.92, 0.92]]],
        "close" => vec![vec![[0.2, 0.2], [0.8, 0.8]], vec![[0.8, 0.2], [0.2, 0.8]]],
        "menu" => vec![
            vec![[0.1, 0.25], [0.9, 0.25]],
            vec![[0.1, 0.5], [0.9, 0.5]],
            vec![[0.1, 0.75], [0.9, 0.75]],
        ],
        "home" => vec![
            vec![[0.05, 0.45], [0.5, 0.08], [0.95, 0.45]],
            vec![[0.2, 0.35], [0.2, 0.9], [0.8, 0.9], [0.8, 0.35]],
            vec![[0.4, 0.9], [0.4, 0.6], [0.6, 0.6], [0.6, 0.9]],
        ],
        "user" => vec![
            circle(0.5, 0.28, 0.2),
            vec![
                [0.1, 0.95],
                [0.16, 0.7],
                [0.3, 0.58],
                [0.7, 0.58],
                [0.84, 0.7],
                [0.9, 0.95],
            ],
        ],
        "check" => vec![vec![[0.1, 0.5], [0.38, 0.8], [0.9, 0.2]]],
        "plus" => vec![vec![[0.1, 0.5], [0.9, 0.5]], vec![[0.5, 0.1], [0.5, 0.9]]],
        _ => vec![vec![[0.3, 0.1], [0.7, 0.5], [0.3, 0.9]]],
    };
    let closed = vec![false; lines.len()];
    paths(&lines, &closed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_preset_roundtrips_and_groups_remain_individually_editable() {
        let mut p = Project::blank();
        for item in ITEMS {
            let widgets = item.create(&mut p, 10.0, 20.0);
            assert!(!widgets.is_empty());
            for w in &widgets {
                if let Some(points) = &w.vector_points {
                    crate::vector_edit::validate_points(points).unwrap();
                    assert!(crate::shape_geometry::svg_path(w).is_some());
                    for (contour, _) in crate::shape_geometry::contours(w) {
                        if contour.len() < 3 {
                            continue;
                        }
                        let area: f32 = contour
                            .iter()
                            .zip(contour.iter().cycle().skip(1))
                            .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
                            .sum();
                        if area.abs() > 0.00001 {
                            assert!(
                                !crate::shape_geometry::triangles(&contour).is_empty(),
                                "canvas fill failed: {}",
                                item.id
                            );
                        }
                    }
                }
            }
            p.pages[0].widgets.extend(widgets);
        }
        p.validate().unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let decoded: Project = serde_json::from_str(&json).unwrap();
        for (before, after) in p.pages[0].widgets.iter().zip(&decoded.pages[0].widgets) {
            assert!(before == after, "preset roundtrip changed: {}", before.name);
        }
        let table = ITEMS
            .iter()
            .find(|i| i.id == "table")
            .unwrap()
            .create(&mut p, 0.0, 0.0);
        assert_eq!(table.len(), 12);
        assert!(table.iter().all(|w| w.groups == table[0].groups));
    }
    #[test]
    fn search_matches_aliases_and_multiple_words() {
        assert!(
            ITEMS
                .iter()
                .find(|i| i.id == "textarea")
                .unwrap()
                .matches("多行")
        );
        assert!(
            ITEMS
                .iter()
                .find(|i| i.id == "flow-start")
                .unwrap()
                .matches("flow start")
        );
        assert!(!ITEMS[0].matches("repeater"));
    }
}
