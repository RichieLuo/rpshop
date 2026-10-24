use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FORMAT_VERSION: u32 = 1;
pub const MAX_GROUP_DEPTH: usize = 32;
const MAX_PROJECT_BYTES: u64 = 48 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub pages: Vec<Page>,
    /// Navigation order and folders. Empty retains the flat order of older files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page_tree: Vec<PageTreeNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interaction_events: Vec<AuthoredEvent>,
    pub next_id: u64,
}

/// Editable authored cases. Native documents keep every other source case intact.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AuthoredEvent {
    pub id: String,
    pub page: u64,
    pub owner: Option<u64>,
    pub trigger: EventTrigger,
    pub name: String,
    pub actions: Vec<Interaction>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventTrigger {
    PageLoad,
    Click,
}

impl EventTrigger {
    pub fn native_type(self) -> i64 {
        match self {
            Self::PageLoad => 320,
            Self::Click => 16,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::PageLoad => "页面载入时",
            Self::Click => "单击时",
        }
    }
}

impl AuthoredEvent {
    pub fn new(page: u64, owner: Option<u64>, trigger: EventTrigger) -> Self {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            ^ ((std::process::id() as u128) << 64)
            ^ SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128;
        Self {
            id: format!("72707368{:024x}", nonce & 0xffff_ffff_ffff_ffff_ffff_ffff),
            page,
            owner,
            trigger,
            name: "用例1".into(),
            actions: Vec::new(),
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PageTreeNode {
    Folder {
        id: String,
        name: String,
        #[serde(default)]
        children: Vec<PageTreeNode>,
    },
    Page {
        page_id: u64,
        #[serde(default)]
        children: Vec<PageTreeNode>,
    },
}

// RON's enum-tag reader does not roundtrip this internally tagged enum in
// desktop session storage. A typed record keeps the same JSON/RON output and
// reads the tag as a string without Serde's buffered enum representation.
impl<'de> Deserialize<'de> for PageTreeNode {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Record {
            #[serde(rename = "type")]
            kind: String,
            #[serde(default)]
            id: String,
            #[serde(default)]
            name: String,
            #[serde(default)]
            page_id: u64,
            #[serde(default)]
            children: Vec<PageTreeNode>,
        }
        let record = Record::deserialize(deserializer)?;
        match record.kind.as_str() {
            "folder" => Ok(Self::Folder {
                id: record.id,
                name: record.name,
                children: record.children,
            }),
            "page" => Ok(Self::Page {
                page_id: record.page_id,
                children: record.children,
            }),
            _ => Err(serde::de::Error::unknown_variant(
                &record.kind,
                &["folder", "page"],
            )),
        }
    }
}

impl PageTreeNode {
    pub fn children(&self) -> &[Self] {
        match self {
            Self::Folder { children, .. } | Self::Page { children, .. } => children,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Page {
    pub id: u64,
    pub name: String,
    pub width: f32,
    pub height: f32,
    /// Auto pages are an unbounded editor surface; width/height remain export bounds.
    #[serde(default)]
    pub canvas: CanvasMode,
    pub background: String,
    pub widgets: Vec<Widget>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CanvasMode {
    #[default]
    Fixed,
    Auto,
    FixedWidth,
    FixedHeight,
}

impl CanvasMode {
    pub fn is_auto(self) -> bool {
        self != Self::Fixed
    }
    pub fn auto_width(self) -> bool {
        matches!(self, Self::Auto | Self::FixedHeight)
    }
    pub fn auto_height(self) -> bool {
        matches!(self, Self::Auto | Self::FixedWidth)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TextLayout {
    /// Native alignment: 0 = start, 1 = center, 2 = end.
    pub horizontal: u8,
    pub vertical: u8,
    /// Left, top, right, bottom in document pixels.
    pub padding: [f32; 4],
    pub wrap: bool,
    pub bold: bool,
}

impl Page {
    /// Finite preview/export dimensions for otherwise unbounded editor pages.
    pub fn size_for_output(&self) -> [f32; 2] {
        let mut size = [self.width, self.height];
        if self.canvas.auto_width() {
            size[0] = 64.0;
        }
        if self.canvas.auto_height() {
            size[1] = 64.0;
        }
        for widget in self.widgets.iter().filter(|widget| !widget.hidden) {
            let bounds = crate::manipulation::rotated_bounds(widget);
            if self.canvas.auto_width() {
                size[0] = size[0].max(bounds[2]);
            }
            if self.canvas.auto_height() {
                size[1] = size[1].max(bounds[3]);
            }
        }
        size
    }
}

#[cfg(test)]
mod canvas_metadata_tests {
    use super::*;

    #[test]
    fn old_documents_default_to_fixed_and_auto_output_tracks_content() {
        let mut project = Project::blank();
        let mut raw = serde_json::to_value(&project).unwrap();
        raw["pages"][0].as_object_mut().unwrap().remove("canvas");
        let old: Project = serde_json::from_value(raw).unwrap();
        assert_eq!(old.pages[0].canvas, CanvasMode::Fixed);
        let id = project.allocate_id();
        project.pages[0]
            .widgets
            .push(Widget::new(id, WidgetKind::Rectangle, 1600.0, 1200.0));
        project.pages[0].canvas = CanvasMode::Auto;
        assert_eq!(project.pages[0].size_for_output(), [1780.0, 1300.0]);
        project.pages[0].canvas = CanvasMode::FixedWidth;
        assert_eq!(project.pages[0].size_for_output(), [1280.0, 1300.0]);
        project.pages[0].widgets[0].hidden = true;
        assert_eq!(project.pages[0].size_for_output(), [1280.0, 64.0]);
        project.validate().unwrap();
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Widget {
    pub id: u64,
    pub name: String,
    pub kind: WidgetKind,
    /// None uses the independent editor's same-name radio groups. An explicit
    /// empty group preserves an ungrouped native radio button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio_group: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Clockwise degrees around the widget's center.
    #[serde(default)]
    pub rotation: f32,
    /// Group IDs from the outermost group to the innermost group.
    #[serde(default)]
    pub groups: Vec<u64>,
    /// Native normalized vector contours, when available for shape editing.
    #[serde(default)]
    pub vector_points: Option<serde_json::Value>,
    pub text: String,
    pub fill: String,
    pub stroke: String,
    pub text_color: String,
    pub font_size: f32,
    /// Optional source layout; absent keeps the independent editor's legacy defaults.
    #[serde(default)]
    pub text_layout: Option<TextLayout>,
    pub radius: f32,
    pub opacity: f32,
    pub hidden: bool,
    pub locked: bool,
    pub interaction: Interaction,
    pub image_data: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetKind {
    Rectangle,
    Ellipse,
    Text,
    Button,
    Input,
    Checkbox,
    Radio,
    TextArea,
    Droplist,
    ListBox,
    Line,
    Image,
    Panel,
}

impl WidgetKind {
    pub const ALL: [Self; 13] = [
        Self::Rectangle,
        Self::Ellipse,
        Self::Text,
        Self::Button,
        Self::Input,
        Self::Checkbox,
        Self::Radio,
        Self::TextArea,
        Self::Droplist,
        Self::ListBox,
        Self::Line,
        Self::Image,
        Self::Panel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rectangle => "矩形",
            Self::Ellipse => "椭圆",
            Self::Text => "文本",
            Self::Button => "按钮",
            Self::Input => "输入框",
            Self::Checkbox => "复选框",
            Self::Radio => "单选按钮",
            Self::TextArea => "文本域",
            Self::Droplist => "下拉列表",
            Self::ListBox => "列表框",
            Self::Line => "直线",
            Self::Image => "图片",
            Self::Panel => "容器",
        }
    }

    /// Native plain text controls store their text on the control itself.
    pub fn is_plain_text_control(self) -> bool {
        matches!(
            self,
            Self::Input | Self::TextArea | Self::Droplist | Self::ListBox
        )
    }

    pub fn is_option_list(self) -> bool {
        matches!(self, Self::Droplist | Self::ListBox)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Interaction {
    pub action: Action,
    pub target_page: Option<u64>,
    pub target_widget: Option<u64>,
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    None,
    Navigate,
    ToggleVisibility,
    SetText,
}

impl Widget {
    pub fn new(id: u64, kind: WidgetKind, x: f32, y: f32) -> Self {
        let (width, height, text) = match kind {
            WidgetKind::Text => (180.0, 32.0, "双击编辑文本"),
            WidgetKind::Button => (140.0, 44.0, "按钮"),
            WidgetKind::Input => (240.0, 44.0, "请输入内容"),
            WidgetKind::Checkbox => (180.0, 32.0, "复选框"),
            WidgetKind::Radio => (180.0, 32.0, "选项"),
            WidgetKind::TextArea => (300.0, 140.0, "请输入多行内容"),
            WidgetKind::Droplist => (220.0, 40.0, "选项 1\n选项 2\n选项 3"),
            WidgetKind::ListBox => (220.0, 132.0, "选项 1\n选项 2\n选项 3"),
            WidgetKind::Line => (240.0, 2.0, ""),
            WidgetKind::Image => (240.0, 160.0, "图片"),
            WidgetKind::Panel => (360.0, 240.0, ""),
            WidgetKind::Ellipse => (120.0, 120.0, ""),
            WidgetKind::Rectangle => (180.0, 100.0, ""),
        };
        Self {
            id,
            name: format!("{} {}", kind.label(), id),
            kind,
            radio_group: None,
            x,
            y,
            width,
            height,
            rotation: 0.0,
            groups: Vec::new(),
            vector_points: None,
            text: text.to_owned(),
            fill: match kind {
                WidgetKind::Text | WidgetKind::Checkbox | WidgetKind::Radio => "#FFFFFF00",
                WidgetKind::Button => "#6759E8",
                WidgetKind::Line => "#DDE2EB",
                _ => "#FFFFFF",
            }
            .to_owned(),
            stroke: match kind {
                WidgetKind::Text
                | WidgetKind::Checkbox
                | WidgetKind::Radio
                | WidgetKind::Button
                | WidgetKind::Line => "#FFFFFF00",
                _ => "#DDE2EB",
            }
            .to_owned(),
            text_color: if kind == WidgetKind::Button {
                "#FFFFFF"
            } else {
                "#243047"
            }
            .to_owned(),
            font_size: 16.0,
            text_layout: None,
            radius: if matches!(kind, WidgetKind::Text | WidgetKind::Line) {
                0.0
            } else {
                10.0
            },
            opacity: 1.0,
            hidden: false,
            locked: false,
            interaction: Interaction::default(),
            image_data: None,
        }
    }
}

impl Project {
    pub fn blank() -> Self {
        Self {
            version: FORMAT_VERSION,
            name: "未命名项目".to_owned(),
            pages: vec![Page {
                id: 1,
                name: "页面 1".to_owned(),
                width: 1280.0,
                height: 800.0,
                canvas: CanvasMode::Fixed,
                background: "#FFFFFF".to_owned(),
                widgets: Vec::new(),
            }],
            page_tree: Vec::new(),
            interaction_events: Vec::new(),
            next_id: 2,
        }
    }

    pub fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("项目 ID 已耗尽");
        id
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == FORMAT_VERSION,
            "不支持的项目版本：{}",
            self.version
        );
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 1024,
            "项目名称不能为空且不能超过 1024 字节"
        );
        ensure!(
            !self.pages.is_empty() && self.pages.len() <= 128,
            "项目应包含 1–128 个页面"
        );
        let mut ids = HashSet::new();
        let mut page_ids = HashSet::new();
        let mut widget_ids = HashSet::new();
        let mut count = 0usize;
        for page in &self.pages {
            validate_page(page).with_context(|| format!("页面「{}」无效", page.name))?;
            ensure!(
                page.id > 0 && ids.insert(page.id),
                "页面 ID 重复或为零：{}",
                page.id
            );
            page_ids.insert(page.id);
            for widget in &page.widgets {
                ensure!(
                    widget.id > 0 && ids.insert(widget.id),
                    "元件 ID 重复或为零：{}",
                    widget.id
                );
                widget_ids.insert(widget.id);
                count += 1;
            }
        }
        ensure!(count <= 20_000, "项目最多包含 20000 个元件");
        validate_page_tree(&self.page_tree, &page_ids)?;
        let mut groups: BTreeMap<u64, (u64, Vec<u64>, usize)> = BTreeMap::new();
        for page in &self.pages {
            for (position, widget) in page.widgets.iter().enumerate() {
                for (depth, group) in widget.groups.iter().enumerate() {
                    ensure!(
                        !ids.contains(group),
                        "组合 ID 与页面或元件 ID 冲突：{group}"
                    );
                    let prefix = &widget.groups[..=depth];
                    if let Some((owner, path, previous)) = groups.get_mut(group) {
                        ensure!(*owner == page.id, "同一组合不能跨越多个页面：{group}");
                        ensure!(path.as_slice() == prefix, "组合的父级路径不一致：{group}");
                        ensure!(
                            *previous + 1 == position,
                            "同一组合的成员必须在图层中连续排列：{group}"
                        );
                        *previous = position;
                    } else {
                        groups.insert(*group, (page.id, prefix.to_vec(), position));
                    }
                }
            }
        }
        ids.extend(groups.keys().copied());
        let highest = ids.iter().copied().max().unwrap_or(0);
        ensure!(
            self.next_id > highest && self.next_id < u64::MAX,
            "next_id 必须大于所有页面、元件和组合 ID"
        );
        for page in &self.pages {
            for widget in &page.widgets {
                let interaction = &widget.interaction;
                match interaction.action {
                    Action::None => {}
                    Action::Navigate => ensure!(
                        interaction
                            .target_page
                            .is_some_and(|id| page_ids.contains(&id)),
                        "元件「{}」的跳转页面不存在",
                        widget.name
                    ),
                    Action::ToggleVisibility | Action::SetText => ensure!(
                        interaction
                            .target_widget
                            .is_some_and(|id| widget_ids.contains(&id)),
                        "元件「{}」的目标元件不存在",
                        widget.name
                    ),
                }
            }
        }
        ensure!(
            self.interaction_events.len() <= 4096,
            "最多创建4096个交互用例"
        );
        let mut event_ids = HashSet::new();
        for event in &self.interaction_events {
            ensure!(
                event.id.len() == 32
                    && event.id.bytes().all(|b| b.is_ascii_hexdigit())
                    && event_ids.insert(&event.id),
                "交互标识无效或重复"
            );
            ensure!(
                !event.name.trim().is_empty() && event.name.len() <= 1024,
                "交互用例名称不能为空且不能超过1024字节"
            );
            let page = self
                .pages
                .iter()
                .find(|page| page.id == event.page)
                .context("交互所在页面不存在")?;
            ensure!(
                matches!(
                    (event.trigger, event.owner),
                    (EventTrigger::PageLoad, None) | (EventTrigger::Click, Some(_))
                ),
                "交互事件与页面/元件目标不一致"
            );
            if let Some(owner) = event.owner {
                ensure!(
                    page.widgets.iter().any(|w| w.id == owner),
                    "交互所属元件不在当前页面"
                );
            }
            ensure!(event.actions.len() <= 256, "每个用例最多256个动作");
            for action in &event.actions {
                ensure!(action.value.len() <= 65536, "交互文字不能超过65536字节");
                match action.action {
                    Action::None => bail!("交互动作不能为空"),
                    Action::Navigate => ensure!(
                        action.target_page.is_none_or(|id| page_ids.contains(&id)),
                        "交互跳转目标不存在"
                    ),
                    Action::ToggleVisibility | Action::SetText => ensure!(
                        action
                            .target_widget
                            .is_some_and(|id| page.widgets.iter().any(|w| w.id == id)),
                        "交互目标元件不在当前页面"
                    ),
                }
            }
        }
        Ok(())
    }

    pub fn prune_interaction_references(&mut self) {
        let pages = self
            .pages
            .iter()
            .map(|page| {
                (
                    page.id,
                    page.widgets.iter().map(|w| w.id).collect::<HashSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        self.interaction_events.retain_mut(|event| {
            let Some(widgets) = pages.get(&event.page) else {
                return false;
            };
            if event.owner.is_some_and(|id| !widgets.contains(&id)) {
                return false;
            }
            event.actions.retain(|action| match action.action {
                Action::None => false,
                Action::Navigate => action.target_page.is_none_or(|id| pages.contains_key(&id)),
                Action::ToggleVisibility | Action::SetText => {
                    action.target_widget.is_some_and(|id| widgets.contains(&id))
                }
            });
            true
        });
    }

    pub fn duplicate_interaction_events(&mut self, page: u64, mapping: &BTreeMap<u64, u64>) {
        let copied = self
            .interaction_events
            .iter()
            .filter(|event| {
                event.page == page && event.owner.is_some_and(|id| mapping.contains_key(&id))
            })
            .map(|event| {
                let mut copied =
                    AuthoredEvent::new(page, event.owner.map(|id| mapping[&id]), event.trigger);
                copied.name = event.name.clone();
                copied.actions = event.actions.clone();
                for action in &mut copied.actions {
                    if let Some(target) = action
                        .target_widget
                        .and_then(|id| mapping.get(&id))
                        .copied()
                    {
                        action.target_widget = Some(target);
                    }
                }
                copied
            })
            .collect::<Vec<_>>();
        self.interaction_events.extend(copied);
    }

    /// Write to a sibling temporary file, flush it, then atomically replace the destination.
    /// A failed write never truncates an existing project.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).context("无法序列化项目")?;
        ensure!(
            bytes.len() as u64 <= MAX_PROJECT_BYTES,
            "项目文件超过 48 MB 限制"
        );
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        ensure!(parent.is_dir(), "保存目录不存在：{}", parent.display());
        let name = path
            .file_name()
            .context("保存路径缺少文件名")?
            .to_string_lossy();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut temporary = None;
        for attempt in 0..16 {
            let candidate = parent.join(format!(
                ".{name}.{}.{nonce}.{attempt}.tmp",
                std::process::id()
            ));
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&candidate)
            {
                Ok(file) => {
                    temporary = Some((candidate, file));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error).context("无法创建临时项目文件"),
            }
        }
        let (temporary_path, mut file) = temporary.context("无法创建唯一临时项目文件")?;
        let result = (|| -> Result<()> {
            file.write_all(&bytes).context("写入项目失败")?;
            file.sync_all().context("同步项目失败")?;
            drop(file);
            atomic_replace(&temporary_path, path).context("替换项目文件失败")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }

    pub fn load(path: &Path) -> Result<Self> {
        let file = fs::File::open(path).context("无法打开项目文件")?;
        let metadata = file.metadata().context("无法读取项目文件信息")?;
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_PROJECT_BYTES,
            "项目必须是小于 48 MB 的普通文件"
        );
        let mut bytes = Vec::new();
        file.take(MAX_PROJECT_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("无法读取项目文件")?;
        ensure!(
            bytes.len() as u64 <= MAX_PROJECT_BYTES,
            "项目文件超过 48 MB 限制"
        );
        let project: Self =
            serde_json::from_slice(&bytes).context("项目格式无效，预期为 .rpshop JSON 文件")?;
        project.validate()?;
        Ok(project)
    }
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let mut source_wide: Vec<u16> = source.as_os_str().encode_wide().collect();
    let mut destination_wide: Vec<u16> = destination.as_os_str().encode_wide().collect();
    if source_wide.contains(&0) || destination_wide.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "路径包含 NUL 字符",
        ));
    }
    source_wide.push(0);
    destination_wide.push(0);
    // Both paths share a directory, so this never falls back to a cross-volume copy.
    let result = unsafe { MoveFileExW(source_wide.as_ptr(), destination_wide.as_ptr(), 0x1 | 0x8) };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

pub(crate) fn valid_color(value: &str) -> bool {
    matches!(value.len(), 7 | 9)
        && value.starts_with('#')
        && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn validate_page_tree(tree: &[PageTreeNode], page_ids: &HashSet<u64>) -> Result<()> {
    if tree.is_empty() {
        return Ok(());
    }
    fn visit(
        nodes: &[PageTreeNode],
        page_ids: &HashSet<u64>,
        seen_pages: &mut HashSet<u64>,
        seen_folders: &mut HashSet<String>,
        count: &mut usize,
        depth: usize,
    ) -> Result<()> {
        ensure!(depth <= 64, "页面目录嵌套超过 64 层");
        for node in nodes {
            *count += 1;
            ensure!(*count <= 4096, "页面目录最多包含 4096 个节点");
            match node {
                PageTreeNode::Folder { id, name, .. } => {
                    ensure!(
                        !id.is_empty() && id.len() <= 1024 && seen_folders.insert(id.clone()),
                        "页面文件夹 ID 为空、过长或重复"
                    );
                    ensure!(
                        !name.trim().is_empty() && name.len() <= 1024,
                        "页面文件夹名称不能为空或超过 1024 字节"
                    );
                }
                PageTreeNode::Page { page_id, .. } => {
                    ensure!(
                        page_ids.contains(page_id) && seen_pages.insert(*page_id),
                        "页面目录引用不存在或重复的页面：{page_id}"
                    );
                }
            }
            visit(
                node.children(),
                page_ids,
                seen_pages,
                seen_folders,
                count,
                depth + 1,
            )?;
        }
        Ok(())
    }
    let mut seen_pages = HashSet::new();
    visit(
        tree,
        page_ids,
        &mut seen_pages,
        &mut HashSet::new(),
        &mut 0,
        0,
    )?;
    ensure!(&seen_pages == page_ids, "页面目录没有包含所有页面");
    Ok(())
}

pub(crate) fn validate_page(page: &Page) -> Result<()> {
    ensure!(page.id > 0, "页面 ID 不能为零");
    ensure!(
        !page.name.trim().is_empty() && page.name.len() <= 1024,
        "页面名称不能为空且不能超过 1024 字节"
    );
    ensure!(
        bounded(page.width, 64.0, 20_000.0) && bounded(page.height, 64.0, 20_000.0),
        "页面尺寸应在 64–20000 之间"
    );
    ensure!(valid_color(&page.background), "页面背景颜色无效");
    ensure!(page.widgets.len() <= 5_000, "单页最多包含 5000 个元件");
    let mut ids = HashSet::new();
    for widget in &page.widgets {
        ensure!(widget.id > 0 && ids.insert(widget.id), "元件 ID 重复或为零");
        ensure!(
            widget.groups.len() <= MAX_GROUP_DEPTH,
            "元件组合不能超过 32 层"
        );
        let mut ancestors = HashSet::new();
        ensure!(
            widget
                .groups
                .iter()
                .all(|group| *group > 0 && ancestors.insert(*group)),
            "组合 ID 不能为零或在同一路径中重复"
        );
        ensure!(
            widget.name.len() <= 1024
                && widget
                    .radio_group
                    .as_ref()
                    .is_none_or(|group| group.len() <= 1024)
                && widget.text.len() <= 100_000
                && widget.interaction.value.len() <= 100_000,
            "元件文本过长"
        );
        ensure!(
            bounded(widget.x, -100_000.0, 100_000.0) && bounded(widget.y, -100_000.0, 100_000.0),
            "元件坐标必须是有限数字"
        );
        ensure!(
            bounded(widget.width, 1.0, 20_000.0) && bounded(widget.height, 1.0, 20_000.0),
            "元件尺寸应在 1–20000 之间"
        );
        ensure!(bounded(widget.font_size, 4.0, 512.0), "字号应在 4–512 之间");
        if let Some(layout) = &widget.text_layout {
            ensure!(
                layout.horizontal <= 2
                    && layout.vertical <= 2
                    && layout
                        .padding
                        .iter()
                        .all(|value| bounded(*value, 0.0, 20_000.0)),
                "文字对齐或内边距无效"
            );
        }
        ensure!(
            bounded(widget.rotation, -36_000.0, 36_000.0),
            "旋转角度必须为 -36000–36000 之间的有限数字"
        );
        ensure!(
            bounded(widget.radius, 0.0, 20_000.0) && bounded(widget.opacity, 0.0, 1.0),
            "圆角或透明度无效"
        );
        ensure!(
            valid_color(&widget.fill)
                && valid_color(&widget.stroke)
                && valid_color(&widget.text_color),
            "元件颜色应为 #RRGGBB 或 #RRGGBBAA"
        );
        if let Some(data) = &widget.image_data {
            validate_image(data).with_context(|| format!("元件「{}」的图片无效", widget.name))?;
        }
        if let Some(points) = &widget.vector_points {
            crate::vector_edit::validate_points(points)
                .with_context(|| format!("元件「{}」的矢量路径无效", widget.name))?;
        }
    }
    Ok(())
}

fn bounded(value: f32, min: f32, max: f32) -> bool {
    value.is_finite() && value >= min && value <= max
}

fn validate_image(data: &str) -> Result<()> {
    ensure!(
        data.len() <= MAX_IMAGE_BYTES * 4 / 3 + 64,
        "图片超过 12 MB 限制"
    );
    let (header, payload) = data.split_once(',').context("图片必须为 base64 data URI")?;
    let expected = match header {
        "data:image/png;base64" => image::ImageFormat::Png,
        "data:image/jpeg;base64" => image::ImageFormat::Jpeg,
        "data:image/gif;base64" => image::ImageFormat::Gif,
        "data:image/webp;base64" => image::ImageFormat::WebP,
        _ => bail!("只支持 PNG、JPEG、GIF 和 WebP 图片"),
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .context("图片 base64 无效")?;
    ensure!(bytes.len() <= MAX_IMAGE_BYTES, "图片超过 12 MB 限制");
    ensure!(
        image::guess_format(&bytes).context("无法识别图片格式")? == expected,
        "图片内容与 MIME 类型不匹配"
    );
    let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), expected);
    let (width, height) = reader.into_dimensions().context("无法读取图片尺寸")?;
    ensure!(
        width > 0
            && height > 0
            && width <= 8192
            && height <= 8192
            && u64::from(width) * u64::from(height) <= 32_000_000,
        "图片尺寸过大"
    );
    Ok(())
}

impl Default for Project {
    fn default() -> Self {
        let mut next_id = 3;
        let mut overview = sample_page(1, "工作台");
        let mut library = sample_page(2, "组件与交互");
        for (page, active) in [(&mut overview, 1u64), (&mut library, 2u64)] {
            sample(
                &mut next_id,
                page,
                WidgetKind::Panel,
                0.0,
                0.0,
                224.0,
                800.0,
                "",
                "#FFFFFF",
                "#243047",
                16.0,
            );
            sample(
                &mut next_id,
                page,
                WidgetKind::Rectangle,
                28.0,
                30.0,
                38.0,
                38.0,
                "R",
                "#6759E8",
                "#FFFFFF",
                22.0,
            );
            sample(
                &mut next_id,
                page,
                WidgetKind::Text,
                79.0,
                31.0,
                128.0,
                34.0,
                "rpshop",
                "#FFFFFF00",
                "#243047",
                25.0,
            );
            sample(
                &mut next_id,
                page,
                WidgetKind::Text,
                28.0,
                91.0,
                170.0,
                24.0,
                "想法，从这里开始。",
                "#FFFFFF00",
                "#929BAA",
                13.0,
            );
            for (target, y, label) in [(1, 150.0, "工作台"), (2, 204.0, "组件与交互")] {
                let id = sample(
                    &mut next_id,
                    page,
                    WidgetKind::Button,
                    16.0,
                    y,
                    192.0,
                    44.0,
                    label,
                    if target == active {
                        "#EEEAFE"
                    } else {
                        "#FFFFFF00"
                    },
                    if target == active {
                        "#6759E8"
                    } else {
                        "#647085"
                    },
                    15.0,
                );
                page.widgets
                    .iter_mut()
                    .find(|w| w.id == id)
                    .unwrap()
                    .interaction = Interaction {
                    action: Action::Navigate,
                    target_page: Some(target),
                    ..Default::default()
                };
            }
            sample(
                &mut next_id,
                page,
                WidgetKind::Line,
                24.0,
                278.0,
                176.0,
                1.0,
                "",
                "#E9EDF3",
                "#243047",
                16.0,
            );
            sample(
                &mut next_id,
                page,
                WidgetKind::Text,
                28.0,
                300.0,
                170.0,
                24.0,
                "我的工作空间",
                "#FFFFFF00",
                "#929BAA",
                12.0,
            );
            sample(
                &mut next_id,
                page,
                WidgetKind::Text,
                28.0,
                340.0,
                170.0,
                28.0,
                "○   产品设计",
                "#FFFFFF00",
                "#647085",
                14.0,
            );
            sample(
                &mut next_id,
                page,
                WidgetKind::Text,
                28.0,
                384.0,
                170.0,
                28.0,
                "○   灵感收集",
                "#FFFFFF00",
                "#647085",
                14.0,
            );
            sample(
                &mut next_id,
                page,
                WidgetKind::Panel,
                16.0,
                690.0,
                192.0,
                88.0,
                "",
                "#F5F3FF",
                "#243047",
                14.0,
            );
            sample(
                &mut next_id,
                page,
                WidgetKind::Text,
                30.0,
                705.0,
                165.0,
                24.0,
                "本地创作，自由掌控",
                "#FFFFFF00",
                "#6759E8",
                14.0,
            );
            sample(
                &mut next_id,
                page,
                WidgetKind::Text,
                30.0,
                735.0,
                165.0,
                22.0,
                "无需账号 · 随时保存",
                "#FFFFFF00",
                "#929BAA",
                12.0,
            );
        }
        sample(
            &mut next_id,
            &mut overview,
            WidgetKind::Text,
            264.0,
            38.0,
            650.0,
            40.0,
            "让每个想法，都有形状。",
            "#FFFFFF00",
            "#243047",
            30.0,
        );
        sample(
            &mut next_id,
            &mut overview,
            WidgetKind::Text,
            265.0,
            87.0,
            740.0,
            28.0,
            "欢迎来到你的设计工作台。拖入组件，连接页面，预览下一次灵感。",
            "#FFFFFF00",
            "#929BAA",
            14.0,
        );
        let nav = sample(
            &mut next_id,
            &mut overview,
            WidgetKind::Button,
            1060.0,
            48.0,
            176.0,
            44.0,
            "探索组件  →",
            "#6759E8",
            "#FFFFFF",
            15.0,
        );
        overview
            .widgets
            .iter_mut()
            .find(|w| w.id == nav)
            .unwrap()
            .interaction = Interaction {
            action: Action::Navigate,
            target_page: Some(2),
            ..Default::default()
        };
        for (x, label, number, caption, accent) in [
            (264.0, "设计项目", "12", "本周新增 3 个项目", "#6759E8"),
            (596.0, "页面与画板", "48", "让思路有序展开", "#229B87"),
            (928.0, "可复用组件", "126", "专注设计，减少重复", "#E5A14A"),
        ] {
            sample(
                &mut next_id,
                &mut overview,
                WidgetKind::Panel,
                x,
                150.0,
                308.0,
                160.0,
                "",
                "#FFFFFF",
                "#243047",
                14.0,
            );
            sample(
                &mut next_id,
                &mut overview,
                WidgetKind::Text,
                x + 24.0,
                170.0,
                240.0,
                26.0,
                label,
                "#FFFFFF00",
                "#929BAA",
                14.0,
            );
            sample(
                &mut next_id,
                &mut overview,
                WidgetKind::Text,
                x + 24.0,
                205.0,
                240.0,
                48.0,
                number,
                "#FFFFFF00",
                "#243047",
                36.0,
            );
            sample(
                &mut next_id,
                &mut overview,
                WidgetKind::Text,
                x + 24.0,
                268.0,
                260.0,
                22.0,
                caption,
                "#FFFFFF00",
                accent,
                12.0,
            );
        }
        sample(
            &mut next_id,
            &mut overview,
            WidgetKind::Text,
            264.0,
            352.0,
            400.0,
            34.0,
            "最近的灵感",
            "#FFFFFF00",
            "#243047",
            22.0,
        );
        sample(
            &mut next_id,
            &mut overview,
            WidgetKind::Text,
            1060.0,
            356.0,
            176.0,
            26.0,
            "持续创造，保持好奇",
            "#FFFFFF00",
            "#929BAA",
            13.0,
        );
        for (x, tint, accent, title, detail, symbol) in [
            (
                264.0,
                "#EEEAFE",
                "#6759E8",
                "品牌官网 · 焕新计划",
                "12 个页面  ·  刚刚编辑",
                "01",
            ),
            (
                596.0,
                "#E6F5F0",
                "#229B87",
                "数据工作台",
                "8 个页面  ·  今天 10:24",
                "02",
            ),
            (
                928.0,
                "#FFF2DF",
                "#D89639",
                "移动端 · 生活方式",
                "24 个页面  ·  昨天",
                "03",
            ),
        ] {
            sample(
                &mut next_id,
                &mut overview,
                WidgetKind::Panel,
                x,
                410.0,
                308.0,
                284.0,
                "",
                "#FFFFFF",
                "#243047",
                16.0,
            );
            sample(
                &mut next_id,
                &mut overview,
                WidgetKind::Rectangle,
                x + 16.0,
                426.0,
                276.0,
                170.0,
                "",
                tint,
                accent,
                16.0,
            );
            sample(
                &mut next_id,
                &mut overview,
                WidgetKind::Text,
                x + 112.0,
                466.0,
                90.0,
                80.0,
                symbol,
                "#FFFFFF00",
                accent,
                58.0,
            );
            sample(
                &mut next_id,
                &mut overview,
                WidgetKind::Text,
                x + 20.0,
                614.0,
                268.0,
                28.0,
                title,
                "#FFFFFF00",
                "#243047",
                17.0,
            );
            sample(
                &mut next_id,
                &mut overview,
                WidgetKind::Text,
                x + 20.0,
                652.0,
                268.0,
                22.0,
                detail,
                "#FFFFFF00",
                "#929BAA",
                12.0,
            );
        }
        sample(
            &mut next_id,
            &mut overview,
            WidgetKind::Text,
            265.0,
            738.0,
            960.0,
            28.0,
            "小提示：选中元件可编辑样式；在「交互」中设置页面跳转、显示隐藏与文本变化。",
            "#FFFFFF00",
            "#929BAA",
            13.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Text,
            264.0,
            38.0,
            800.0,
            40.0,
            "把想法，变成可点击的原型。",
            "#FFFFFF00",
            "#243047",
            30.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Text,
            265.0,
            87.0,
            950.0,
            28.0,
            "从基础组件开始，尝试下方交互。导出的 HTML 同样可以独立运行。",
            "#FFFFFF00",
            "#929BAA",
            14.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Panel,
            264.0,
            150.0,
            972.0,
            262.0,
            "",
            "#FFFFFF",
            "#243047",
            16.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Text,
            292.0,
            170.0,
            860.0,
            32.0,
            "基础组件",
            "#FFFFFF00",
            "#243047",
            20.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Rectangle,
            296.0,
            230.0,
            168.0,
            112.0,
            "矩形",
            "#EEEAFE",
            "#6759E8",
            16.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Ellipse,
            516.0,
            230.0,
            112.0,
            112.0,
            "椭圆",
            "#E6F5F0",
            "#229B87",
            16.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Input,
            684.0,
            232.0,
            232.0,
            44.0,
            "输入一些内容…",
            "#F8FAFC",
            "#929BAA",
            14.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Checkbox,
            684.0,
            302.0,
            236.0,
            32.0,
            "保持我的创作节奏",
            "#FFFFFF00",
            "#647085",
            14.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Button,
            972.0,
            232.0,
            224.0,
            44.0,
            "一个主按钮",
            "#6759E8",
            "#FFFFFF",
            15.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Line,
            972.0,
            318.0,
            224.0,
            2.0,
            "",
            "#DDE2EB",
            "#647085",
            14.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Text,
            296.0,
            367.0,
            880.0,
            24.0,
            "矩形、椭圆、文字、按钮、表单、图片与容器 —— 自由组合，随心表达。",
            "#FFFFFF00",
            "#929BAA",
            13.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Panel,
            264.0,
            444.0,
            972.0,
            278.0,
            "",
            "#FFFFFF",
            "#243047",
            16.0,
        );
        sample(
            &mut next_id,
            &mut library,
            WidgetKind::Text,
            292.0,
            464.0,
            860.0,
            32.0,
            "让页面回应你",
            "#FFFFFF00",
            "#243047",
            20.0,
        );
        let message = sample(
            &mut next_id,
            &mut library,
            WidgetKind::Text,
            296.0,
            521.0,
            860.0,
            38.0,
            "你好，创造者。点击按钮，看看会发生什么。",
            "#FFFFFF00",
            "#647085",
            17.0,
        );
        let badge = sample(
            &mut next_id,
            &mut library,
            WidgetKind::Rectangle,
            296.0,
            638.0,
            520.0,
            44.0,
            "✦ 灵感已就位，开始你的下一个作品吧。",
            "#EEEAFE",
            "#6759E8",
            14.0,
        );
        let set = sample(
            &mut next_id,
            &mut library,
            WidgetKind::Button,
            296.0,
            578.0,
            180.0,
            42.0,
            "改变一句话",
            "#6759E8",
            "#FFFFFF",
            14.0,
        );
        library
            .widgets
            .iter_mut()
            .find(|w| w.id == set)
            .unwrap()
            .interaction = Interaction {
            action: Action::SetText,
            target_widget: Some(message),
            value: "好设计，始于一次大胆的尝试。现在轮到你了！".to_owned(),
            ..Default::default()
        };
        let toggle = sample(
            &mut next_id,
            &mut library,
            WidgetKind::Button,
            496.0,
            578.0,
            180.0,
            42.0,
            "显示 / 隐藏提示",
            "#EEEAFE",
            "#6759E8",
            14.0,
        );
        library
            .widgets
            .iter_mut()
            .find(|w| w.id == toggle)
            .unwrap()
            .interaction = Interaction {
            action: Action::ToggleVisibility,
            target_widget: Some(badge),
            ..Default::default()
        };
        let back = sample(
            &mut next_id,
            &mut library,
            WidgetKind::Button,
            696.0,
            578.0,
            180.0,
            42.0,
            "返回工作台 →",
            "#F0F3F8",
            "#647085",
            14.0,
        );
        library
            .widgets
            .iter_mut()
            .find(|w| w.id == back)
            .unwrap()
            .interaction = Interaction {
            action: Action::Navigate,
            target_page: Some(1),
            ..Default::default()
        };
        Self {
            version: FORMAT_VERSION,
            name: "灵感工作台".to_owned(),
            pages: vec![overview, library],
            page_tree: Vec::new(),
            interaction_events: Vec::new(),
            next_id,
        }
    }
}

fn sample_page(id: u64, name: &str) -> Page {
    Page {
        id,
        name: name.to_owned(),
        width: 1280.0,
        height: 800.0,
        canvas: CanvasMode::Fixed,
        background: "#F4F7FB".to_owned(),
        widgets: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn sample(
    next_id: &mut u64,
    page: &mut Page,
    kind: WidgetKind,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    text: &str,
    fill: &str,
    color: &str,
    font_size: f32,
) -> u64 {
    let id = *next_id;
    *next_id += 1;
    let mut widget = Widget::new(id, kind, x, y);
    widget.width = width;
    widget.height = height;
    widget.text = text.to_owned();
    widget.name = if text.is_empty() {
        format!("{} {}", kind.label(), id)
    } else {
        text.to_owned()
    };
    widget.fill = fill.to_owned();
    widget.stroke = "#FFFFFF00".to_owned();
    widget.text_color = color.to_owned();
    widget.font_size = font_size;
    if kind == WidgetKind::Panel {
        widget.radius = 14.0;
    }
    if kind == WidgetKind::Panel && x == 0.0 {
        widget.radius = 0.0;
    }
    page.widgets.push(widget);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_components_roundtrip_and_preserve_explicit_radio_groups() {
        let mut project = Project::blank();
        for kind in [
            WidgetKind::Radio,
            WidgetKind::TextArea,
            WidgetKind::Droplist,
            WidgetKind::ListBox,
        ] {
            let id = project.allocate_id();
            let mut widget = Widget::new(id, kind, 20.0, 30.0);
            if kind == WidgetKind::Radio {
                widget.radio_group = Some("preferences".into());
            }
            project.pages[0].widgets.push(widget);
        }
        project.validate().unwrap();
        let encoded = serde_json::to_string(&project).unwrap();
        assert_eq!(serde_json::from_str::<Project>(&encoded).unwrap(), project);
        let legacy = Widget::new(7, WidgetKind::Checkbox, 0.0, 0.0);
        let encoded = serde_json::to_value(&legacy).unwrap();
        assert!(encoded.get("radio_group").is_none());
        assert_eq!(serde_json::from_value::<Widget>(encoded).unwrap(), legacy);
    }

    #[test]
    fn group_paths_roundtrip_and_older_widgets_default_to_ungrouped() {
        let mut project = Project::blank();
        for path in [vec![10, 11], vec![10, 11], vec![10, 12]] {
            let id = project.allocate_id();
            let mut widget = Widget::new(id, WidgetKind::Rectangle, 0.0, 0.0);
            widget.groups = path;
            project.pages[0].widgets.push(widget);
        }
        project.next_id = 13;
        project.validate().unwrap();
        assert_eq!(
            serde_json::from_value::<Project>(serde_json::to_value(&project).unwrap()).unwrap(),
            project
        );
        let mut old_widget = serde_json::to_value(&project.pages[0].widgets[0]).unwrap();
        old_widget.as_object_mut().unwrap().remove("groups");
        assert!(
            serde_json::from_value::<Widget>(old_widget)
                .unwrap()
                .groups
                .is_empty()
        );
    }

    #[test]
    fn validates_group_identity_paths_pages_and_contiguous_layers() {
        let mut valid = Project::blank();
        for _ in 0..3 {
            let id = valid.allocate_id();
            let mut widget = Widget::new(id, WidgetKind::Rectangle, 0.0, 0.0);
            widget.groups = vec![10];
            valid.pages[0].widgets.push(widget);
        }
        valid.next_id = 11;
        valid.validate().unwrap();
        for path in [
            vec![0],
            vec![1],
            vec![2],
            vec![10, 10],
            (100..133).collect(),
        ] {
            let mut invalid = valid.clone();
            invalid.pages[0].widgets[0].groups = path;
            invalid.next_id = 134;
            assert!(invalid.validate().is_err());
        }
        let mut invalid = valid.clone();
        invalid.pages[0].widgets[1].groups = vec![11, 10];
        invalid.next_id = 12;
        assert!(
            invalid.validate().is_err(),
            "same group cannot have different ancestors"
        );
        let mut invalid = valid.clone();
        invalid.pages[0].widgets[1].groups.clear();
        assert!(
            invalid.validate().is_err(),
            "group members must occupy consecutive layers"
        );
        let mut invalid = valid.clone();
        let mut second = invalid.pages[0].clone();
        second.id = 5;
        second.widgets.truncate(1);
        second.widgets[0].id = 6;
        invalid.pages.push(second);
        assert!(
            invalid.validate().is_err(),
            "a group belongs to exactly one page"
        );
        let mut invalid = valid;
        invalid.next_id = 10;
        assert!(
            invalid.validate().is_err(),
            "the allocator includes group IDs"
        );
    }

    #[test]
    fn page_tree_roundtrips_and_validates_unique_complete_references() {
        let mut project = Project::default();
        let first = project.pages[0].id;
        let second = project.pages[1].id;
        project.page_tree = vec![PageTreeNode::Folder {
            id: "folder-a".into(),
            name: "文档".into(),
            children: vec![PageTreeNode::Page {
                page_id: first,
                children: vec![PageTreeNode::Page {
                    page_id: second,
                    children: vec![],
                }],
            }],
        }];
        project.validate().unwrap();
        let value = serde_json::to_value(&project).unwrap();
        assert_eq!(
            serde_json::from_value::<Project>(value.clone()).unwrap(),
            project
        );
        let mut old = value;
        old.as_object_mut().unwrap().remove("page_tree");
        assert!(
            serde_json::from_value::<Project>(old)
                .unwrap()
                .page_tree
                .is_empty()
        );
        for id in [first, 999_999] {
            let mut invalid = project.clone();
            invalid.page_tree.push(PageTreeNode::Page {
                page_id: id,
                children: vec![],
            });
            assert!(invalid.validate().is_err());
        }
        let mut invalid = project.clone();
        invalid.page_tree = vec![PageTreeNode::Page {
            page_id: first,
            children: vec![],
        }];
        assert!(invalid.validate().is_err());
        let mut invalid = project;
        invalid.page_tree.push(PageTreeNode::Folder {
            id: "folder-a".into(),
            name: "重名 ID".into(),
            children: vec![],
        });
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn rotation_roundtrips_and_older_widgets_default_to_zero() {
        let mut widget = Widget::new(2, WidgetKind::Line, 20.0, 30.0);
        widget.rotation = -37.5;
        let mut encoded = serde_json::to_value(&widget).unwrap();
        assert_eq!(
            serde_json::from_value::<Widget>(encoded.clone()).unwrap(),
            widget
        );
        encoded.as_object_mut().unwrap().remove("rotation");
        assert_eq!(
            serde_json::from_value::<Widget>(encoded).unwrap().rotation,
            0.0
        );
    }

    #[test]
    fn rejects_non_finite_or_excessive_rotation() {
        let mut project = Project::blank();
        let id = project.allocate_id();
        project.pages[0]
            .widgets
            .push(Widget::new(id, WidgetKind::Line, 0.0, 0.0));
        for rotation in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            36_001.0,
            -36_001.0,
        ] {
            project.pages[0].widgets[0].rotation = rotation;
            assert!(project.validate().is_err());
        }
        project.pages[0].widgets[0].rotation = 36_000.0;
        project.validate().unwrap();
    }

    #[test]
    fn sample_roundtrips_and_references_are_valid() {
        let sample = Project::default();
        sample.validate().unwrap();
        assert_eq!(sample.pages.len(), 2);
        let encoded = serde_json::to_vec(&sample).unwrap();
        let decoded: Project = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(sample, decoded);
        assert!(
            sample
                .pages
                .iter()
                .flat_map(|p| &p.widgets)
                .any(|w| w.interaction.action == Action::Navigate)
        );
    }

    #[test]
    fn rejects_nan_duplicates_invalid_targets_and_css_injection() {
        let mut project = Project::blank();
        let id = project.allocate_id();
        project.pages[0]
            .widgets
            .push(Widget::new(id, WidgetKind::Button, 0.0, 0.0));
        project.pages[0].widgets[0].x = f32::NAN;
        assert!(project.validate().is_err());
        project.pages[0].widgets[0].x = 0.0;
        project.pages[0].widgets[0].fill = "red;background:url(https://example.invalid)".into();
        assert!(project.validate().is_err());
        project.pages[0].widgets[0].fill = "#123456".into();
        project.pages[0].widgets[0].interaction.action = Action::Navigate;
        project.pages[0].widgets[0].interaction.target_page = Some(999);
        assert!(project.validate().is_err());
        project.pages[0].widgets[0].interaction = Interaction::default();
        project.pages[0].widgets[0].id = 1;
        assert!(project.validate().is_err());
    }

    #[test]
    fn save_overwrites_without_corrupting_existing_document_on_validation_error() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rpshop-model-{}-{nonce}.rpshop",
            std::process::id()
        ));
        let mut project = Project::blank();
        project.save(&path).unwrap();
        project.name = "保存后的项目".into();
        project.save(&path).unwrap();
        assert_eq!(Project::load(&path).unwrap(), project);
        project.pages[0].width = f32::INFINITY;
        assert!(project.save(&path).is_err());
        assert_eq!(Project::load(&path).unwrap().name, "保存后的项目");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_executable_image_uris() {
        let mut project = Project::blank();
        let id = project.allocate_id();
        let mut widget = Widget::new(id, WidgetKind::Image, 0.0, 0.0);
        widget.image_data = Some("data:image/svg+xml,<svg onload=alert(1)>".into());
        project.pages[0].widgets.push(widget);
        assert!(project.validate().is_err());
    }
}
