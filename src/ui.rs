use base64::Engine;
use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, vec2,
};
use rpshop::manipulation::{self, Handle};
use rpshop::model::{Action, Page, Project, Widget, WidgetKind};
use rpshop::widget_library::{self, Category, LibraryItem};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    time::{Duration, Instant},
};

const PURPLE: Color32 = Color32::from_rgb(117, 83, 222);
const INK: Color32 = Color32::from_rgb(41, 43, 58);
const MUTED: Color32 = Color32::from_rgb(136, 140, 157);
const BORDER: Color32 = Color32::from_rgb(231, 232, 240);
const LAYER_CONTROL_WIDTH: f32 = 32.0;
const LAYER_COLUMN_SPACING: f32 = 4.0;

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;

#[path = "ui_render.rs"]
mod render;
use render::draw_widget;
#[path = "ui_viewport.rs"]
mod viewport;
use viewport::{CanvasView, PanDrag, ViewCommand};

#[path = "ui_page_tree.rs"]
mod page_tree;

#[path = "ui_sidebar.rs"]
mod sidebar;

#[path = "ui_inspector.rs"]
mod inspector;

#[path = "ui_interactions.rs"]
mod interactions;

#[derive(Clone)]
struct Drag {
    before: Project,
    pointer: Pos2,
    mode: DragMode,
}

#[derive(Clone)]
enum DragMode {
    Move,
    Draw {
        id: u64,
        kind: WidgetKind,
        start: [f32; 2],
    },
    Resize {
        handle: Handle,
        bounds: Box<Widget>,
    },
    Rotate {
        pivot: [f32; 2],
        angle: f32,
        start: [f32; 2],
    },
    Marquee {
        initial: BTreeSet<u64>,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Session {
    project: Project,
    path: Option<PathBuf>,
    saved: Project,
    #[serde(default)]
    import_warnings: Vec<String>,
    #[serde(default)]
    native_state: Option<serde_json::Value>,
}

struct OpenedDocument {
    project: Project,
    save_path: Option<PathBuf>,
    warnings: Vec<String>,
    native: Option<rpshop::rp_format::NativeDocument>,
}

fn is_native_path(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("rp") || e.eq_ignore_ascii_case("rplib"))
}

fn restore_native_context(
    document: &mut rpshop::rp_format::NativeDocument,
    state: Option<&serde_json::Value>,
    draft: &mut Project,
    saved: &mut Project,
) -> anyhow::Result<bool> {
    let Some(state) = state else {
        anyhow::ensure!(document.project == *saved, "原文件与保存检查点不一致");
        return Ok(false);
    };
    let old_tree_missing = state["project"]
        .get("page_tree")
        .is_none_or(|tree| tree.as_array().is_some_and(Vec::is_empty));
    if state["version"].as_u64() == Some(2) {
        let previous: Project = serde_json::from_value(state["project"].clone())?;
        let fresh = &document.project;
        if previous == *fresh {
            document.restore_session_state(state)?;
            return Ok(false);
        }
        anyhow::ensure!(*saved == previous, "旧版草稿的保存检查点不匹配");
        let ids = |project: &Project| project.pages.iter().map(|p| p.id).collect::<Vec<_>>();
        anyhow::ensure!(
            ids(&previous) == ids(fresh) && ids(draft) == ids(&previous),
            "旧版草稿页面绑定与源文件不匹配"
        );
        let mut normalized = previous.clone();
        let mut migrated_draft = draft.clone();
        if old_tree_missing && !fresh.page_tree.is_empty() {
            anyhow::ensure!(draft.page_tree.is_empty(), "旧版草稿页面目录不匹配");
            normalized.page_tree = fresh.page_tree.clone();
            migrated_draft.page_tree = fresh.page_tree.clone();
            for ((page, old), source) in migrated_draft
                .pages
                .iter_mut()
                .zip(&previous.pages)
                .zip(&fresh.pages)
            {
                if page.name == old.name {
                    page.name = source.name.clone();
                }
            }
            for (page, source) in normalized.pages.iter_mut().zip(&fresh.pages) {
                page.name = source.name.clone();
            }
        }
        let raw_pages = state["project"]["pages"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("旧版原生页面元数据无效"))?;
        for (index, ((page, old), source)) in normalized
            .pages
            .iter_mut()
            .zip(&previous.pages)
            .zip(&fresh.pages)
            .enumerate()
        {
            let raw = &raw_pages[index];
            let draft_page = &mut migrated_draft.pages[index];
            // Before 0.4.1 native dimensions were a preview fallback. Only
            // upgrade metadata absent from that baseline, preserving draft edits.
            if raw.get("canvas").is_none() {
                page.canvas = source.canvas;
                page.width = source.width;
                page.height = source.height;
                if draft_page.canvas == old.canvas {
                    draft_page.canvas = source.canvas;
                }
                if draft_page.width == old.width {
                    draft_page.width = source.width;
                }
                if draft_page.height == old.height {
                    draft_page.height = source.height;
                }
            }
            let raw_widgets = raw["widgets"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("旧版原生组件元数据无效"))?;
            for (widget, raw_widget) in page.widgets.iter_mut().zip(raw_widgets) {
                if raw_widget.get("text_layout").is_none() {
                    let Some(source_widget) = source.widgets.iter().find(|w| w.id == widget.id)
                    else {
                        continue;
                    };
                    if let Some(draft_widget) =
                        draft_page.widgets.iter_mut().find(|w| w.id == widget.id)
                        && draft_widget.text_layout == widget.text_layout
                    {
                        draft_widget.text_layout = source_widget.text_layout.clone();
                    }
                    widget.text_layout = source_widget.text_layout.clone();
                }
            }
        }
        anyhow::ensure!(
            normalized == *fresh,
            "旧版保存检查点与源文件有显示元数据以外的差异"
        );
        let migrated_saved = fresh.clone();
        let mut migrated_state = state.clone();
        migrated_state["project"] = serde_json::to_value(fresh)?;
        // Commit the draft migration only after fingerprint, object, group, and
        // preview bindings have all passed the normal native restore checks.
        document.restore_session_state(&migrated_state)?;
        *draft = migrated_draft;
        *saved = migrated_saved;
        Ok(true)
    } else {
        document.restore_session_state(state)?;
        Ok(false)
    }
}

fn load_document(path: &std::path::Path) -> anyhow::Result<OpenedDocument> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rp" | "rplib" => {
            let result = rpshop::rp_format::open(path)?;
            Ok(OpenedDocument {
                project: result.project.clone(),
                save_path: Some(path.to_path_buf()),
                warnings: result.warnings.clone(),
                native: Some(result),
            })
        }
        "rpshop" | "json" => Ok(OpenedDocument {
            project: Project::load(path)?,
            save_path: Some(path.to_path_buf()),
            warnings: Vec::new(),
            native: None,
        }),
        other => {
            anyhow::bail!("暂不支持 .{other} 文件；请选择 .rp、.rplib 或 .rpshop 工程")
        }
    }
}

pub struct App {
    project: Project,
    saved: Project,
    page: usize,
    selected: BTreeSet<u64>,
    path: Option<PathBuf>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    clipboard: Vec<Widget>,
    zoom: f32,
    view: CanvasView,
    sidebar_layout: sidebar::SidebarLayout,
    fit: bool,
    grid: bool,
    snap: bool,
    tab: usize,
    tool: Option<WidgetKind>,
    drag: Option<Drag>,
    pending_widget: Option<LibraryItem>,
    library_query: String,
    library_category: Option<Category>,
    browser_preview: Option<rpshop::preview::PreviewServer>,
    textures: HashMap<u64, egui::TextureHandle>,
    logo: Option<egui::TextureHandle>,
    status: String,
    last_recovery: Instant,
    screenshot: Option<PathBuf>,
    screenshot_mode: bool,
    history_changed: bool,
    editing: Option<u64>,
    frames: u32,
    confirm: Option<String>,
    help: bool,
    import_warnings: Vec<String>,
    pending_open: Option<PathBuf>,
    native: Option<rpshop::rp_format::NativeDocument>,
    cached_native_preview_source:
        Option<(usize, std::sync::Arc<rpshop::rp_format::NativeDocument>)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<PathBuf>) -> Self {
        setup_style(&cc.egui_ctx);
        let restored: Option<Session> = if screenshot.is_none() {
            cc.storage
                .and_then(|s| eframe::get_value::<Session>(s, "rpshop-session"))
                .map(|mut s| {
                    normalize_draft(&mut s.project);
                    s
                })
                .filter(|s: &Session| s.project.validate().is_ok() && s.saved.validate().is_ok())
        } else {
            None
        };
        let native_state = restored.as_ref().and_then(|s| s.native_state.clone());
        let sidebar_layout = if screenshot.is_none() {
            cc.storage
                .and_then(|s| eframe::get_value(s, sidebar::STORAGE_KEY))
                .unwrap_or_default()
        } else {
            sidebar::SidebarLayout::default()
        };
        let (mut project, mut saved, mut path, mut import_warnings) = restored.map_or_else(
            || {
                let p = Project::default();
                (p.clone(), p, None, Vec::new())
            },
            |s| (s.project, s.saved, s.path, s.import_warnings),
        );
        let args: Vec<String> = std::env::args().collect();
        let startup_file = args
            .windows(2)
            .find(|w| w[0] == "--open")
            .map(|w| w[1].clone())
            .or_else(|| {
                if screenshot.is_none() {
                    args.get(1).filter(|a| !a.starts_with('-')).cloned()
                } else {
                    None
                }
            });
        let mut startup_status = "准备就绪".to_owned();
        let mut native = None;
        if let Some(path) = &path
            && is_native_path(path)
        {
            match rpshop::rp_format::open(path) {
                Ok(mut document) => {
                    let fresh = document.project.clone();
                    match restore_native_context(
                        &mut document,
                        native_state.as_ref(),
                        &mut project,
                        &mut saved,
                    ) {
                        Ok(migrated) => {
                            if migrated {
                                startup_status = "已恢复原生草稿、页面目录和显示设置".into();
                            }
                            native = Some(document);
                        }
                        Err(_) if project == saved => {
                            project = fresh.clone();
                            saved = fresh;
                            import_warnings = document.warnings.clone();
                            native = Some(document);
                            startup_status = "已重新打开原生文件并恢复页面目录".into();
                        }
                        Err(error) => {
                            startup_status = format!(
                                "原生保存上下文未恢复（{error:#}），当前草稿已保留；请另存或重新打开源文件"
                            );
                        }
                    }
                }
                Err(e) => {
                    startup_status = format!("无法恢复 原生保存上下文：{e:#}；可另存当前会话")
                }
            }
        }
        if let Some(file) = startup_file {
            match load_document(std::path::Path::new(&file)) {
                Ok(document) => {
                    project = document.project.clone();
                    saved = document.project;
                    path = document.save_path;
                    import_warnings = document.warnings;
                    native = document.native;
                    startup_status = format!("已打开：{file}");
                }
                Err(e) => startup_status = format!("打开失败：{e:#}"),
            }
        }
        let interaction_example = screenshot.is_some()
            && path.is_none()
            && native.is_none()
            && args.iter().any(|arg| arg == "--interaction-example");
        if interaction_example && let Some(widget) = project.pages[0].widgets.first() {
            let mut event = rpshop::model::AuthoredEvent::new(
                project.pages[0].id,
                None,
                rpshop::model::EventTrigger::PageLoad,
            );
            event.name = "页面载入".into();
            event.actions.push(rpshop::model::Interaction {
                action: Action::SetText,
                target_page: None,
                target_widget: Some(widget.id),
                value: "欢迎使用 RPShop".into(),
            });
            project.interaction_events.push(event);
            saved = project.clone();
        }
        let tab = args
            .windows(2)
            .find(|pair| pair[0] == "--inspector-tab")
            .and_then(|pair| match pair[1].as_str() {
                "style" => Some(0),
                "interaction" => Some(1),
                "notes" => Some(2),
                _ => None,
            })
            .unwrap_or(if interaction_example { 1 } else { 0 });
        let logo = image::load_from_memory(include_bytes!("../assets/branding/rpshop-logo.png"))
            .ok()
            .map(|img| {
                let img = img.into_rgba8();
                cc.egui_ctx.load_texture(
                    "rpshop-logo",
                    egui::ColorImage::from_rgba_unmultiplied(
                        [img.width() as usize, img.height() as usize],
                        &img,
                    ),
                    egui::TextureOptions::LINEAR,
                )
            });
        let screenshot_mode = screenshot.is_some();
        let startup_selection = args
            .windows(2)
            .find(|w| w[0] == "--select")
            .and_then(|w| w[1].parse::<u64>().ok());
        let page = startup_selection
            .and_then(|id| {
                project
                    .pages
                    .iter()
                    .position(|p| p.widgets.iter().any(|w| w.id == id))
            })
            .or_else(|| {
                args.windows(2)
                    .find(|pair| pair[0] == "--page-name")
                    .and_then(|pair| project.pages.iter().position(|page| page.name == pair[1]))
            })
            .unwrap_or(0);
        let selected = startup_selection
            .filter(|id| project.pages[page].widgets.iter().any(|w| w.id == *id))
            .into_iter()
            .collect();
        Self {
            project,
            saved,
            page,
            selected,
            path,
            undo: vec![],
            redo: vec![],
            clipboard: vec![],
            zoom: 0.65,
            view: CanvasView::default(),
            sidebar_layout,
            fit: true,
            grid: native.is_none(),
            snap: true,
            tab,
            tool: None,
            drag: None,
            pending_widget: None,
            library_query: String::new(),
            library_category: None,
            browser_preview: None,
            textures: HashMap::new(),
            logo,
            status: startup_status,
            last_recovery: Instant::now(),
            screenshot,
            screenshot_mode,
            history_changed: false,
            editing: None,
            frames: 0,
            confirm: None,
            help: false,
            import_warnings,
            pending_open: None,
            native,
            cached_native_preview_source: None,
        }
    }

    fn remember(&mut self, before: Project) {
        if let Some(native) = &self.native {
            native.refresh_previews(&mut self.project, &before);
        }
        if before != self.project {
            if self.undo.len() >= 100 {
                self.undo.remove(0);
            }
            self.undo.push(before);
            self.redo.clear();
            self.history_changed = true;
            self.publish_preview();
        }
    }

    fn undo(&mut self) {
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project, p));
            self.page = self.page.min(self.project.pages.len() - 1);
            self.selected.clear();
            self.textures.clear();
            self.status = "已撤销".into();
            self.history_changed = true;
            self.publish_preview();
        }
    }

    fn redo(&mut self) {
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project, p));
            self.page = self.page.min(self.project.pages.len() - 1);
            self.selected.clear();
            self.textures.clear();
            self.status = "已重做".into();
            self.history_changed = true;
            self.publish_preview();
        }
    }

    fn save_project(&mut self, save_as: bool) -> bool {
        let path = if !save_as && self.path.is_some() {
            self.path.clone()
        } else {
            let mut dialog = rfd::FileDialog::new();
            let extension = if self.native.is_some() {
                self.path
                    .as_ref()
                    .and_then(|p| p.extension())
                    .and_then(|e| e.to_str())
                    .unwrap_or("rp")
            } else {
                "rpshop"
            };
            if self.native.is_some() {
                dialog = dialog.add_filter("RP 原生文件", &[extension]);
            }
            dialog
                .add_filter("RPShop 工程", &["rpshop"])
                .set_file_name(format!("{}.{}", self.project.name, extension))
                .save_file()
        };
        let Some(mut path) = path else {
            return false;
        };
        if path.extension().is_none() {
            let extension = if self.native.is_some() {
                self.path
                    .as_ref()
                    .and_then(|p| p.extension())
                    .and_then(|e| e.to_str())
                    .unwrap_or("rp")
            } else {
                "rpshop"
            };
            path.set_extension(extension);
        }
        let native_output = is_native_path(&path);
        let result = self.write_document(&path);
        match result {
            Ok(()) => {
                if native_output && let Some(document) = &self.native {
                    self.project = document.project.clone();
                    self.page = self.page.min(self.project.pages.len() - 1);
                    self.selected.clear();
                    self.textures.clear();
                    self.undo.clear();
                    self.redo.clear();
                    self.history_changed = true;
                }
                self.saved = self.project.clone();
                self.status = format!("已保存：{}", path.display());
                self.path = Some(path);
                if !native_output {
                    self.native = None;
                }
                self.publish_preview();
                true
            }
            Err(e) => {
                self.status = format!("保存失败：{e:#}");
                false
            }
        }
    }

    fn write_document(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        if is_native_path(path) {
            self.native
                .as_mut()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "未加载对应的 RP 原生数据，无法写入此文件；请重新打开源文件或选择 .rpshop"
                    )
                })?
                .save(&self.project, path)
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("rpshop") || e.eq_ignore_ascii_case("json"))
        {
            self.project.save(path)
        } else {
            anyhow::bail!("请选择 .rp、.rplib 或 .rpshop 文件，不能将工程写入其他文件类型")
        }
    }

    fn request_replace(&mut self, action: &str) {
        if self.project != self.saved {
            self.confirm = Some(action.into());
        } else {
            self.replace(action);
        }
    }

    fn replace(&mut self, action: &str) {
        let result = match action {
            "open" => rfd::FileDialog::new()
                .add_filter("RP / RPShop 文件", &["rp", "rplib", "rpshop", "json"])
                .add_filter("RP 工程", &["rp"])
                .add_filter("RP 组件库", &["rplib"])
                .add_filter("RPShop 工程", &["rpshop", "json"])
                .pick_file()
                .map(|p| load_document(&p)),
            "open-path" => self.pending_open.take().map(|p| load_document(&p)),
            "demo" => Some(Ok(OpenedDocument {
                project: Project::default(),
                save_path: None,
                warnings: vec![],
                native: None,
            })),
            _ => Some(Ok(OpenedDocument {
                project: Project::blank(),
                save_path: None,
                warnings: vec![],
                native: None,
            })),
        };
        if let Some(result) = result {
            match result {
                Ok(document) => {
                    self.browser_preview = None;
                    self.cached_native_preview_source = None;
                    let p = document.project;
                    let path = document.save_path;
                    self.import_warnings = document.warnings;
                    self.native = document.native;
                    if self.native.is_some() {
                        self.grid = false;
                    }
                    self.project = p.clone();
                    self.saved = p;
                    self.path = path;
                    self.page = 0;
                    self.selected.clear();
                    self.undo.clear();
                    self.redo.clear();
                    self.textures.clear();
                    self.fit = true;
                    self.history_changed = true;
                    self.status = format!(
                        "已打开 {} 个页面 · {} 个组件{}",
                        self.project.pages.len(),
                        self.project
                            .pages
                            .iter()
                            .map(|p| p.widgets.len())
                            .sum::<usize>(),
                        if self.native.is_some() {
                            " · Ctrl+S 保存原 RP 文件"
                        } else {
                            ""
                        }
                    );
                }
                Err(e) => self.status = format!("打开失败：{e:#}"),
            }
        }
    }

    fn export_file(&mut self, svg: bool) {
        let (ext, name) = if svg {
            ("svg", self.project.pages[self.page].name.clone())
        } else {
            ("html", self.project.name.clone())
        };
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(ext, &[ext])
            .set_file_name(format!("{name}.{ext}"))
            .save_file()
        {
            let result = if svg {
                rpshop::export::svg(&self.project.pages[self.page])
            } else {
                self.native_preview().and_then(|native| {
                    rpshop::export::html_with_native(
                        &self.project,
                        &self.preview_page_keys(),
                        native.as_deref(),
                    )
                })
            };
            match result.and_then(|s| {
                std::fs::write(&path, s)?;
                Ok(())
            }) {
                Ok(()) => self.status = format!("已导出：{}", path.display()),
                Err(e) => self.status = format!("导出失败：{e:#}"),
            }
        }
    }

    fn add_widget(&mut self, kind: WidgetKind, x: f32, y: f32) {
        let id = self.project.allocate_id();
        let mut widget = Widget::new(id, kind, x.max(0.0), y.max(0.0));
        if kind == WidgetKind::Image {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("图片", &["png", "jpg", "jpeg", "gif", "webp"])
                .pick_file()
            else {
                return;
            };
            match std::fs::read(&path)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| {
                    anyhow::ensure!(bytes.len() <= 12 * 1024 * 1024, "图片不能超过 12 MB");
                    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
                        .with_guessed_format()?
                        .into_dimensions()?;
                    anyhow::ensure!(
                        width <= 8192
                            && height <= 8192
                            && u64::from(width) * u64::from(height) <= 32_000_000,
                        "图片尺寸不能超过 8192 px 或 3200 万像素"
                    );
                    let img = image::load_from_memory(&bytes)?;
                    let mut png = std::io::Cursor::new(Vec::new());
                    img.write_to(&mut png, image::ImageFormat::Png)?;
                    anyhow::ensure!(
                        png.get_ref().len() <= 12 * 1024 * 1024,
                        "转换后的图片超过 12 MB，请缩小后再导入"
                    );
                    widget.height =
                        (widget.width * img.height() as f32 / img.width() as f32).max(1.0);
                    widget.image_data = Some(format!(
                        "data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(png.into_inner())
                    ));
                    Ok(())
                }) {
                Ok(()) => {}
                Err(e) => {
                    self.status = format!("图片导入失败：{e:#}");
                    return;
                }
            }
        }
        self.project.pages[self.page].widgets.push(widget);
        self.selected = BTreeSet::from([id]);
        self.status = format!("已添加{}", kind.label());
    }

    fn add_library_item(&mut self, item: LibraryItem, x: f32, y: f32) {
        if item.id == "image" {
            self.add_widget(WidgetKind::Image, x, y);
            return;
        }
        let widgets = item.create(&mut self.project, x.max(0.0), y.max(0.0));
        self.selected = widgets.iter().map(|widget| widget.id).collect();
        self.project.pages[self.page].widgets.extend(widgets);
        self.status = format!("已添加{}", item.label);
    }

    fn delete_selection(&mut self) {
        let ids: BTreeSet<u64> = self.project.pages[self.page]
            .widgets
            .iter()
            .filter(|w| {
                self.selected.contains(&w.id)
                    && !w.locked
                    && self
                        .native
                        .as_ref()
                        .is_none_or(|d| d.is_editable_widget(w.id))
            })
            .map(|w| w.id)
            .collect();
        self.project.pages[self.page]
            .widgets
            .retain(|w| !ids.contains(&w.id));
        for page in &mut self.project.pages {
            for widget in &mut page.widgets {
                if widget
                    .interaction
                    .target_widget
                    .is_some_and(|id| ids.contains(&id))
                {
                    widget.interaction.target_widget = None;
                    widget.interaction.action = Action::None;
                }
            }
        }
        self.project.prune_interaction_references();
        self.selected.clear();
    }

    fn copy(&mut self) {
        self.clipboard = self.project.pages[self.page]
            .widgets
            .iter()
            .filter(|w| self.selected.contains(&w.id))
            .cloned()
            .collect();
        self.status = format!("已复制 {} 个组件", self.clipboard.len());
    }

    fn select_hit(&mut self, id: u64, additive: bool, inside: bool) {
        let id = self
            .native
            .as_ref()
            .map_or(id, |native| native.selection_owner(id));
        let hit = rpshop::grouping::expanded_selection(
            &self.project.pages[self.page],
            &BTreeSet::from([id]),
            inside,
        );
        if !additive {
            self.selected = hit;
        } else if hit.is_subset(&self.selected) {
            self.selected.retain(|id| !hit.contains(id));
        } else {
            self.selected.extend(hit);
        }
    }

    fn group_selection(&mut self, ungroup: bool) {
        let before = self.project.clone();
        if self.native.as_ref().is_some_and(|native| {
            self.selected
                .iter()
                .any(|id| native.selection_owner(*id) == *id && !native.is_editable_widget(*id))
        }) {
            self.status = "当前选择包含只读内容，无法组合或解组".into();
            return;
        }
        let mut draft = before.clone();
        if let Some(native) = &self.native {
            for widget in &mut draft.pages[self.page].widgets {
                if native.selection_owner(widget.id) != widget.id {
                    widget.locked = false;
                }
            }
        }
        let result = if ungroup {
            rpshop::grouping::ungroup(&mut draft, self.page, &self.selected)
        } else {
            rpshop::grouping::group(&mut draft, self.page, &self.selected)
        };
        match result {
            Ok(selected) => {
                for widget in &mut draft.pages[self.page].widgets {
                    if let Some(original) = before.pages[self.page]
                        .widgets
                        .iter()
                        .find(|w| w.id == widget.id)
                    {
                        widget.locked = original.locked;
                    }
                }
                self.project = draft;
                self.selected = selected;
                self.status = if ungroup { "已解组" } else { "已组合" }.into();
                self.remember(before);
            }
            Err(error) => {
                self.status = format!("{}失败：{error:#}", if ungroup { "解组" } else { "组合" })
            }
        }
    }

    fn flip_selection(&mut self, horizontal: bool) {
        let before = self.project.clone();
        let mut draft = before.clone();
        let mut count = 0;
        let mut result = Ok(());
        for widget in &mut draft.pages[self.page].widgets {
            if !self.selected.contains(&widget.id) || widget.locked {
                continue;
            }
            if self
                .native
                .as_ref()
                .is_some_and(|native| !native.is_editable_widget(widget.id))
            {
                continue;
            }
            if self.native.is_none()
                && widget.vector_points.is_none()
                && widget.kind == WidgetKind::Line
            {
                widget.vector_points = Some(
                    serde_json::json!([[{"Path":{"X":0,"Y":0},"HigherSegmentExists":true},{"Path":{"X":1,"Y":1},"HigherSegmentExists":false}]]),
                );
            }
            let is_bitmap = widget.kind == WidgetKind::Image
                && self
                    .native
                    .as_ref()
                    .is_none_or(|native| native.is_bitmap_widget(widget.id));
            if is_bitmap {
                if let Some(data) = &widget.image_data {
                    match rpshop::bitmap::flip_data_uri(data, horizontal) {
                        Ok(data) => {
                            widget.image_data = Some(data);
                            count += 1;
                        }
                        Err(error) => {
                            result = Err(error);
                            break;
                        }
                    }
                }
            } else if widget.vector_points.is_some() {
                if let Err(error) = rpshop::vector_edit::flip_widget(widget, horizontal) {
                    result = Err(error);
                    break;
                }
                count += 1;
            }
        }
        match result {
            Ok(()) if count > 0 => {
                self.project = draft;
                self.textures.clear();
                self.status = format!(
                    "已{}翻转 {count} 个图像或形状",
                    if horizontal { "水平" } else { "垂直" }
                );
                self.remember(before);
            }
            Ok(()) => self.status = "当前选择没有可翻转的图像或矢量路径".into(),
            Err(error) => self.status = format!("翻转失败：{error:#}"),
        }
    }

    fn paste(&mut self) {
        let mut widgets = self.clipboard.clone();
        let mut remap = HashMap::new();
        for w in &mut widgets {
            let id = self.project.allocate_id();
            remap.insert(w.id, id);
            w.id = id;
            w.x += 24.0;
            w.y += 24.0;
            w.name.push_str(" 副本");
            w.locked = false;
        }
        if let Err(error) = rpshop::grouping::remap_groups(&mut widgets, &mut self.project.next_id)
        {
            self.status = format!("粘贴失败：{error:#}");
            return;
        }
        for w in &mut widgets {
            if let Some(id) = w.interaction.target_widget.and_then(|v| remap.get(&v)) {
                w.interaction.target_widget = Some(*id);
            } else if w.interaction.target_widget.is_some_and(|id| {
                !self.project.pages[self.page]
                    .widgets
                    .iter()
                    .any(|x| x.id == id)
            }) {
                w.interaction = Default::default();
            }
        }
        self.selected = widgets.iter().map(|w| w.id).collect();
        self.project.pages[self.page].widgets.extend(widgets);
        self.project.duplicate_interaction_events(
            self.project.pages[self.page].id,
            &remap.into_iter().collect(),
        );
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if let Some(save_as) = Self::consume_save_shortcut(ctx) {
            self.save_project(save_as);
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::O) {
            self.request_replace("open");
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::N) {
            self.request_replace("new");
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::NONE, egui::Key::F5) {
            self.start_preview(ctx);
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::NONE, egui::Key::Escape) {
            self.tool = None;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        for (key, kind) in [
            (egui::Key::R, WidgetKind::Rectangle),
            (egui::Key::O, WidgetKind::Ellipse),
            (egui::Key::T, WidgetKind::Text),
            (egui::Key::L, WidgetKind::Line),
        ] {
            if Self::consume_editor_key(ctx, egui::Modifiers::NONE, key) {
                self.tool = Some(kind);
                self.status = format!("{}工具：在画布中绘制", kind.label());
            }
        }
        for (key, kind) in [
            (egui::Key::B, WidgetKind::Rectangle),
            (egui::Key::E, WidgetKind::Ellipse),
            (egui::Key::T, WidgetKind::Text),
        ] {
            if Self::consume_editor_key(ctx, egui::Modifiers::CTRL | egui::Modifiers::SHIFT, key) {
                self.tool = Some(kind);
                self.status = format!("{}工具：在画布中绘制", kind.label());
            }
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::NONE, egui::Key::V) {
            self.tool = None;
        }
        for (key, mode) in [
            (egui::Key::L, 0),
            (egui::Key::C, 1),
            (egui::Key::R, 2),
            (egui::Key::T, 3),
            (egui::Key::M, 4),
            (egui::Key::B, 5),
        ] {
            if Self::consume_editor_key(ctx, egui::Modifiers::CTRL | egui::Modifiers::ALT, key) {
                self.align(mode);
                return;
            }
        }
        for (key, mode) in [(egui::Key::H, 6), (egui::Key::U, 7)] {
            if Self::consume_editor_key(ctx, egui::Modifiers::CTRL | egui::Modifiers::SHIFT, key) {
                self.align(mode);
                return;
            }
        }
        if Self::consume_editor_key(
            ctx,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            egui::Key::Z,
        ) {
            self.redo();
        } else if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::Z) {
            self.undo();
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::Y) {
            self.redo();
        }
        let before = self.project.clone();
        if Self::consume_editor_key(
            ctx,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            egui::Key::K,
        ) {
            let lock = self.project.pages[self.page].widgets.iter().any(|widget| {
                self.selected.contains(&widget.id)
                    && !widget.locked
                    && self
                        .native
                        .as_ref()
                        .is_none_or(|native| native.is_editable_widget(widget.id))
            });
            for widget in &mut self.project.pages[self.page].widgets {
                if self.selected.contains(&widget.id)
                    && self
                        .native
                        .as_ref()
                        .is_none_or(|native| native.is_editable_widget(widget.id))
                {
                    widget.locked = lock;
                }
            }
            self.status = if lock {
                "已锁定所选组件"
            } else {
                "已解锁所选组件"
            }
            .into();
            self.remember(before);
            return;
        }
        if Self::consume_editor_key(
            ctx,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            egui::Key::G,
        ) {
            self.group_selection(true);
            return;
        } else if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::G) {
            self.group_selection(false);
            return;
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::A) {
            self.selected = self.project.pages[self.page]
                .widgets
                .iter()
                .filter(|w| !w.locked && !w.hidden)
                .map(|w| w.id)
                .collect();
        }
        // Native desktop backends emit clipboard events instead of Key(C/X/V).
        // Leave these events untouched while a text editor owns keyboard focus.
        let (copy, cut, paste) = ctx.input_mut(|input| {
            let mut copy = false;
            let mut cut = false;
            let mut paste = None;
            input.events.retain(|event| match event {
                egui::Event::Copy => {
                    copy = true;
                    false
                }
                egui::Event::Cut => {
                    cut = true;
                    false
                }
                egui::Event::Paste(contents) => {
                    paste = Some(contents.clone());
                    false
                }
                _ => true,
            });
            (copy, cut, paste)
        });
        if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::C) || copy {
            self.copy_to_system(ctx);
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::X) || cut {
            self.cut_components(ctx);
        }
        if let Some(contents) = paste {
            self.paste_components(&contents);
        } else if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::V) {
            self.paste();
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::D) {
            self.copy();
            self.paste();
        }
        if Self::consume_editor_key(ctx, egui::Modifiers::NONE, egui::Key::Delete)
            || Self::consume_editor_key(ctx, egui::Modifiers::NONE, egui::Key::Backspace)
        {
            self.delete_selection();
        }
        for (key, dx, dy) in [
            (egui::Key::ArrowLeft, -1.0, 0.0),
            (egui::Key::ArrowRight, 1.0, 0.0),
            (egui::Key::ArrowUp, 0.0, -1.0),
            (egui::Key::ArrowDown, 0.0, 1.0),
        ] {
            let resize_fast =
                Self::consume_editor_key(ctx, egui::Modifiers::CTRL | egui::Modifiers::SHIFT, key);
            if resize_fast || Self::consume_editor_key(ctx, egui::Modifiers::CTRL, key) {
                let step = if resize_fast { 10.0 } else { 1.0 };
                for widget in &mut self.project.pages[self.page].widgets {
                    if self.selected.contains(&widget.id)
                        && !widget.locked
                        && self
                            .native
                            .as_ref()
                            .is_none_or(|native| native.is_editable_widget(widget.id))
                    {
                        widget.width = (widget.width + dx * step).clamp(1.0, 20_000.0);
                        widget.height = (widget.height + dy * step).clamp(1.0, 20_000.0);
                    }
                }
            }
            let fast = Self::consume_editor_key(ctx, egui::Modifiers::SHIFT, key);
            if fast || Self::consume_editor_key(ctx, egui::Modifiers::NONE, key) {
                let step = if fast { 10.0 } else { 1.0 };
                for w in &mut self.project.pages[self.page].widgets {
                    if self.selected.contains(&w.id)
                        && !w.locked
                        && self
                            .native
                            .as_ref()
                            .is_none_or(|native| native.is_editable_widget(w.id))
                    {
                        w.x += dx * step;
                        w.y += dy * step;
                    }
                }
            }
        }
        self.remember(before);
    }

    fn consume_editor_key(ctx: &egui::Context, modifiers: egui::Modifiers, key: egui::Key) -> bool {
        ctx.input_mut(|input| {
            let mut consumed = false;
            // egui-winit recognizes any Ctrl+C as Copy, including Ctrl+Alt+C.
            // Recover the editor's horizontal-center shortcut before treating
            // the remaining native Copy event as an ordinary clipboard action.
            let native_center = key == egui::Key::C
                && modifiers == (egui::Modifiers::CTRL | egui::Modifiers::ALT)
                && input.modifiers.matches_exact(modifiers);
            input.events.retain(|event| {
                let matches = matches!(event, egui::Event::Key {
                    key: pressed_key,
                    pressed: true,
                    modifiers: pressed_modifiers,
                    ..
                } if *pressed_key == key && pressed_modifiers.matches_exact(modifiers))
                    || (native_center && matches!(event, egui::Event::Copy));
                consumed |= matches;
                !matches
            });
            consumed
        })
    }

    fn consume_save_shortcut(ctx: &egui::Context) -> Option<bool> {
        if Self::consume_editor_key(
            ctx,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            egui::Key::S,
        ) {
            Some(true)
        } else if Self::consume_editor_key(ctx, egui::Modifiers::CTRL, egui::Key::S) {
            Some(false)
        } else {
            None
        }
    }

    fn write_component_clipboard(&self, ctx: &egui::Context) {
        if !self.clipboard.is_empty()
            && let Ok(contents) = serde_json::to_string(&self.clipboard)
        {
            ctx.copy_text(format!("RPShop.components.v1\n{contents}"));
        }
    }

    fn copy_to_system(&mut self, ctx: &egui::Context) {
        self.copy();
        self.write_component_clipboard(ctx);
    }

    fn cut_components(&mut self, ctx: &egui::Context) {
        self.clipboard = self.project.pages[self.page]
            .widgets
            .iter()
            .filter(|widget| {
                self.selected.contains(&widget.id)
                    && !widget.locked
                    && self
                        .native
                        .as_ref()
                        .is_none_or(|native| native.is_editable_widget(widget.id))
            })
            .cloned()
            .collect();
        if self.clipboard.is_empty() {
            self.status = "当前选择没有可剪切的组件".into();
            return;
        }
        self.write_component_clipboard(ctx);
        let count = self.clipboard.len();
        self.delete_selection();
        self.status = format!("已剪切 {count} 个组件");
    }

    fn paste_components(&mut self, contents: &str) {
        let Some(contents) = contents.strip_prefix("RPShop.components.v1\n") else {
            self.status = "剪贴板中没有可粘贴的组件".into();
            return;
        };
        if contents.len() > 48 * 1024 * 1024 {
            self.status = "剪贴板组件超过 48 MB 限制".into();
            return;
        }
        let widgets = match serde_json::from_str::<Vec<Widget>>(contents) {
            Ok(widgets) if !widgets.is_empty() && widgets.len() <= 5_000 => widgets,
            _ => {
                self.status = "剪贴板组件数据无效".into();
                return;
            }
        };
        let extra_ids = widgets
            .iter()
            .map(|widget| widget.groups.len() as u64 + 1)
            .sum::<u64>();
        if self.project.next_id.checked_add(extra_ids + 1).is_none() {
            self.status = "组件标识已耗尽，无法粘贴".into();
            return;
        }
        let before = self.project.clone();
        let selected = self.selected.clone();
        let expected_count = before.pages[self.page].widgets.len() + widgets.len();
        let old_clipboard = std::mem::replace(&mut self.clipboard, widgets);
        self.paste();
        if self.project.pages[self.page].widgets.len() != expected_count {
            self.project = before;
            self.selected = selected;
            self.clipboard = old_clipboard;
            return;
        }
        // A copied navigation target may belong to another document.
        let pages: BTreeSet<u64> = self.project.pages.iter().map(|page| page.id).collect();
        for widget in &mut self.project.pages[self.page].widgets {
            if self.selected.contains(&widget.id)
                && widget
                    .interaction
                    .target_page
                    .is_some_and(|id| !pages.contains(&id))
            {
                widget.interaction = Default::default();
            }
        }
        if let Err(error) = self.project.validate() {
            self.project = before;
            self.selected = selected;
            self.clipboard = old_clipboard;
            self.status = format!("粘贴失败：{error:#}");
        } else {
            self.status = format!("已粘贴 {} 个组件", self.selected.len());
        }
    }

    fn publish_preview(&mut self) {
        if self.native.is_none() {
            self.cached_native_preview_source = None;
        }
        if self.browser_preview.is_none() {
            return;
        }
        let keys = self.preview_page_keys();
        let native = self.native_source_snapshot();
        if let Some(server) = &mut self.browser_preview
            && let Err(error) =
                server.publish_with_native_document(self.project.clone(), keys, native)
        {
            self.status = format!("预览尚未更新：{error:#}");
        }
    }

    fn native_source_snapshot(
        &mut self,
    ) -> Option<std::sync::Arc<rpshop::rp_format::NativeDocument>> {
        let Some(native) = &self.native else {
            self.cached_native_preview_source = None;
            return None;
        };
        let identity = native.preview_identity();
        if let Some((cached_identity, source)) = &self.cached_native_preview_source
            && *cached_identity == identity
        {
            return Some(std::sync::Arc::clone(source));
        }
        let source = std::sync::Arc::new(native.clone());
        self.cached_native_preview_source = Some((identity, std::sync::Arc::clone(&source)));
        Some(source)
    }

    fn native_preview(
        &self,
    ) -> anyhow::Result<Option<std::sync::Arc<rpshop::rp_format::NativePreview>>> {
        self.native
            .as_ref()
            .map(|native| native.preview_scene(&self.project).map(std::sync::Arc::new))
            .transpose()
    }

    fn preview_page_keys(&self) -> std::collections::BTreeMap<u64, String> {
        self.native
            .as_ref()
            .map_or_else(Default::default, |native| native.preview_page_keys())
    }

    fn start_preview(&mut self, ctx: &egui::Context) {
        let result = (|| -> anyhow::Result<String> {
            let keys = self.preview_page_keys();
            let native = self.native_source_snapshot();
            if let Some(server) = &mut self.browser_preview {
                server.publish_with_native_document(self.project.clone(), keys, native)?;
            } else {
                self.browser_preview =
                    Some(rpshop::preview::PreviewServer::start_with_native_document(
                        self.project.clone(),
                        keys,
                        native,
                    )?);
            }
            Ok(self
                .browser_preview
                .as_ref()
                .unwrap()
                .url(self.project.pages[self.page].id))
        })();
        match result {
            Ok(url) => {
                ctx.open_url(egui::OpenUrl::new_tab(url));
                self.status = "已在浏览器预览当前页；编辑后刷新浏览器查看最新内容".into();
            }
            Err(error) => self.status = format!("预览失败：{error:#}"),
        }
    }

    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(18, 10)),
            )
            .show(ctx, |ui| {
                if self.confirm.is_some() || self.editing.is_some() {
                    ui.disable();
                }
                ui.horizontal(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.image((logo.id(), vec2(38.0, 38.0)));
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new("rpshop").size(22.0).strong().color(INK));
                        ui.label(RichText::new("PROTOTYPE STUDIO").size(9.0).color(MUTED));
                    });
                    ui.add_space(24.0);
                    ui.label(
                        RichText::new("● 工作空间")
                            .color(Color32::from_rgb(71, 153, 130))
                            .size(12.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("▶  预览原型").color(Color32::WHITE),
                                )
                                .fill(PURPLE)
                                .min_size(vec2(118.0, 34.0)),
                            )
                            .clicked()
                        {
                            self.start_preview(ctx);
                        }
                        if ui.button("导出 HTML").clicked() {
                            self.export_file(false);
                        }
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(if self.project != self.saved {
                                "● 未保存"
                            } else if self.path.is_some() {
                                "已保存"
                            } else {
                                "未保存为文件"
                            })
                            .color(MUTED)
                            .size(11.0),
                        );
                        ui.add_sized(
                            [220.0, 24.0],
                            egui::Label::new(RichText::new(&self.project.name).strong()).truncate(),
                        )
                        .on_hover_text(&self.project.name);
                    });
                });
            });
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(egui::Margin::symmetric(18, 7)),
            )
            .show(ctx, |ui| {
                if self.confirm.is_some() || self.editing.is_some() {
                    ui.disable();
                }
                ui.horizontal(|ui| {
                    ui.menu_button("文件", |ui| {
                        if ui.button("新建工程       Ctrl+N").clicked() {
                            self.request_replace("new");
                            ui.close();
                        }
                        if ui.button("打开工程       Ctrl+O").clicked() {
                            self.request_replace("open");
                            ui.close();
                        }
                        if ui.button("保存              Ctrl+S").clicked() {
                            self.save_project(false);
                            ui.close();
                        }
                        if ui.button("另存为…").clicked() {
                            self.save_project(true);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("导出交互 HTML…").clicked() {
                            self.export_file(false);
                            ui.close();
                        }
                        if ui.button("导出当前页面 SVG…").clicked() {
                            self.export_file(true);
                            ui.close();
                        }
                        if ui.button("打开欢迎示例").clicked() {
                            self.request_replace("demo");
                            ui.close();
                        }
                    });
                    if ui
                        .add_enabled(!self.undo.is_empty(), egui::Button::new("撤销"))
                        .on_hover_text("撤销 Ctrl+Z")
                        .clicked()
                    {
                        self.undo();
                    }
                    if ui
                        .add_enabled(!self.redo.is_empty(), egui::Button::new("重做"))
                        .on_hover_text("重做 Ctrl+Y")
                        .clicked()
                    {
                        self.redo();
                    }
                    ui.separator();
                    if ui
                        .selectable_label(self.tool.is_none(), "↖  选择")
                        .clicked()
                    {
                        self.tool = None;
                    }
                    for (kind, label) in [
                        (WidgetKind::Rectangle, "□"),
                        (WidgetKind::Text, "T"),
                        (WidgetKind::Ellipse, "○"),
                        (WidgetKind::Line, "╱"),
                    ] {
                        if ui
                            .selectable_label(self.tool == Some(kind), label)
                            .on_hover_text(format!(
                                "绘制{}：拖动绘制，单击按默认尺寸放置",
                                kind.label()
                            ))
                            .clicked()
                        {
                            self.tool = Some(kind);
                        }
                    }
                    ui.separator();
                    let before = self.project.clone();
                    if ui
                        .add_enabled(!self.selected.is_empty(), egui::Button::new("复制"))
                        .clicked()
                    {
                        self.copy_to_system(ctx);
                    }
                    if ui.button("粘贴").on_hover_text("Ctrl+V").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    }
                    if ui
                        .add_enabled(!self.selected.is_empty(), egui::Button::new("删除"))
                        .clicked()
                    {
                        self.delete_selection();
                    }
                    self.remember(before);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("?").clicked() {
                            self.help = true;
                        }
                        ui.menu_button("视图", |ui| {
                            for (label, command) in [
                                ("放大  Ctrl++", ViewCommand::Step(0.1)),
                                ("缩小  Ctrl+−", ViewCommand::Step(-0.1)),
                                ("100%  Ctrl+0", ViewCommand::Zoom(1.0)),
                                ("居中画布  Ctrl+1", ViewCommand::CenterPage),
                                ("适配选择  Ctrl+2", ViewCommand::FitSelection),
                                ("居中选择  Ctrl+3", ViewCommand::CenterSelection),
                                ("回到原点  Ctrl+9", ViewCommand::Origin),
                            ] {
                                if ui.button(label).clicked() {
                                    self.view.pending = Some(command);
                                    self.fit = false;
                                    ui.close();
                                }
                            }
                            ui.separator();
                            ui.label("Ctrl+滚轮缩放 · 空格拖动平移");
                        });
                        if ui.button("适应").clicked() {
                            self.fit = true;
                        }
                        egui::ComboBox::from_id_salt("zoom")
                            .selected_text(format!("{:.0}%", self.zoom * 100.0))
                            .width(65.0)
                            .show_ui(ui, |ui| {
                                for z in [
                                    0.1, 0.25, 0.5, 0.65, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 5.0, 10.0,
                                ] {
                                    if ui
                                        .selectable_label(
                                            (self.zoom - z).abs() < 0.01,
                                            format!("{:.0}%", z * 100.0),
                                        )
                                        .clicked()
                                    {
                                        self.view.pending = Some(ViewCommand::Zoom(z));
                                        self.fit = false;
                                    }
                                }
                            });
                        ui.checkbox(&mut self.snap, "吸附");
                        ui.checkbox(&mut self.grid, "网格");
                    });
                });
            });
    }

    fn left_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigator")
            .resizable(true)
            .default_width(225.0)
            .width_range(195.0..=320.0)
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(14.0))
            .show(ctx, |ui| {
                if self.confirm.is_some() || self.editing.is_some() {
                    ui.disable();
                }
                let rects = self.sidebar_layout.show(ui);
                self.pages_pane(&mut sidebar::pane(ui, rects.panes[0], "pages-pane"));
                self.library_pane(&mut sidebar::pane(ui, rects.panes[1], "library-pane"));
                self.layers_pane(&mut sidebar::pane(ui, rects.panes[2], "layers-pane"));
            });
    }

    fn pages_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("页面").strong().size(14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(self.native.is_none(), egui::Button::new("＋").small())
                    .on_hover_text("新建页面")
                    .on_disabled_hover_text(
                        "当前 RP 原生编辑支持修改已有页面和组件，暂不支持新增页面",
                    )
                    .clicked()
                {
                    let id = self.project.allocate_id();
                    self.project.pages.push(Page {
                        id,
                        name: format!("页面 {}", self.project.pages.len() + 1),
                        width: 1280.0,
                        height: 800.0,
                        canvas: rpshop::model::CanvasMode::Fixed,
                        background: "#FFFFFF".into(),
                        widgets: vec![],
                    });
                    if !self.project.page_tree.is_empty() {
                        self.project
                            .page_tree
                            .push(rpshop::model::PageTreeNode::Page {
                                page_id: id,
                                children: Vec::new(),
                            });
                    }
                    self.page = self.project.pages.len() - 1;
                    self.selected.clear();
                    self.fit = true;
                }
            });
        });
        ui.add_space(9.0);
        let navigation = page_tree::show(ui, &self.project, self.page, self.native.is_none());
        if let Some(i) = navigation.activate {
            self.page = i;
            self.selected.clear();
            self.fit = true;
        }
        if let Some(i) = navigation.remove {
            let p = self.project.pages.remove(i);
            page_tree::remove_entry(&mut self.project.page_tree, p.id);
            let ids: BTreeSet<_> = p.widgets.iter().map(|w| w.id).collect();
            for p2 in &mut self.project.pages {
                for w in &mut p2.widgets {
                    if w.interaction.target_page == Some(p.id)
                        || w.interaction
                            .target_widget
                            .is_some_and(|id| ids.contains(&id))
                    {
                        w.interaction = Default::default();
                    }
                }
            }
            self.project.prune_interaction_references();
            self.page = self.page.min(self.project.pages.len() - 1);
            self.selected.clear();
        }
    }

    fn library_pane(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("组件库").strong().size(14.0));
            ui.label(
                RichText::new(widget_library::ITEMS.len().to_string())
                    .small()
                    .color(MUTED),
            );
        });
        ui.label(
            RichText::new("点击添加，或拖入画布")
                .size(11.0)
                .color(MUTED),
        );
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.library_query)
                .id_salt("library-search")
                .hint_text("搜索元件…")
                .desired_width(f32::INFINITY),
        );
        egui::ComboBox::from_id_salt("library-category")
            .selected_text(self.library_category.map_or("全部元件", Category::label))
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.library_category, None, "全部元件");
                for category in Category::ALL {
                    ui.selectable_value(
                        &mut self.library_category,
                        Some(category),
                        category.label(),
                    );
                }
            });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("library-scroll")
            .auto_shrink([false, false])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                let columns = ((ui.available_width() + 5.0) / 64.0)
                    .floor()
                    .clamp(1.0, 3.0) as usize;
                let mut found = false;
                for category in Category::ALL {
                    if self
                        .library_category
                        .is_some_and(|selected| selected != category)
                    {
                        continue;
                    }
                    let items: Vec<_> = widget_library::ITEMS
                        .iter()
                        .copied()
                        .filter(|item| {
                            item.category == category && item.matches(&self.library_query)
                        })
                        .collect();
                    if items.is_empty() {
                        continue;
                    }
                    found = true;
                    ui.label(
                        RichText::new(format!("{}  {}", category.label(), items.len()))
                            .size(11.0)
                            .color(MUTED),
                    );
                    egui::Grid::new(("library", category.label()))
                        .num_columns(columns)
                        .spacing([5.0, 6.0])
                        .show(ui, |ui| {
                            for (i, item) in items.into_iter().enumerate() {
                                let r = library_button(ui, item);
                                if r.clicked() {
                                    self.add_library_item(
                                        item,
                                        64.0 + (self.project.pages[self.page].widgets.len() % 6)
                                            as f32
                                            * 16.0,
                                        80.0,
                                    );
                                }
                                if r.drag_started() {
                                    self.pending_widget = Some(item);
                                }
                                if i % columns == columns - 1 {
                                    ui.end_row();
                                }
                            }
                        });
                    ui.add_space(8.0);
                }
                if !found {
                    ui.label(RichText::new("没有匹配的元件").color(MUTED));
                }
            });
    }

    fn layers_pane(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = LAYER_COLUMN_SPACING;
            let title_width = layer_title_width(ui.available_width());
            ui.allocate_ui_with_layout(
                vec2(title_width, 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(title_width);
                    ui.label(RichText::new("图层").strong().size(14.0));
                    ui.label(
                        RichText::new(self.project.pages[self.page].widgets.len().to_string())
                            .small()
                            .color(MUTED),
                    );
                },
            );
            ui.add_sized(
                [LAYER_CONTROL_WIDTH, 22.0],
                egui::Label::new(RichText::new("显示").size(11.0).color(MUTED)),
            )
            .on_hover_text("眼睛控制组件在画布和预览中的显示；隐藏不会删除组件。");
            ui.add_sized(
                [LAYER_CONTROL_WIDTH, 22.0],
                egui::Label::new(RichText::new("锁定").size(11.0).color(MUTED)),
            )
            .on_hover_text("锁定后不能移动、缩放或修改组件；点击开锁即可恢复编辑。");
        });
        ui.label(
            RichText::new("眼睛控制显示，锁控制编辑")
                .size(11.0)
                .color(MUTED),
        );
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .id_salt("layers")
            .auto_shrink([false, false])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                let mut layer_hit = None;
                for widget in self.project.pages[self.page].widgets.iter_mut().rev() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = LAYER_COLUMN_SPACING;
                        let title_width = layer_title_width(ui.available_width());
                        let r = ui
                            .add_sized(
                                [title_width, 27.0],
                                egui::Button::selectable(
                                    self.selected.contains(&widget.id),
                                    format!(
                                        "{} {}",
                                        if widget.kind == WidgetKind::Text {
                                            "T"
                                        } else {
                                            "◇"
                                        },
                                        widget.name
                                    ),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&widget.name);
                        if r.clicked() {
                            layer_hit = Some((
                                widget.id,
                                ui.input(|i| i.modifiers.shift || i.modifiers.ctrl),
                                ui.input(|i| i.modifiers.ctrl && i.modifiers.alt),
                            ));
                        }
                        let editable = self
                            .native
                            .as_ref()
                            .is_none_or(|d| d.is_editable_widget(widget.id));
                        ui.add_enabled_ui(editable, |ui| {
                            if layer_control(
                                ui,
                                LayerControl::Visibility {
                                    hidden: widget.hidden,
                                },
                                &widget.name,
                            )
                            .clicked()
                            {
                                widget.hidden = !widget.hidden;
                            }
                            if layer_control(
                                ui,
                                LayerControl::Lock {
                                    locked: widget.locked,
                                },
                                &widget.name,
                            )
                            .clicked()
                            {
                                widget.locked = !widget.locked;
                            }
                        })
                        .response
                        .on_hover_text(if editable {
                            "显示与锁定"
                        } else {
                            "组件名称标签仅用于浏览，不属于原生组件内容"
                        });
                    });
                }
                if let Some((id, additive, inside)) = layer_hit {
                    self.select_hit(id, additive, inside);
                }
            });
    }

    fn inspector(&mut self, ctx: &egui::Context) {
        self.inspector_panel(ctx);
    }

    fn align(&mut self, mode: usize) {
        let editable_ids: BTreeSet<u64> = self.project.pages[self.page]
            .widgets
            .iter()
            .filter(|widget| {
                self.selected.contains(&widget.id)
                    && !widget.locked
                    && self
                        .native
                        .as_ref()
                        .is_none_or(|document| document.is_editable_widget(widget.id))
            })
            .map(|widget| widget.id)
            .collect();
        if editable_ids.is_empty() || mode > 13 {
            return;
        }
        let before = self.project.clone();
        let widgets = &mut self.project.pages[self.page].widgets;
        match mode {
            0..=7 => {
                use std::collections::BTreeMap;
                let owner = |id| {
                    self.native
                        .as_ref()
                        .map_or(id, |native| native.selection_owner(id))
                };
                // Read-only master contents belong to their selected instance;
                // their synthetic locks must not freeze its enclosing group.
                let mut covered = self.selected.clone();
                for widget in widgets.iter() {
                    if owner(widget.id) != widget.id && self.selected.contains(&owner(widget.id)) {
                        covered.insert(widget.id);
                    }
                }
                let mut group_counts: BTreeMap<u64, (usize, usize)> = BTreeMap::new();
                for widget in widgets.iter() {
                    for group in &widget.groups {
                        let counts = group_counts.entry(*group).or_default();
                        counts.0 += 1;
                        counts.1 += usize::from(covered.contains(&widget.id));
                    }
                }
                let complete_groups = group_counts
                    .into_iter()
                    .filter_map(|(id, (total, chosen))| (total == chosen).then_some(id))
                    .collect::<BTreeSet<_>>();
                let mut unit_order = Vec::new();
                let mut unit_members: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
                let mut widget_units = BTreeMap::new();
                for (index, widget) in widgets.iter().enumerate() {
                    if !self.selected.contains(&widget.id) || owner(widget.id) != widget.id {
                        continue;
                    }
                    let unit = widget
                        .groups
                        .iter()
                        .find(|group| complete_groups.contains(group))
                        .copied()
                        .unwrap_or(widget.id);
                    if !unit_members.contains_key(&unit) {
                        unit_order.push(unit);
                    }
                    unit_members.entry(unit).or_default().push(index);
                    widget_units.insert(widget.id, unit);
                }
                let mut unit_bounds: BTreeMap<u64, [f32; 4]> = BTreeMap::new();
                for widget in widgets.iter() {
                    let Some(unit) = widget_units.get(&owner(widget.id)) else {
                        continue;
                    };
                    if !complete_groups.contains(unit) {
                        continue;
                    }
                    let next = rpshop::manipulation::rotated_bounds(widget);
                    let bounds = unit_bounds.entry(*unit).or_insert(next);
                    bounds[0] = bounds[0].min(next[0]);
                    bounds[1] = bounds[1].min(next[1]);
                    bounds[2] = bounds[2].max(next[2]);
                    bounds[3] = bounds[3].max(next[3]);
                }
                let mut proxies = Vec::with_capacity(unit_order.len());
                for unit in &unit_order {
                    let members = &unit_members[unit];
                    let mut proxy = widgets[members[0]].clone();
                    proxy.id = *unit;
                    proxy.locked = members
                        .iter()
                        .any(|index| !editable_ids.contains(&widgets[*index].id));
                    if let Some(bounds) = unit_bounds.get(unit) {
                        proxy.x = bounds[0];
                        proxy.y = bounds[1];
                        proxy.width = bounds[2] - bounds[0];
                        proxy.height = bounds[3] - bounds[1];
                        proxy.rotation = 0.0;
                    }
                    proxies.push(proxy);
                }
                let initial_positions = proxies
                    .iter()
                    .map(|proxy| (proxy.id, [proxy.x, proxy.y]))
                    .collect::<BTreeMap<_, _>>();
                let proxy_ids = unit_order.into_iter().collect::<BTreeSet<_>>();
                if mode < 6 {
                    let modes = [
                        rpshop::manipulation::Align::Left,
                        rpshop::manipulation::Align::Center,
                        rpshop::manipulation::Align::Right,
                        rpshop::manipulation::Align::Top,
                        rpshop::manipulation::Align::Middle,
                        rpshop::manipulation::Align::Bottom,
                    ];
                    rpshop::manipulation::align(&mut proxies, &proxy_ids, modes[mode]);
                } else {
                    rpshop::manipulation::distribute(&mut proxies, &proxy_ids, mode == 6);
                }
                for proxy in proxies {
                    let initial = initial_positions[&proxy.id];
                    let delta = [proxy.x - initial[0], proxy.y - initial[1]];
                    for index in &unit_members[&proxy.id] {
                        let widget = &mut widgets[*index];
                        if editable_ids.contains(&widget.id) {
                            widget.x += delta[0];
                            widget.y += delta[1];
                        }
                    }
                }
            }
            8..=11 => {
                let mut staged = before.pages[self.page].clone();
                for widget in &mut staged.widgets {
                    if !self.selected.contains(&widget.id) {
                        continue;
                    }
                    if let Some(native) = &self.native {
                        let owner = native.selection_owner(widget.id);
                        if owner != widget.id
                            && self.selected.contains(&owner)
                            && editable_ids.contains(&owner)
                        {
                            widget.locked = false;
                        } else if !native.is_editable_widget(widget.id) {
                            widget.locked = true;
                        }
                    }
                }
                let movement = match mode {
                    8 => rpshop::selection::LayerMove::ToFront,
                    9 => rpshop::selection::LayerMove::Forward,
                    10 => rpshop::selection::LayerMove::Backward,
                    _ => rpshop::selection::LayerMove::ToBack,
                };
                if rpshop::grouping::reorder(&mut staged, &self.selected, movement) {
                    let locks = widgets
                        .iter()
                        .map(|widget| (widget.id, widget.locked))
                        .collect::<std::collections::BTreeMap<_, _>>();
                    for widget in &mut staged.widgets {
                        widget.locked = locks[&widget.id];
                    }
                    *widgets = staged.widgets;
                }
            }
            12 | 13 => {
                let bounds = widgets
                    .iter()
                    .filter(|widget| self.selected.contains(&widget.id))
                    .fold(
                        [
                            f32::INFINITY,
                            f32::INFINITY,
                            f32::NEG_INFINITY,
                            f32::NEG_INFINITY,
                        ],
                        |mut bounds, widget| {
                            let next = rpshop::manipulation::rotated_bounds(widget);
                            bounds[0] = bounds[0].min(next[0]);
                            bounds[1] = bounds[1].min(next[1]);
                            bounds[2] = bounds[2].max(next[2]);
                            bounds[3] = bounds[3].max(next[3]);
                            bounds
                        },
                    );
                let pivot = [(bounds[0] + bounds[2]) / 2.0, (bounds[1] + bounds[3]) / 2.0];
                for widget in widgets {
                    if editable_ids.contains(&widget.id) {
                        *widget = rpshop::manipulation::rotate(
                            widget,
                            if mode == 12 { -90.0 } else { 90.0 },
                            pivot,
                        );
                    }
                }
            }
            _ => {}
        }
        if before != self.project {
            self.status = [
                "已左对齐",
                "已水平居中",
                "已右对齐",
                "已顶对齐",
                "已垂直居中",
                "已底对齐",
                "已水平分布",
                "已垂直分布",
                "已置于顶层",
                "已上移一层",
                "已下移一层",
                "已置于底层",
                "已逆时针旋转 90°",
                "已顺时针旋转 90°",
            ][mode]
                .into();
            self.remember(before);
        }
    }

    fn load_textures(&mut self, ctx: &egui::Context) {
        let pages = BTreeSet::from([self.page]);
        let needed: BTreeSet<u64> = pages
            .iter()
            .filter_map(|i| self.project.pages.get(*i))
            .flat_map(|p| &p.widgets)
            .map(|w| w.id)
            .collect();
        self.textures.retain(|id, _| needed.contains(id));
        for w in pages
            .iter()
            .filter_map(|i| self.project.pages.get(*i))
            .flat_map(|p| &p.widgets)
        {
            if w.kind != WidgetKind::Image || self.textures.contains_key(&w.id) {
                continue;
            }
            if let Some(data) = &w.image_data
                && let Some((_, payload)) = data.split_once(',')
                && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(payload)
                && let Ok(img) = image::load_from_memory(&bytes)
            {
                let img = img.into_rgba8();
                let tex = ctx.load_texture(
                    format!("widget-{}", w.id),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [img.width() as usize, img.height() as usize],
                        &img,
                    ),
                    egui::TextureOptions::LINEAR,
                );
                self.textures.insert(w.id, tex);
            }
        }
    }

    fn viewport_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || self.drag.is_some() {
            return;
        }
        let ctrl = egui::Modifiers::CTRL;
        for (key, command) in [
            (egui::Key::Num0, ViewCommand::Zoom(1.0)),
            (egui::Key::Num1, ViewCommand::CenterPage),
            (egui::Key::Num2, ViewCommand::FitSelection),
            (egui::Key::Num3, ViewCommand::CenterSelection),
            (egui::Key::Num9, ViewCommand::Origin),
        ] {
            if Self::consume_editor_key(ctx, ctrl, key) {
                self.view.pending = Some(command);
                self.fit = false;
            }
        }
        if Self::consume_editor_key(ctx, ctrl, egui::Key::Plus)
            || Self::consume_editor_key(ctx, ctrl, egui::Key::Equals)
            || Self::consume_editor_key(ctx, ctrl | egui::Modifiers::SHIFT, egui::Key::Plus)
            || Self::consume_editor_key(ctx, ctrl | egui::Modifiers::SHIFT, egui::Key::Equals)
        {
            self.view.pending = Some(ViewCommand::Step(0.1));
            self.fit = false;
        }
        if Self::consume_editor_key(ctx, ctrl, egui::Key::Minus) {
            self.view.pending = Some(ViewCommand::Step(-0.1));
            self.fit = false;
        }
    }

    fn navigate_canvas(&mut self, ui: &egui::Ui, ctx: &egui::Context, viewport: Rect) -> bool {
        if !ui.is_enabled() {
            self.view.panning = None;
            return true;
        }
        let page = &self.project.pages[self.page];
        let output_size = page.size_for_output();
        let page_bounds = Rect::from_min_size(Pos2::ZERO, vec2(output_size[0], output_size[1]));
        if self.fit {
            if page.canvas.is_auto() {
                self.zoom = 1.0;
                self.view.pan = vec2(viewport::RULER_SIZE, viewport::RULER_SIZE);
            } else {
                self.view.fit(&mut self.zoom, viewport, page_bounds);
                self.zoom = self.zoom.min(1.0);
                self.view.center(self.zoom, viewport, page_bounds);
            }
            self.fit = false;
        }
        if let Some(command) = self.view.pending.take() {
            let selected = selection_geometry(page, &self.selected).map(|w| {
                let bounds = manipulation::rotated_bounds(&w);
                Rect::from_min_max(
                    Pos2::new(bounds[0], bounds[1]),
                    Pos2::new(bounds[2], bounds[3]),
                )
            });
            match command {
                ViewCommand::Step(step) => {
                    let target = self.zoom + step;
                    self.view
                        .zoom_at(&mut self.zoom, target, viewport.size() / 2.0);
                }
                ViewCommand::Zoom(target) => {
                    self.view
                        .zoom_at(&mut self.zoom, target, viewport.size() / 2.0)
                }
                ViewCommand::CenterPage => self.view.center(self.zoom, viewport, page_bounds),
                ViewCommand::FitSelection => {
                    if let Some(bounds) = selected {
                        self.view.fit(&mut self.zoom, viewport, bounds);
                    }
                }
                ViewCommand::CenterSelection => {
                    if let Some(bounds) = selected {
                        self.view.center(self.zoom, viewport, bounds);
                    }
                }
                ViewCommand::Origin => {
                    self.view.pan = if page.canvas.is_auto() {
                        vec2(viewport::RULER_SIZE, viewport::RULER_SIZE)
                    } else {
                        vec2(40.0, 40.0)
                    }
                }
            }
        }
        let hovered = ui.rect_contains_pointer(viewport);
        let space = !ctx.wants_keyboard_input() && ctx.input(|i| i.key_down(egui::Key::Space));
        let was_panning = self.view.panning.is_some();
        if hovered && self.drag.is_none() {
            let pointer = ctx.input(|i| i.pointer.interact_pos());
            let button = if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle)) {
                Some(egui::PointerButton::Middle)
            } else if space && ctx.input(|i| i.pointer.primary_pressed()) {
                Some(egui::PointerButton::Primary)
            } else {
                None
            };
            if let (Some(button), Some(pointer)) = (button, pointer) {
                self.view.panning = Some(PanDrag {
                    pointer,
                    pan: self.view.pan,
                    button,
                });
            }
        }
        if let Some(drag) = &self.view.panning {
            if let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()) {
                self.view.pan = drag.pan + (pointer - drag.pointer);
            }
            if !ctx.input(|i| i.pointer.button_down(drag.button)) {
                self.view.panning = None;
            }
        }
        let navigating = was_panning
            || self.view.panning.is_some()
            || (space && hovered)
            || (hovered && ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle)));
        if hovered && self.drag.is_none() && self.view.panning.is_none() {
            let pointer = ctx
                .input(|i| i.pointer.hover_pos())
                .unwrap_or(viewport.center());
            for event in ctx.input(|i| i.events.clone()) {
                match event {
                    egui::Event::MouseWheel {
                        unit,
                        delta,
                        modifiers,
                    } => self.view.wheel(
                        &mut self.zoom,
                        unit,
                        delta,
                        modifiers,
                        pointer - viewport.min,
                        viewport,
                    ),
                    egui::Event::Zoom(factor) if factor.is_finite() && factor > 0.0 => {
                        let target = self.zoom * factor;
                        self.view
                            .zoom_at(&mut self.zoom, target, pointer - viewport.min);
                    }
                    _ => {}
                }
            }
        }
        if navigating {
            ctx.set_cursor_icon(if self.view.panning.is_some() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        navigating
    }

    fn canvas(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::from_rgb(243, 244, 248)))
            .show(ctx, |ui| {
                if self.confirm.is_some() || self.editing.is_some() {
                    ui.disable();
                }
                let auto_canvas = self.project.pages[self.page].canvas.is_auto();
                if !auto_canvas {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}  /  {}",
                                self.project.name, self.project.pages[self.page].name
                            ))
                            .size(11.0)
                            .color(MUTED),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{:.0} × {:.0}",
                                    self.project.pages[self.page].width,
                                    self.project.pages[self.page].height
                                ))
                                .size(11.0)
                                .color(MUTED),
                            );
                        });
                    });
                    ui.add_space(8.0);
                }
                let size = ui.available_size().max(vec2(1.0, 1.0));
                let (workspace, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
                let navigating = self.navigate_canvas(ui, ctx, workspace);
                let page = &self.project.pages[self.page];
                let origin = workspace.min + self.view.pan;
                let page_rect =
                    Rect::from_min_size(origin, vec2(page.width, page.height) * self.zoom);
                let rect = if auto_canvas {
                    viewport::content_rect(workspace)
                } else {
                    page_rect
                };
                let painter = ui
                    .painter()
                    .with_clip_rect(workspace.intersect(ui.clip_rect()));
                if !auto_canvas {
                    painter.rect_filled(
                        rect.translate(vec2(0.0, 5.0)).expand(4.0),
                        5.0,
                        Color32::from_black_alpha(7),
                    );
                }
                painter.rect_filled(rect, 0.0, color(&page.background));
                if auto_canvas {
                    viewport::paint_auto_rulers(&painter, workspace, origin, self.zoom);
                } else {
                    // Rulers remain in document coordinates as zoom changes.
                    for x in (0..=page.width as usize).step_by(100) {
                        let px = origin.x + x as f32 * self.zoom;
                        painter.text(
                            Pos2::new(px, origin.y - 16.0),
                            Align2::CENTER_CENTER,
                            x.to_string(),
                            FontId::proportional(9.0),
                            MUTED,
                        );
                        painter.line_segment(
                            [Pos2::new(px, origin.y - 6.0), Pos2::new(px, origin.y - 2.0)],
                            Stroke::new(1.0, BORDER),
                        );
                    }
                    for y in (0..=page.height as usize).step_by(100) {
                        let py = origin.y + y as f32 * self.zoom;
                        painter.text(
                            Pos2::new(origin.x - 20.0, py),
                            Align2::CENTER_CENTER,
                            y.to_string(),
                            FontId::proportional(9.0),
                            MUTED,
                        );
                    }
                }
                if self.grid && self.zoom >= 0.35 {
                    let grid = Stroke::new(0.5, Color32::from_black_alpha(9));
                    let x_range = if auto_canvas {
                        (
                            ((rect.left() - origin.x) / self.zoom / 20.0).floor() as i32,
                            ((rect.right() - origin.x) / self.zoom / 20.0).ceil() as i32,
                        )
                    } else {
                        (0, (page.width / 20.0).ceil() as i32)
                    };
                    let y_range = if auto_canvas {
                        (
                            ((rect.top() - origin.y) / self.zoom / 20.0).floor() as i32,
                            ((rect.bottom() - origin.y) / self.zoom / 20.0).ceil() as i32,
                        )
                    } else {
                        (0, (page.height / 20.0).ceil() as i32)
                    };
                    let grid_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
                    for x in x_range.0..=x_range.1 {
                        let x = x as f32 * 20.0;
                        let px = origin.x + x * self.zoom;
                        grid_painter.line_segment(
                            [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
                            grid,
                        );
                    }
                    for y in y_range.0..=y_range.1 {
                        let y = y as f32 * 20.0;
                        let py = origin.y + y * self.zoom;
                        grid_painter.line_segment(
                            [Pos2::new(rect.left(), py), Pos2::new(rect.right(), py)],
                            grid,
                        );
                    }
                }
                let clipped = painter.with_clip_rect(rect.intersect(ui.clip_rect()));
                let painter = if auto_canvas {
                    clipped.clone()
                } else {
                    painter
                };
                for w in &page.widgets {
                    if !w.hidden {
                        draw_widget(&clipped, w, origin, self.zoom, self.textures.get(&w.id));
                    }
                }
                for w in page
                    .widgets
                    .iter()
                    .filter(|w| self.selected.contains(&w.id) && !w.hidden)
                {
                    draw_selection_frame(&painter, w, origin, self.zoom, false);
                }
                let editable: BTreeSet<_> = page
                    .widgets
                    .iter()
                    .filter(|w| {
                        self.selected.contains(&w.id)
                            && !w.locked
                            && !w.hidden
                            && self
                                .native
                                .as_ref()
                                .is_none_or(|n| n.is_editable_widget(w.id))
                    })
                    .map(|w| w.id)
                    .collect();
                let bounds = selection_geometry(page, &editable);
                if let Some(bounds) = &bounds {
                    draw_selection_frame(&painter, bounds, origin, self.zoom, true);
                }
                let zoom = self.zoom;
                let handle_at = |pos: Pos2| {
                    bounds.as_ref().and_then(|w| {
                        manipulation::handle_positions(w)
                            .into_iter()
                            .rev()
                            .find_map(|(handle, p)| {
                                (document_pos(p, origin, zoom).distance(pos) <= 8.0)
                                    .then_some(handle)
                            })
                    })
                };
                if let Some(pos) = response.hover_pos().filter(|_| !navigating) {
                    if self.tool.is_some() || self.pending_widget.is_some() {
                        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                    } else if let Some(handle) = handle_at(pos) {
                        ctx.set_cursor_icon(match handle {
                            Handle::Rotate => egui::CursorIcon::Grab,
                            Handle::North | Handle::South => egui::CursorIcon::ResizeVertical,
                            Handle::East | Handle::West => egui::CursorIcon::ResizeHorizontal,
                            Handle::NorthWest | Handle::SouthEast => egui::CursorIcon::ResizeNwSe,
                            Handle::NorthEast | Handle::SouthWest => egui::CursorIcon::ResizeNeSw,
                        });
                    }
                }
                if ctx.input(|i| i.pointer.any_released())
                    && let Some(item) = self.pending_widget.take()
                    && let Some(pos) = ctx
                        .input(|i| i.pointer.interact_pos())
                        .filter(|p| rect.contains(*p))
                {
                    let p = (pos - origin) / self.zoom;
                    self.add_library_item(item, p.x, p.y);
                }
                if !navigating
                    && response.clicked()
                    && let Some(pos) = response
                        .interact_pointer_pos()
                        .filter(|pos| !auto_canvas || rect.contains(*pos))
                {
                    if let Some(kind) = self.tool.take() {
                        if rect.contains(pos) {
                            let p = (pos - origin) / self.zoom;
                            self.add_widget(kind, p.x, p.y);
                        }
                    } else {
                        let p = (pos - origin) / self.zoom;
                        let hit = self.project.pages[self.page]
                            .widgets
                            .iter()
                            .rev()
                            .find(|w| {
                                !w.hidden && manipulation::hit_test(w, [p.x, p.y], 4.0 / self.zoom)
                            })
                            .map(|w| w.id);
                        if let Some(id) = hit {
                            let modifiers = ctx.input(|i| i.modifiers);
                            self.select_hit(
                                id,
                                modifiers.shift || modifiers.ctrl,
                                modifiers.ctrl && modifiers.alt,
                            );
                        } else if !ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl) {
                            self.selected.clear();
                        }
                    }
                }
                if !navigating && response.double_clicked() {
                    self.editing = self
                        .selected
                        .iter()
                        .copied()
                        .find(|id| editable.contains(id));
                }
                if !navigating
                    && response.secondary_clicked()
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    let p = (pos - origin) / self.zoom;
                    let hit = self.project.pages[self.page]
                        .widgets
                        .iter()
                        .rev()
                        .find(|w| {
                            !w.hidden && manipulation::hit_test(w, [p.x, p.y], 4.0 / self.zoom)
                        })
                        .map(|w| w.id);
                    if let Some(id) = hit
                        && !self.selected.contains(&id)
                    {
                        self.select_hit(
                            id,
                            false,
                            ctx.input(|i| i.modifiers.ctrl && i.modifiers.alt),
                        );
                    }
                }
                if !navigating
                    && response.drag_started_by(egui::PointerButton::Primary)
                    && let Some(pos) = ctx
                        .input(|i| i.pointer.press_origin())
                        .filter(|pos| !auto_canvas || rect.contains(*pos))
                {
                    if self.tool.is_some() && rect.contains(pos) {
                        let kind = self.tool.take().unwrap();
                        let before = self.project.clone();
                        let p = (pos - origin) / self.zoom;
                        let id = self.project.allocate_id();
                        self.project.pages[self.page]
                            .widgets
                            .push(Widget::new(id, kind, p.x, p.y));
                        self.selected = BTreeSet::from([id]);
                        self.drag = Some(Drag {
                            before,
                            pointer: pos,
                            mode: DragMode::Draw {
                                id,
                                kind,
                                start: [p.x, p.y],
                            },
                        });
                    } else {
                        let page = &self.project.pages[self.page];
                        let handle = handle_at(pos);
                        let p = (pos - origin) / self.zoom;
                        let mut mode = DragMode::Move;
                        if let (Some(handle), Some(bounds)) = (handle, bounds.clone()) {
                            mode = if handle == Handle::Rotate {
                                let pivot = [
                                    bounds.x + bounds.width / 2.0,
                                    bounds.y + bounds.height / 2.0,
                                ];
                                DragMode::Rotate {
                                    pivot,
                                    angle: (p.y - pivot[1]).atan2(p.x - pivot[0]).to_degrees(),
                                    start: [p.x, p.y],
                                }
                            } else {
                                DragMode::Resize {
                                    handle,
                                    bounds: Box::new(bounds),
                                }
                            };
                        } else {
                            let hit = page
                                .widgets
                                .iter()
                                .rev()
                                .find(|w| {
                                    let owner = self
                                        .native
                                        .as_ref()
                                        .map_or(w.id, |n| n.selection_owner(w.id));
                                    let unlocked = if owner == w.id {
                                        !w.locked
                                    } else {
                                        page.widgets.iter().any(|w| w.id == owner && !w.locked)
                                    };
                                    !w.hidden
                                        && unlocked
                                        && self.native.as_ref().is_none_or(|n| {
                                            n.is_editable_widget(n.selection_owner(w.id))
                                        })
                                        && manipulation::hit_test(w, [p.x, p.y], 4.0 / self.zoom)
                                })
                                .map(|w| {
                                    self.native
                                        .as_ref()
                                        .map_or(w.id, |n| n.selection_owner(w.id))
                                });
                            if let Some(id) = hit {
                                if !self.selected.contains(&id) {
                                    if !ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl) {
                                        self.selected.clear();
                                    }
                                    let inside = ctx.input(|i| i.modifiers.ctrl && i.modifiers.alt);
                                    self.selected.extend(rpshop::grouping::expanded_selection(
                                        &self.project.pages[self.page],
                                        &BTreeSet::from([id]),
                                        inside,
                                    ));
                                }
                            } else {
                                let initial =
                                    if ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl) {
                                        self.selected.clone()
                                    } else {
                                        BTreeSet::new()
                                    };
                                self.selected = initial.clone();
                                mode = DragMode::Marquee { initial };
                            }
                        }
                        if !self.selected.is_empty() || matches!(mode, DragMode::Marquee { .. }) {
                            self.drag = Some(Drag {
                                before: self.project.clone(),
                                pointer: pos,
                                mode,
                            });
                        }
                    }
                }
                if !navigating
                    && response.dragged_by(egui::PointerButton::Primary)
                    && let Some(drag) = &self.drag
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    let delta = (pos - drag.pointer) / self.zoom;
                    let modifiers = ctx.input(|i| i.modifiers);
                    if let DragMode::Marquee { initial } = &drag.mode {
                        let from = (drag.pointer - origin) / self.zoom;
                        let to = (pos - origin) / self.zoom;
                        self.selected = initial.clone();
                        self.selected.extend(rpshop::selection::box_select(
                            &self.project.pages[self.page].widgets,
                            [from.x, from.y],
                            [to.x, to.y],
                            rpshop::selection::BoxSelectionMode::Contained,
                        ));
                        self.selected = rpshop::grouping::expanded_selection(
                            &self.project.pages[self.page],
                            &self.selected,
                            modifiers.ctrl && modifiers.alt,
                        );
                        painter.rect_filled(
                            Rect::from_two_pos(drag.pointer, pos),
                            0.0,
                            PURPLE.gamma_multiply(0.08),
                        );
                        painter.rect_stroke(
                            Rect::from_two_pos(drag.pointer, pos),
                            0.0,
                            Stroke::new(1.0, PURPLE),
                            StrokeKind::Inside,
                        );
                    } else {
                        let editable: BTreeSet<_> = self.project.pages[self.page]
                            .widgets
                            .iter()
                            .filter(|w| {
                                self.selected.contains(&w.id)
                                    && !w.locked
                                    && self
                                        .native
                                        .as_ref()
                                        .is_none_or(|n| n.is_editable_widget(w.id))
                            })
                            .map(|w| w.id)
                            .collect();
                        apply_drag(
                            &mut self.project.pages[self.page],
                            &drag.before.pages[self.page],
                            &editable,
                            &drag.mode,
                            [delta.x, delta.y],
                            modifiers.shift,
                            modifiers.alt,
                            self.snap,
                        );
                        if let Some(native) = &self.native {
                            native.refresh_previews(&mut self.project, &drag.before);
                        }
                    }
                }
                if !navigating
                    && response.drag_stopped_by(egui::PointerButton::Primary)
                    && let Some(drag) = self.drag.take()
                {
                    self.remember(drag.before);
                }
                response.context_menu(|ui| {
                    if ui.button("粘贴").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                        ui.close();
                    }
                    if ui.button("复制").clicked() {
                        self.copy_to_system(ctx);
                        ui.close();
                    }
                    if ui
                        .add_enabled(!self.selected.is_empty(), egui::Button::new("剪切  Ctrl+X"))
                        .clicked()
                    {
                        self.cut_components(ctx);
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            !self.selected.is_empty(),
                            egui::Button::new("创建副本  Ctrl+D"),
                        )
                        .clicked()
                    {
                        self.copy();
                        self.paste();
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(self.selected.len() > 1, egui::Button::new("组合  Ctrl+G"))
                        .clicked()
                    {
                        self.group_selection(false);
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            !self.selected.is_empty(),
                            egui::Button::new("解组  Ctrl+Shift+G"),
                        )
                        .clicked()
                    {
                        self.group_selection(true);
                        ui.close();
                    }
                    if ui
                        .add_enabled(!self.selected.is_empty(), egui::Button::new("置于顶层"))
                        .clicked()
                    {
                        self.align(8);
                        ui.close();
                    }
                    if ui
                        .add_enabled(!self.selected.is_empty(), egui::Button::new("置于底层"))
                        .clicked()
                    {
                        self.align(11);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("删除").clicked() {
                        self.delete_selection();
                        ui.close();
                    }
                });
            });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if !self.import_warnings.is_empty() && !self.screenshot_mode {
            let mut open = true;
            egui::Window::new("原生文件打开结果")
                .id(egui::Id::new("import-report"))
                .open(&mut open)
                .default_width(520.0)
                .show(ctx, |ui| {
                    ui.label("工程已打开。以下内容的还原范围需要留意：");
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical()
                        .max_height(330.0)
                        .show(ui, |ui| {
                            for warning in &self.import_warnings {
                                ui.label(format!("• {warning}"));
                            }
                        });
                    ui.separator();
                    ui.label("直接编辑，Ctrl+S 保存原格式；尚不支持编辑的数据会保留。");
                    if ui.button("知道了").clicked() {
                        self.import_warnings.clear();
                    }
                });
            if !open {
                self.import_warnings.clear();
            }
        }
        if let Some(id) = self.editing {
            let mut open = true;
            egui::Window::new("编辑文本")
                .id(egui::Id::new("text-editor"))
                .open(&mut open)
                .collapsible(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    if let Some(w) = self.project.pages[self.page]
                        .widgets
                        .iter_mut()
                        .find(|w| w.id == id)
                    {
                        ui.add(
                            egui::TextEdit::multiline(&mut w.text)
                                .desired_width(340.0)
                                .desired_rows(5),
                        );
                    }
                    if ui.button("完成").clicked() {
                        self.editing = None;
                    }
                });
            if !open {
                self.editing = None;
            }
        }
        if let Some(action) = self.confirm.clone() {
            egui::Window::new("工程尚未保存")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("是否先保存当前修改？");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("保存并继续").clicked() && self.save_project(false) {
                            self.confirm = None;
                            if action == "close" {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            } else {
                                self.replace(&action);
                            }
                        }
                        if ui.button("放弃修改").clicked() {
                            self.confirm = None;
                            if action == "close" {
                                self.saved = self.project.clone();
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            } else {
                                self.replace(&action);
                            }
                        }
                        if ui.button("取消").clicked() {
                            self.confirm = None;
                        }
                    });
                });
        }
        if self.help {
            egui::Window::new("关于 RPShop")
                .open(&mut self.help)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading("把想法，变成可以点击的原型。");
                    ui.add_space(10.0);
                    ui.label("原型设计 · 交互预览 · HTML/SVG 导出");
                    ui.separator();
                    for (key, usage) in [
                        ("Ctrl+N / O / S", "新建 / 打开 / 保存"),
                        ("Ctrl+Z / Y", "撤销 / 重做"),
                        ("Ctrl+C / X / V / D", "复制 / 剪切 / 粘贴 / 创建副本"),
                        ("Ctrl+Shift+S / Z", "另存为 / 重做"),
                        ("V / R / O / T / L", "选择 / 矩形 / 椭圆 / 文本 / 直线"),
                        ("Ctrl+方向键", "调整尺寸；加 Shift 每次 10 像素"),
                        ("Shift + 点击", "多选组件"),
                        ("Ctrl + 点击", "增减多选"),
                        ("Ctrl+Alt + 点击", "选择组内元件"),
                        ("Ctrl + 滚轮 / + / −", "缩放画布"),
                        ("滚轮 / Shift + 滚轮", "纵向 / 横向滚动画布"),
                        ("空格+拖动 / 中键拖动", "平移画布"),
                        ("Ctrl+0 / 1 / 2 / 3", "100% / 居中画布 / 适配选择 / 居中选择"),
                        ("Ctrl+G / Ctrl+Shift+G", "组合 / 解组"),
                        ("Shift + 拖动 / 旋转", "主轴移动 / 15° 角度吸附"),
                        ("Shift / Alt + 缩放", "等比缩放 / 从中心缩放"),
                        ("方向键 / Shift+方向键", "移动 1 / 10 像素"),
                        ("Delete / Backspace", "删除选中的未锁定组件"),
                        ("F5 / Esc", "浏览器预览当前页 / 取消绘图工具"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(key).strong());
                            ui.label(usage);
                        });
                    }
                    ui.separator();
                ui.label("支持打开已验证的 RP 8/11 .rp 和 RP 11 .rplib，Ctrl+S 保存原格式。打开时会列出还原限制。");
                });
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
        self.frames += 1;
        for event in ctx.input(|i| i.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event
                && let Some(path) = self.screenshot.take()
            {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = image::save_buffer(
                    &path,
                    image.as_raw(),
                    image.width() as u32,
                    image.height() as u32,
                    image::ColorType::Rgba8,
                ) {
                    eprintln!("截图失败: {e}");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.view.panning = None;
            if let Some(drag) = self.drag.take() {
                self.project = drag.before;
                self.publish_preview();
            }
            self.editing = None;
            self.tool = None;
        }
        if self.confirm.is_none() && self.editing.is_none() {
            self.viewport_shortcuts(ctx);
            self.keyboard(ctx);
        }
        if !self.screenshot_mode
            && self.confirm.is_none()
            && let Some(path) =
                ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()))
        {
            self.pending_open = Some(path);
            self.request_replace("open-path");
        }
        let before = self.project.clone();
        self.history_changed = false;
        let dragging = self.drag.is_some();
        self.header(ctx);
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(18, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        vec2((ui.available_width() - 330.0).max(120.0), 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&self.status).size(11.0).color(MUTED),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&self.status);
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} 个页面  ·  {} 个组件",
                                self.project.pages.len(),
                                self.project.pages[self.page].widgets.len()
                            ))
                            .size(11.0)
                            .color(MUTED),
                        );
                    });
                });
            });
        self.left_panel(ctx);
        self.inspector(ctx);
        self.load_textures(ctx);
        self.canvas(ctx);
        self.dialogs(ctx);
        if !dragging && self.drag.is_none() && !self.history_changed {
            self.remember(before);
        }
        if ctx.input(|i| i.viewport().close_requested())
            && self.project != self.saved
            && !self.screenshot_mode
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some("close".into());
        }
        if self.last_recovery.elapsed() > Duration::from_secs(5) && !self.screenshot_mode {
            if let Some(dir) = directories::ProjectDirs::from("", "RPShop", "RPShop") {
                let _ = std::fs::create_dir_all(dir.data_local_dir());
                let mut draft = self.project.clone();
                normalize_draft(&mut draft);
                if let Err(e) = draft.save(&dir.data_local_dir().join("recovery.rpshop")) {
                    self.status = format!("恢复备份失败：{e:#}");
                }
            }
            self.last_recovery = Instant::now();
        }
        if self.frames == 5 && self.screenshot.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.screenshot_mode {
            eframe::set_value(storage, sidebar::STORAGE_KEY, &self.sidebar_layout);
            eframe::set_value(
                storage,
                "rpshop-session",
                &Session {
                    project: self.project.clone(),
                    path: self.path.clone(),
                    saved: self.saved.clone(),
                    import_warnings: self.import_warnings.clone(),
                    native_state: self.native.as_ref().map(|n| n.session_state()),
                },
            );
        }
    }
    fn auto_save_interval(&self) -> Duration {
        Duration::from_secs(5)
    }
}

fn layer_title_width(row_width: f32) -> f32 {
    (row_width - 2.0 * LAYER_CONTROL_WIDTH - 2.0 * LAYER_COLUMN_SPACING).max(0.0)
}

#[derive(Clone, Copy)]
enum LayerControl {
    Visibility { hidden: bool },
    Lock { locked: bool },
}

fn layer_control(ui: &mut egui::Ui, control: LayerControl, name: &str) -> egui::Response {
    let (checked, emphasized, description) = match control {
        LayerControl::Visibility { hidden: true } => {
            (false, true, "当前已隐藏；点击显示。隐藏不会删除组件。")
        }
        LayerControl::Visibility { hidden: false } => (
            true,
            false,
            "当前已显示；点击隐藏。隐藏后画布和预览中都不可见。",
        ),
        LayerControl::Lock { locked: true } => (
            true,
            true,
            "当前已锁定，不能移动、缩放或修改；点击解锁以恢复编辑。",
        ),
        LayerControl::Lock { locked: false } => {
            (false, false, "当前未锁定，可以编辑；点击锁定以防止误改。")
        }
    };
    let label = format!("{name}：{description}");
    let (rect, response) = ui.allocate_exact_size(vec2(LAYER_CONTROL_WIDTH, 27.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), checked, &label)
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let painter = ui.painter();
        painter.rect(
            rect.shrink(1.0),
            4.0,
            if emphasized {
                Color32::from_rgb(240, 235, 255)
            } else {
                visuals.weak_bg_fill
            },
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
        let color = if !ui.is_enabled() {
            MUTED
        } else if emphasized {
            PURPLE
        } else {
            INK
        };
        let stroke = Stroke::new(1.4, color);
        let center = rect.center();
        match control {
            LayerControl::Visibility { hidden } => {
                for direction in [-1.0, 1.0] {
                    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                        [
                            center + vec2(-8.0, 0.0),
                            center + vec2(-4.0, 7.0 * direction),
                            center + vec2(4.0, 7.0 * direction),
                            center + vec2(8.0, 0.0),
                        ],
                        false,
                        Color32::TRANSPARENT,
                        stroke,
                    ));
                }
                painter.circle_stroke(center, 2.3, stroke);
                if hidden {
                    painter.line_segment(
                        [center + vec2(-9.0, -7.0), center + vec2(9.0, 7.0)],
                        Stroke::new(1.8, color),
                    );
                }
            }
            LayerControl::Lock { locked } => {
                let shackle = if locked {
                    [
                        vec2(-4.0, -1.0),
                        vec2(-4.0, -9.0),
                        vec2(4.0, -9.0),
                        vec2(4.0, -1.0),
                    ]
                } else {
                    [
                        vec2(-3.0, -1.0),
                        vec2(-3.0, -9.0),
                        vec2(7.0, -9.0),
                        vec2(7.0, -2.0),
                    ]
                };
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    shackle.map(|point| center + point),
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ));
                painter.rect_stroke(
                    Rect::from_min_max(center + vec2(-6.0, -1.0), center + vec2(6.0, 7.0)),
                    2.0,
                    stroke,
                    StrokeKind::Inside,
                );
                painter.circle_filled(center + vec2(0.0, 2.0), 1.1, color);
                painter.line_segment([center + vec2(0.0, 2.0), center + vec2(0.0, 4.5)], stroke);
            }
        }
    }
    response.on_hover_text(label)
}

fn library_button(ui: &mut egui::Ui, item: LibraryItem) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(59.0, 56.0), Sense::click_and_drag());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let p = ui.painter();
    p.rect(
        rect,
        4.0,
        if response.hovered() {
            Color32::from_rgb(240, 235, 255)
        } else {
            Color32::from_rgb(248, 248, 252)
        },
        Stroke::new(1.0, BORDER),
        StrokeKind::Inside,
    );
    let icon = Rect::from_center_size(rect.center_top() + vec2(0.0, 19.0), vec2(25.0, 20.0));
    if matches!(item.kind(), WidgetKind::Text) {
        let symbol = match item.id {
            "heading1" => "H1",
            "heading2" => "H2",
            "heading3" => "H3",
            "paragraph" => "¶",
            "link" => "↗",
            _ => "T",
        };
        p.text(
            icon.center(),
            Align2::CENTER_CENTER,
            symbol,
            FontId::proportional(18.0),
            INK,
        );
    } else if matches!(
        item.id,
        "table" | "horizontal-menu" | "vertical-menu" | "tree"
    ) {
        p.rect_stroke(icon, 1.0, Stroke::new(1.2, INK), StrokeKind::Inside);
        for i in 1..3 {
            let fraction = i as f32 / 3.0;
            p.line_segment(
                [
                    icon.min + vec2(0.0, icon.height() * fraction),
                    icon.min + vec2(icon.width(), icon.height() * fraction),
                ],
                Stroke::new(1.0, INK),
            );
            if item.id == "table" || item.id == "horizontal-menu" {
                p.line_segment(
                    [
                        icon.min + vec2(icon.width() * fraction, 0.0),
                        icon.min + vec2(icon.width() * fraction, icon.height()),
                    ],
                    Stroke::new(1.0, INK),
                );
            }
        }
    } else {
        let mut project = Project::blank();
        let mut preview = item.create(&mut project, 0.0, 0.0).remove(0);
        let scale = (icon.width() / preview.width).min(icon.height() / preview.height);
        let size = if matches!(preview.kind, WidgetKind::Checkbox | WidgetKind::Radio) {
            vec2(18.0, 18.0)
        } else {
            vec2(preview.width, preview.height) * scale
        };
        preview.width = size.x;
        preview.height = size.y;
        preview.radius *= scale;
        preview.text.clear();
        preview.fill = if item.id == "button" {
            "#E9E2FD"
        } else {
            "#FFFFFF00"
        }
        .into();
        preview.stroke = "#292B3A".into();
        if preview.kind == WidgetKind::Line {
            let points = rpshop::vector_edit::line_endpoints(&preview)
                .map(|point| icon.center() - size / 2.0 + vec2(point[0], point[1]));
            p.line_segment(points, Stroke::new(1.3, INK));
        } else {
            draw_widget(p, &preview, icon.center() - size / 2.0, 1.0, None);
        }
    }
    let label_font = if item.label.chars().count() > 5 {
        10.0
    } else {
        12.0
    };
    p.text(
        rect.center_bottom() - vec2(0.0, 12.0),
        Align2::CENTER_CENTER,
        item.label,
        FontId::proportional(label_font),
        INK,
    );
    response.on_hover_text(format!(
        "{} · {}\n点击添加，或拖入画布",
        item.label,
        item.category.label()
    ))
}

fn action_label(action: Action) -> &'static str {
    match action {
        Action::None => "无交互",
        Action::Navigate => "跳转页面",
        Action::ToggleVisibility => "切换组件显示",
        Action::SetText => "设置组件文本",
    }
}

fn normalize_draft(project: &mut Project) {
    if project.name.trim().is_empty() {
        project.name = "未命名项目".into();
    }
    for (i, page) in project.pages.iter_mut().enumerate() {
        if page.name.trim().is_empty() {
            page.name = format!("页面 {}", i + 1);
        }
    }
}

fn color(hex: &str) -> Color32 {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 && h.len() != 8 {
        return Color32::GRAY;
    }
    let read = |i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
    Color32::from_rgba_unmultiplied(
        read(0),
        read(2),
        read(4),
        if h.len() == 8 { read(6) } else { 255 },
    )
}

fn color_edit(ui: &mut egui::Ui, label: &str, hex: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        let c = color(hex);
        let mut rgba = c.to_srgba_unmultiplied();
        if ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed() {
            *hex = format!(
                "#{:02X}{:02X}{:02X}{:02X}",
                rgba[0], rgba[1], rgba[2], rgba[3]
            );
        }
        ui.label(RichText::new(hex.to_uppercase()).size(11.0).color(MUTED));
    });
}

fn document_pos(point: [f32; 2], origin: Pos2, zoom: f32) -> Pos2 {
    origin + vec2(point[0], point[1]) * zoom
}

fn selection_geometry(page: &Page, ids: &BTreeSet<u64>) -> Option<Widget> {
    let mut widgets = page.widgets.iter().filter(|w| ids.contains(&w.id));
    let first = widgets.next()?.clone();
    let rest: Vec<_> = widgets.collect();
    if rest.is_empty() {
        return Some(first);
    }
    let mut bounds = manipulation::rotated_bounds(&first);
    for w in rest {
        let other = manipulation::rotated_bounds(w);
        bounds = [
            bounds[0].min(other[0]),
            bounds[1].min(other[1]),
            bounds[2].max(other[2]),
            bounds[3].max(other[3]),
        ];
    }
    let mut frame = Widget::new(0, WidgetKind::Rectangle, bounds[0], bounds[1]);
    frame.width = (bounds[2] - bounds[0]).max(1.0);
    frame.height = (bounds[3] - bounds[1]).max(1.0);
    Some(frame)
}

fn draw_selection_frame(
    painter: &egui::Painter,
    w: &Widget,
    origin: Pos2,
    zoom: f32,
    handles: bool,
) {
    let points = manipulation::handle_positions(w);
    let screen = |i: usize| document_pos(points[i].1, origin, zoom);
    painter.add(egui::Shape::closed_line(
        vec![screen(0), screen(2), screen(4), screen(6)],
        Stroke::new(1.0, PURPLE),
    ));
    if !handles {
        return;
    }
    painter.line_segment([screen(1), screen(8)], Stroke::new(1.0, PURPLE));
    for (i, (handle, _)) in points.iter().enumerate() {
        let pos = screen(i);
        if *handle == Handle::Rotate {
            painter.circle(pos, 4.5, Color32::WHITE, Stroke::new(1.5, PURPLE));
        } else {
            let rect = Rect::from_center_size(pos, vec2(6.0, 6.0));
            painter.rect_filled(rect, 0.0, Color32::WHITE);
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, PURPLE), StrokeKind::Inside);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_drag(
    page: &mut Page,
    before: &Page,
    ids: &BTreeSet<u64>,
    mode: &DragMode,
    delta: [f32; 2],
    shift: bool,
    alt: bool,
    snap: bool,
) {
    if let DragMode::Draw { id, kind, start } = mode {
        if let Some(widget) = page.widgets.iter_mut().find(|w| w.id == *id) {
            let mut delta = delta;
            if snap {
                delta = delta.map(|v| (v / 8.0).round() * 8.0);
            }
            if shift {
                if *kind == WidgetKind::Line {
                    let length = delta[0].hypot(delta[1]);
                    let angle = (delta[1].atan2(delta[0]) / std::f32::consts::FRAC_PI_4).round()
                        * std::f32::consts::FRAC_PI_4;
                    delta = [length * angle.cos(), length * angle.sin()];
                } else {
                    let side = delta[0].abs().max(delta[1].abs());
                    delta = [
                        side * if delta[0] < 0.0 { -1.0 } else { 1.0 },
                        side * if delta[1] < 0.0 { -1.0 } else { 1.0 },
                    ];
                }
            }
            let from = if alt {
                [start[0] - delta[0], start[1] - delta[1]]
            } else {
                *start
            };
            let to = [start[0] + delta[0], start[1] + delta[1]];
            widget.x = from[0].min(to[0]);
            widget.y = from[1].min(to[1]);
            widget.width = (to[0] - from[0]).abs().clamp(1.0, 20000.0);
            widget.height = (to[1] - from[1]).abs().clamp(1.0, 20000.0);
            if *kind == WidgetKind::Line {
                let x = if to[0] >= from[0] {
                    [0.0, 1.0]
                } else {
                    [1.0, 0.0]
                };
                let y = if (to[1] - from[1]).abs() < 0.001 {
                    [0.5, 0.5]
                } else if to[1] >= from[1] {
                    [0.0, 1.0]
                } else {
                    [1.0, 0.0]
                };
                widget.vector_points = Some(
                    serde_json::json!([[{"Path":{"X":x[0],"Y":y[0]},"HigherSegmentExists":true},{"Path":{"X":x[1],"Y":y[1]},"HigherSegmentExists":false}]]),
                );
            }
        }
        return;
    }
    let grid = if snap { 8.0 } else { 0.0 };
    let movement = rpshop::selection::constrained_delta(delta, shift);
    let movement = movement.map(|v| if snap { (v / 8.0).round() * 8.0 } else { v });
    for w in &mut page.widgets {
        if !ids.contains(&w.id) || w.locked {
            continue;
        }
        let Some(old) = before.widgets.iter().find(|old| old.id == w.id) else {
            continue;
        };
        match mode {
            DragMode::Move => {
                w.x = old.x + movement[0];
                w.y = old.y + movement[1];
            }
            DragMode::Resize { handle, bounds } => {
                let resized = manipulation::resize(bounds, *handle, delta, shift, alt, grid);
                if ids.len() == 1 {
                    *w = resized;
                } else {
                    let sx = resized.width / bounds.width;
                    let sy = resized.height / bounds.height;
                    w.x = resized.x + (old.x - bounds.x) * sx;
                    w.y = resized.y + (old.y - bounds.y) * sy;
                    w.width = (old.width * sx).max(1.0);
                    w.height = (old.height * sy).max(1.0);
                }
            }
            DragMode::Rotate {
                pivot,
                angle,
                start,
            } => {
                let mut degrees = (start[1] + delta[1] - pivot[1])
                    .atan2(start[0] + delta[0] - pivot[0])
                    .to_degrees()
                    - angle;
                if shift {
                    let base = if ids.len() == 1 { old.rotation } else { 0.0 };
                    degrees = ((base + degrees) / 15.0).round() * 15.0 - base;
                }
                *w = manipulation::rotate(old, degrees, *pivot);
            }
            DragMode::Marquee { .. } => {}
            DragMode::Draw { .. } => {}
        }
    }
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let cjk_font = "noto-cjk-sc";
    fonts.font_data.insert(
        cjk_font.into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansCJKsc-Regular.otf"
        ))),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, cjk_font.into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, cjk_font.into());
    let mut bold_names = Vec::new();
    for (name, path) in [
        ("canvas-arial-bold", "C:/Windows/Fonts/arialbd.ttf"),
        ("canvas-cjk-bold", "C:/Windows/Fonts/msyhbd.ttc"),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                name.into(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            bold_names.push(name.to_owned());
        }
    }
    if !bold_names.is_empty() {
        bold_names.extend(fonts.families[&egui::FontFamily::Proportional].clone());
        fonts
            .families
            .insert(egui::FontFamily::Name("canvas-bold".into()), bold_names);
    }
    ctx.set_fonts(fonts);
    let mut style = egui::Style {
        visuals: egui::Visuals::light(),
        ..Default::default()
    };
    style.visuals.override_text_color = Some(INK);
    style.visuals.panel_fill = Color32::WHITE;
    style.visuals.selection.bg_fill = Color32::from_rgb(239, 233, 255);
    style.visuals.selection.stroke = Stroke::new(1.0, PURPLE);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(247, 247, 251);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(247, 247, 251);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(240, 235, 255);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(240, 235, 255);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, PURPLE);
    style.spacing.item_spacing = vec2(7.0, 7.0);
    style.spacing.button_padding = vec2(10.0, 6.0);
    style.spacing.interact_size = vec2(30.0, 29.0);
    style.spacing.text_edit_width = 205.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(11.0));
    ctx.set_theme(egui::Theme::Light);
    ctx.set_style_of(egui::Theme::Light, style);
}
