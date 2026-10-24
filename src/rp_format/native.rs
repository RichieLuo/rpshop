//! Patch editable native properties while preserving every unedited source part.

use super::*;
use anyhow::bail;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Binding {
    pub object: String,
    pub scale: f32,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct GroupBinding {
    pub object: String,
}

#[derive(Clone)]
pub struct NativeDocument {
    pub project: Project,
    pub warnings: Vec<String>,
    baseline: Project,
    source: container::Container,
    parts: BTreeMap<u64, Graph>,
    bindings: BTreeMap<u64, Binding>,
    preview_parents: BTreeMap<u64, u64>,
    group_bindings: BTreeMap<u64, GroupBinding>,
    pages: BTreeMap<u64, String>,
    source_path: PathBuf,
    library: bool,
    preview_identity: Arc<()>,
}

impl NativeDocument {
    pub fn is_editable_widget(&self, id: u64) -> bool {
        editable_native_id(&self.baseline, &self.bindings, id)
    }
    pub fn selection_owner(&self, id: u64) -> u64 {
        self.preview_parents.get(&id).copied().unwrap_or(id)
    }
    pub fn is_bitmap_widget(&self, id: u64) -> bool {
        if !self.bindings.contains_key(&id) {
            return self.is_editable_widget(id);
        }
        self.bindings
            .get(&id)
            .and_then(|binding| {
                let (package, object) = binding.object.rsplit_once('/')?;
                self.parts
                    .values()
                    .find(|graph| graph.package == package)?
                    .node(object)
            })
            .is_some_and(|node| {
                node.kind.ends_with(":ImageBox") || node.kind.ends_with(":Screenshot")
            })
    }
    /// Refresh read-only master contents after an editor geometry operation.
    /// `before` must be the operation's starting project (the drag-start project
    /// for every drag frame), so repeated calls never compound the transform.
    pub fn refresh_previews(&self, project: &mut Project, before: &Project) {
        let parent_ids = self
            .preview_parents
            .values()
            .copied()
            .collect::<BTreeSet<_>>();
        let old = before
            .pages
            .iter()
            .flat_map(|page| &page.widgets)
            .map(|widget| (widget.id, widget))
            .collect::<BTreeMap<_, _>>();
        let parents = project
            .pages
            .iter()
            .flat_map(|page| &page.widgets)
            .filter(|widget| parent_ids.contains(&widget.id))
            .map(|widget| (widget.id, widget.clone()))
            .collect::<BTreeMap<_, _>>();
        for page in &mut project.pages {
            page.widgets.retain(|widget| {
                self.preview_parents
                    .get(&widget.id)
                    .is_none_or(|parent| parents.contains_key(parent))
            });
            for widget in &mut page.widgets {
                let Some(parent) = self.preview_parents.get(&widget.id) else {
                    continue;
                };
                let (Some(previous), Some(old_parent), Some(new_parent)) =
                    (old.get(&widget.id), old.get(parent), parents.get(parent))
                else {
                    continue;
                };
                *widget = transform_preview(previous, old_parent, new_parent);
            }
        }
    }
    /// Source page packages remain stable when a native save reallocates editor IDs.
    pub fn preview_page_keys(&self) -> BTreeMap<u64, String> {
        self.pages.clone()
    }

    /// An in-process identity for immutable preview source snapshots. Clones
    /// share it; replacing verified source bindings allocates a new identity.
    pub fn preview_identity(&self) -> usize {
        Arc::as_ptr(&self.preview_identity) as usize
    }

    /// Build a separate native runtime scene from an in-memory source snapshot.
    /// Staging supported editor edits uses the native patcher without writing a
    /// container or changing the editor/session's baseline and bindings.
    pub fn preview_scene(&self, edited: &Project) -> Result<super::preview_scene::NativePreview> {
        edited.validate()?;
        if self.library {
            return Ok(super::preview_scene::NativePreview {
                scene: super::preview_scene::PreviewScene {
                    diagnostics: vec!["组件库预览展示静态组件，暂不执行库内原生交互".into()],
                    ..Default::default()
                },
                program: super::preview_program::PreviewProgram::default(),
            });
        }
        let mut staged;
        let parts = if edited == &self.baseline {
            &self.parts
        } else {
            staged = self.parts.clone();
            self.patch_project(edited, &mut staged)?;
            &staged
        };
        let (scene, graph, page_keys) = super::preview_scene::collect(parts, &self.pages, edited)?;
        let program = super::preview_program::compile(&graph, &scene.bindings, &page_keys);
        Ok(super::preview_scene::NativePreview { scene, program })
    }

    /// Persist source bindings separately from the editor's unsaved document.
    pub fn session_state(&self) -> Value {
        json!({"version":2,"fingerprint":self.source.original_fingerprint(),
            "project":self.baseline,"bindings":self.bindings,"pages":self.pages,
            "preview_parents":self.preview_parents,
            "group_bindings":self.group_bindings,
            "library":self.library})
    }

    pub fn restore_session_state(&mut self, state: &Value) -> Result<()> {
        ensure!(
            state["version"].as_u64() == Some(2),
            "原生会话绑定版本已更新，请重新打开源文件；草稿内容可继续保留"
        );
        ensure!(
            state["fingerprint"].as_str() == Some(self.source.original_fingerprint().as_str()),
            "原生文件已更改，会话绑定已失效，请重新打开文件"
        );
        ensure!(
            state["library"].as_bool() == Some(self.library),
            "原生会话文件类型不一致"
        );
        let baseline: Project =
            serde_json::from_value(state["project"].clone()).context("原生会话项目无效")?;
        baseline.validate()?;
        ensure!(
            baseline.page_tree == self.baseline.page_tree,
            "页面目录已更新，请重新打开源文件以恢复目录树；当前草稿可继续保留"
        );
        let bindings: BTreeMap<u64, Binding> =
            serde_json::from_value(state["bindings"].clone()).context("原生元件绑定无效")?;
        let pages: BTreeMap<u64, String> =
            serde_json::from_value(state["pages"].clone()).context("原生页面绑定无效")?;
        {
            let previews = state
                .get("preview_parents")
                .context("原生会话缺少母版预览绑定")?;
            let previews: BTreeMap<u64, u64> =
                serde_json::from_value(previews.clone()).context("母版预览绑定无效")?;
            ensure!(
                previews == self.preview_parents,
                "母版预览绑定与源文件不匹配"
            );
        }
        {
            let groups = state
                .get("group_bindings")
                .context("原生会话缺少组合绑定")?;
            let groups: BTreeMap<u64, GroupBinding> =
                serde_json::from_value(groups.clone()).context("原生组绑定无效")?;
            ensure!(groups == self.group_bindings, "原生组绑定与源文件不匹配");
        }
        let widget_ids = baseline
            .pages
            .iter()
            .flat_map(|p| p.widgets.iter().map(|w| w.id))
            .collect::<BTreeSet<_>>();
        let owners = owners(&self.parts);
        for (id, binding) in &bindings {
            ensure!(
                widget_ids.contains(id)
                    && owners.contains_key(&binding.object)
                    && binding.scale.is_finite()
                    && binding.scale > 0.0
                    && binding.scale <= 1.0,
                "原生会话元件绑定与源文件不匹配"
            );
        }
        if !self.library {
            let read_only = self
                .baseline
                .pages
                .iter()
                .flat_map(|p| &p.widgets)
                .filter(|w| !self.bindings.contains_key(&w.id))
                .map(|w| w.id)
                .collect::<BTreeSet<_>>();
            ensure!(
                widget_ids
                    .iter()
                    .all(|id| bindings.contains_key(id) || read_only.contains(id)),
                "原生会话缺少元件绑定"
            );
            ensure!(pages.len() == baseline.pages.len(), "原生会话缺少页面绑定");
            for page in &baseline.pages {
                ensure!(
                    pages.get(&page.id).is_some_and(|package| self
                        .parts
                        .values()
                        .any(|g| g.package == *package
                            && g.root().is_some_and(|n| n.kind == "Axure:Page"))),
                    "原生会话页面绑定与源文件不匹配"
                );
            }
        }
        self.project = baseline.clone();
        self.baseline = baseline;
        self.bindings = bindings;
        self.pages = pages;
        self.preview_identity = Arc::new(());
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        project: Project,
        warnings: Vec<String>,
        source: container::Container,
        parts: BTreeMap<u64, Graph>,
        bindings: BTreeMap<u64, Binding>,
        preview_parents: BTreeMap<u64, u64>,
        group_bindings: BTreeMap<u64, GroupBinding>,
        pages: BTreeMap<String, u64>,
        source_path: PathBuf,
        library: bool,
    ) -> Self {
        Self {
            baseline: project.clone(),
            project,
            warnings,
            source,
            parts,
            bindings,
            preview_parents,
            group_bindings,
            pages: pages
                .into_iter()
                .map(|(package, id)| (id, package))
                .collect(),
            source_path,
            library,
            preview_identity: Arc::new(()),
        }
    }

    /// Save to the original native format. The source is checked immediately
    /// before its atomic replacement, so external edits are never overwritten.
    pub fn save(&mut self, project: &Project, path: &Path) -> Result<()> {
        project.validate()?;
        let extension = if self.library { "rplib" } else { "rp" };
        ensure!(
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension)),
            "原生文件须保存为 .{extension}；不能通过更改扩展名转换项目与元件库"
        );
        let original_path = same_path(path, &self.source_path);
        ensure!(
            self.source.matches_original(&self.source_path)?,
            "原生文件已被其他程序修改，请重新打开后再编辑；本次未覆盖原文件"
        );
        let bytes = if *project == self.baseline {
            self.source.original_bytes()
        } else {
            let mut parts = self.parts.clone();
            self.patch_project(project, &mut parts)?;
            let mut replacements = BTreeMap::new();
            for (offset, graph) in &parts {
                let bytes = binary::encode(graph)
                    .with_context(|| format!("序列化原生部件 {offset} 失败"))?;
                // Reuse raw compression frames whenever the object graph did
                // not change. Native settings and unknown fields stay intact.
                if bytes != self.source.part(*offset)? {
                    replacements.insert(*offset, bytes);
                }
            }
            self.source.encode_with_parts(&replacements)?
        };
        // Verify the entire staged container, including a new RPShop read of its
        // native page content, before touching the destination file.
        let verified = super::open_container(path, container::from_bytes(&bytes)?)
            .context("原生保存结果无法重新读取，原文件保持不变")?;
        if original_path {
            self.source.write_atomic_checked(path, &bytes)?;
        } else {
            container::write_atomic(path, &bytes)?;
        }
        // A complete reopen refreshes native IDs and the coordinate mapping for
        // library galleries and newly created widgets on subsequent saves.
        *self = verified;
        Ok(())
    }

    fn patch_project(&self, project: &Project, parts: &mut BTreeMap<u64, Graph>) -> Result<()> {
        ensure!(
            project.page_tree == self.baseline.page_tree,
            "原生保存暂不支持修改页面文件夹结构；请保留目录结构后保存"
        );
        ensure!(
            project.pages.iter().map(|p| p.id).collect::<Vec<_>>()
                == self.baseline.pages.iter().map(|p| p.id).collect::<Vec<_>>(),
            "原生保存暂不支持新增、删除或重排页面；请保留页面结构后保存"
        );
        ensure!(
            project.name == self.baseline.name,
            "原生项目名称来自文件名，请使用另存为更改文件名"
        );
        let owners = owners(parts);
        let previous = self
            .baseline
            .pages
            .iter()
            .flat_map(|p| p.widgets.iter().map(move |w| (w.id, (p.id, w))))
            .collect::<BTreeMap<_, _>>();
        let current = project
            .pages
            .iter()
            .flat_map(|p| p.widgets.iter().map(move |w| (w.id, (p.id, w))))
            .collect::<BTreeMap<_, _>>();
        let mut edited_bindings = self.bindings.clone();
        for id in previous.keys().filter(|id| !self.bindings.contains_key(id)) {
            ensure!(
                current.contains_key(id)
                    || self
                        .preview_parents
                        .get(id)
                        .is_some_and(|parent| !current.contains_key(parent)),
                "只读母版预览和元件库名称标签暂不支持单独删除"
            );
        }
        for page in &project.pages {
            let old = self
                .baseline
                .pages
                .iter()
                .find(|p| p.id == page.id)
                .context("原生页面绑定丢失")?;
            if self.library {
                ensure!(
                    page.name == old.name
                        && page.width == old.width
                        && page.height == old.height
                        && page.canvas == old.canvas
                        && page.background == old.background,
                    "元件库展示页是浏览布局，暂不支持修改展示页名称、尺寸或背景；组件本身仍可编辑保存"
                );
            } else {
                ensure!(
                    page.width == old.width
                        && page.height == old.height
                        && page.canvas == old.canvas,
                    "原生保存暂不支持修改页面尺寸或 Auto 模式"
                );
                let package = self.pages.get(&page.id).context("原生页面绑定丢失")?;
                if page.name != old.name {
                    rename_page(parts, package, &page.name)?;
                }
                if page.background != old.background {
                    set_page_background(parts, package, &page.background)?;
                }
            }
        }
        for (id, binding) in &self.bindings {
            let Some((old_page, old)) = previous.get(id) else {
                continue;
            };
            if let Some((page, widget)) = current.get(id) {
                ensure!(
                    page == old_page,
                    "原生保存暂不支持跨页面移动现有元件，请在目标页面复制创建"
                );
                if *widget != *old {
                    self.patch_widget(parts, &owners, binding, old, widget)?;
                }
            } else {
                let source = get_node(parts, &owners, &binding.object)?;
                ensure!(
                    !source.kind.ends_with(":DynamicPanel") && !source.kind.ends_with(":Repeater"),
                    "动态面板与 Repeater 在编辑器中已展开，暂不支持原生删除其外层容器"
                );
                ensure!(
                    !parts
                        .get(owners.get(&binding.object).unwrap())
                        .unwrap()
                        .objects
                        .values()
                        .any(|n| array(&n.properties["cells"])
                            .iter()
                            .any(|v| reference_id(v) == Some(raw_id(&binding.object)))),
                    "原生保存暂不支持删除表格单元格，请修改其文本或样式"
                );
                remove_visible_reference(parts, &binding.object);
            }
        }
        for (id, (page, widget)) in &current {
            if let Some((_, old)) = previous.get(id) {
                if !self.bindings.contains_key(id) {
                    let expected = self.preview_parents.get(id).and_then(|parent| {
                        Some(transform_preview(
                            old,
                            previous.get(parent)?.1,
                            current.get(parent)?.1,
                        ))
                    });
                    ensure!(
                        expected
                            .as_ref()
                            .map_or(*widget == *old, |expected| matching_preview(
                                widget, expected
                            )),
                        "只读母版预览和元件库名称标签暂不支持编辑；请编辑母版实例本身或普通元件"
                    );
                }
            } else {
                ensure!(
                    !self.library,
                    "元件库原生保存暂不支持新增组件；已有组件可编辑或删除元件"
                );
                let package = self.pages.get(page).context("新增元件的页面绑定丢失")?;
                let object = add_widget(parts, package, widget, &self.pages)?;
                edited_bindings.insert(*id, Binding { object, scale: 1.0 });
            }
        }
        for page in &project.pages {
            let old = self
                .baseline
                .pages
                .iter()
                .find(|p| p.id == page.id)
                .unwrap();
            let old_ids = old
                .widgets
                .iter()
                .filter(|w| current.contains_key(&w.id))
                .map(|w| w.id)
                .collect::<Vec<_>>();
            let new_ids = page
                .widgets
                .iter()
                .filter(|w| previous.contains_key(&w.id))
                .map(|w| w.id)
                .collect::<Vec<_>>();
            let additions_in_middle = page.widgets.iter().enumerate().any(|(i, w)| {
                !previous.contains_key(&w.id)
                    && page.widgets[i + 1..]
                        .iter()
                        .any(|next| previous.contains_key(&next.id))
            });
            let groups_changed = page.widgets.iter().any(|widget| {
                previous
                    .get(&widget.id)
                    .map_or(!widget.groups.is_empty(), |(_, old)| {
                        old.groups != widget.groups
                    })
            });
            if groups_changed {
                ensure!(!self.library, "元件库展示页暂不支持新增或拆分原生组合");
                patch_native_groups(
                    parts,
                    self.pages.get(&page.id).unwrap(),
                    page,
                    old,
                    &edited_bindings,
                    &self.preview_parents,
                    &self.group_bindings,
                )?;
            }
            if old_ids != new_ids || additions_in_middle || groups_changed {
                ensure!(!self.library, "元件库展示页暂不支持重排元件层级");
                let package = self.pages.get(&page.id).unwrap();
                patch_native_order(
                    parts,
                    package,
                    page,
                    old,
                    &edited_bindings,
                    &self.preview_parents,
                )?;
            }
        }
        super::native_events::patch(
            parts,
            &self.baseline,
            project,
            &edited_bindings,
            &self.pages,
        )?;
        Ok(())
    }

    fn patch_widget(
        &self,
        parts: &mut BTreeMap<u64, Graph>,
        owners: &BTreeMap<String, u64>,
        binding: &Binding,
        old: &Widget,
        widget: &Widget,
    ) -> Result<()> {
        ensure!(
            widget.text_layout == old.text_layout,
            "原生文字布局元数据暂不支持修改，请保留原有对齐和内边距"
        );
        let offset = *owners.get(&binding.object).context("元件原生部件丢失")?;
        let original = get_node(parts, owners, &binding.object)?.clone();
        let text_id = if widget.kind.is_plain_text_control() {
            raw_id(&binding.object).to_owned()
        } else {
            contained_text_id(parts.get(&offset).unwrap(), &original)
                .unwrap_or_else(|| raw_id(&binding.object).to_owned())
        };
        let style_id = original
            .properties
            .get("MyStyle")
            .and_then(reference_id)
            .context("元件原生样式丢失")?
            .to_owned();
        let style_offset = offset;
        let style_node = parts
            .get_mut(&style_offset)
            .unwrap()
            .objects
            .get_mut(&style_id)
            .context("原生样式对象丢失")?;
        let original_properties = props(&style_node.properties["PropList"]);
        if widget.x != old.x || widget.y != old.y {
            let previous = original_properties.get(&101).unwrap_or(&Value::Null);
            let x = number(&previous["X"]).unwrap_or(0.0) + (widget.x - old.x) / binding.scale;
            let y = number(&previous["Y"]).unwrap_or(0.0) + (widget.y - old.y) / binding.scale;
            set_property(&mut style_node.properties, 101, json!({"X":x,"Y":y}));
        }
        if widget.width != old.width || widget.height != old.height {
            let previous = original_properties.get(&105).unwrap_or(&Value::Null);
            let width = number(&previous["Width"]).unwrap_or(old.width / binding.scale)
                * widget.width
                / old.width;
            let height = number(&previous["Height"]).unwrap_or(old.height / binding.scale)
                * widget.height
                / old.height;
            set_property(
                &mut style_node.properties,
                105,
                json!({"Width":width,"Height":height}),
            );
        }
        let bitmap_vector =
            original.kind.ends_with(":VectorShape") && widget.kind == WidgetKind::Image;
        let points_changed = widget.vector_points != old.vector_points;
        let type_changed = widget.kind != old.kind;
        if (points_changed && !type_changed)
            || (bitmap_vector && widget.image_data != old.image_data)
        {
            validate_native_vector_edit(&original, old, widget)?;
        }
        if points_changed && type_changed {
            ensure!(
                widget.vector_points.is_none(),
                "请先保存元件类型变化，再镜像其新原生路径"
            );
        }
        for (changed, id, color) in [
            (
                widget.fill != old.fill,
                if widget.kind == WidgetKind::Line {
                    8
                } else {
                    7
                },
                &widget.fill,
            ),
            (widget.stroke != old.stroke, 8, &widget.stroke),
            (widget.text_color != old.text_color, 6, &widget.text_color),
        ] {
            if changed {
                set_property(
                    &mut style_node.properties,
                    id,
                    json!({"fill-type":"solid","fill-color":color_argb(color)?}),
                );
            }
        }
        if widget.font_size != old.font_size {
            set_property(
                &mut style_node.properties,
                2,
                json!(widget.font_size / binding.scale * 0.75),
            );
        }
        if widget.radius != old.radius {
            set_property(
                &mut style_node.properties,
                71,
                json!(widget.radius / binding.scale),
            );
        }
        if widget.opacity != old.opacity {
            set_property(&mut style_node.properties, 70, json!(widget.opacity));
        }
        if widget.rotation != old.rotation {
            set_property(
                &mut style_node.properties,
                ROTATION_PROPERTY,
                json!(widget.rotation),
            );
        }
        if widget.image_data != old.image_data
            && !bitmap_vector
            && let Some(image) = &widget.image_data
        {
            let data = image.split_once(',').context("图片不是 data URI")?.1;
            set_property(
                &mut style_node.properties,
                27,
                json!({"Stream":{"$binary":data}}),
            );
        }
        let node = parts
            .get_mut(&offset)
            .unwrap()
            .objects
            .get_mut(raw_id(&binding.object))
            .unwrap();
        if widget.hidden != old.hidden {
            node.properties["VisibleFromSettings"] = json!(!widget.hidden);
        }
        if widget.locked != old.locked {
            node.properties["IsLocked"] = json!(widget.locked);
        }
        if widget.name != old.name {
            node.properties["name-block"]["text"] = json!(widget.name);
        }
        if widget.kind == WidgetKind::Radio && widget.radio_group != old.radio_group {
            node.properties["radio-group"] =
                json!(widget.radio_group.as_deref().unwrap_or(&widget.name));
        }
        if widget.kind != old.kind {
            ensure!(!bitmap_vector, "自定义矢量元件暂不支持更改原生类型");
            set_kind(node, widget.kind)?;
        }
        if points_changed && !type_changed {
            node.properties["ControlPoints"] = widget.vector_points.clone().unwrap();
        }
        if widget.kind.is_option_list() && widget.text != old.text {
            let graph = parts.get_mut(&offset).unwrap();
            let mut node = graph.node(raw_id(&binding.object)).unwrap().clone();
            ensure!(
                list_options_text(&node.properties, graph).is_some(),
                "此列表选项结构尚未识别，不能修改原生选项"
            );
            set_list_options(graph, &mut node, &widget.text);
            graph
                .objects
                .insert(raw_id(&binding.object).to_owned(), node);
        }
        let text_node = parts
            .get_mut(&offset)
            .unwrap()
            .objects
            .get_mut(&text_id)
            .context("原生文本对象丢失")?;
        if widget.text != old.text && !widget.kind.is_option_list() {
            if text_node.kind.ends_with(":RichTextPanel")
                && text_node.properties.get("rtf-block").is_none()
            {
                text_node.properties["rtf-block"] = json!({"*type":"rich-text-block", "attributed-string":{
                    "Version":{"Major":8,"Minor":0,"Build":0,"Revision":0},"Paragraphs":[]}});
            }
            set_text(&mut text_node.properties, &widget.text);
        }
        if widget.font_size != old.font_size || widget.text_color != old.text_color {
            update_inlines(&mut text_node.properties, widget, binding.scale)?;
        }
        if widget.interaction != old.interaction {
            set_interaction(
                parts,
                offset,
                raw_id(&binding.object),
                &widget.interaction,
                &self.pages,
            )?;
        }
        Ok(())
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        return a == b;
    }
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

fn owners(parts: &BTreeMap<u64, Graph>) -> BTreeMap<String, u64> {
    parts
        .iter()
        .flat_map(|(offset, g)| {
            g.objects
                .keys()
                .map(move |id| (format!("{}/{id}", g.package), *offset))
        })
        .collect()
}

fn get_node<'a>(
    parts: &'a BTreeMap<u64, Graph>,
    owners: &BTreeMap<String, u64>,
    id: &str,
) -> Result<&'a Node> {
    owners
        .get(id)
        .and_then(|o| parts.get(o))
        .and_then(|g| g.node(raw_id(id)))
        .with_context(|| format!("原生对象 {id} 不存在"))
}

fn set_property(style: &mut Value, id: u32, value: Value) {
    if !style["PropList"].is_array() {
        style["PropList"] = json!([]);
    }
    let list = style["PropList"].as_array_mut().unwrap();
    if let Some(entry) = list.iter_mut().find(|e| {
        e["DOAName"]
            .as_i64()
            .is_some_and(|n| (n as u32) & 0x3fff_ffff == id)
    }) {
        entry["DOAName"] = json!((0xc000_0000u32 | id) as i32);
        entry["Value"] = value;
    } else {
        list.push(json!({"DOAName":(0xc000_0000u32|id) as i32,"Value":value}));
    }
}

fn color_argb(color: &str) -> Result<i32> {
    let rgb = u32::from_str_radix(color.trim_start_matches('#'), 16)?;
    Ok(if color.len() == 9 {
        ((rgb & 255) << 24) | (rgb >> 8)
    } else {
        0xff00_0000 | rgb
    } as i32)
}

fn rename_page(parts: &mut BTreeMap<u64, Graph>, package: &str, name: &str) -> Result<()> {
    for graph in parts.values_mut() {
        for node in graph
            .objects
            .values_mut()
            .filter(|n| n.kind == "Axure:PackageInfo")
        {
            if node.properties.get("package-id").and_then(reference_id) == Some(package) {
                // The editor includes folder paths for browsing. Preserve the
                // native folder tree and store only the page's final segment.
                node.properties["package-name"] = json!(name.rsplit(" / ").next().unwrap_or(name));
                return Ok(());
            }
        }
    }
    bail!("原生页面名称对象不存在")
}

fn set_page_background(parts: &mut BTreeMap<u64, Graph>, package: &str, color: &str) -> Result<()> {
    let graph = parts
        .values_mut()
        .find(|g| g.package == package)
        .context("原生页面部件不存在")?;
    let style = graph
        .root()
        .and_then(|n| n.properties.get("page-style"))
        .and_then(reference_id)
        .context("原生页面样式不存在")?
        .to_owned();
    let node = graph
        .objects
        .get_mut(&style)
        .context("原生页面样式对象不存在")?;
    set_property(
        &mut node.properties,
        7,
        json!({"fill-type":"solid","fill-color":color_argb(color)?}),
    );
    Ok(())
}

fn set_text(properties: &mut Value, text: &str) {
    if properties.get("TextBlock").is_some() {
        properties["TextBlock"]["text"] = json!(text);
    } else {
        let field = if properties.get("rtf-block").is_some() {
            "rtf-block"
        } else {
            "RichTextBlock"
        };
        let mut paragraph=properties.pointer(&format!("/{field}/attributed-string/Paragraphs/0")).cloned()
            .unwrap_or_else(||json!({"Wrapping":0,"Trimming":0,"HorizontalAlignment":0,"LineSpacing":0.0,"ListInfo":{"IndentLevel":0,"ListType":0}}));
        let mut inline = paragraph.pointer("/Inlines/0").cloned().unwrap_or_else(|| {
            json!({
            "Type":"Axure:AttributedString:AxRun","Family":"Arial","Size":9.75,"Weight":400,
            "Style":0,"Stretch":5,"Underline":false,"Color":-13421773,"Baseline":0,
            "CharacterSpacing":0.0,"Strikethrough":false})
        });
        let paragraphs = text
            .split('\n')
            .map(|line| {
                inline["Text"] = json!(line);
                paragraph["Inlines"] = json!([inline.clone()]);
                paragraph.clone()
            })
            .collect::<Vec<_>>();
        if !properties[field].is_object() {
            properties[field] = json!({"*type":"rich-text-block","attributed-string":{
                "Version":{"Major":11,"Minor":0,"Build":0,"Revision":0},"Paragraphs":[]}});
        }
        properties[field]["attributed-string"]["Paragraphs"] = json!(paragraphs);
    }
}

fn update_inlines(properties: &mut Value, widget: &Widget, scale: f32) -> Result<()> {
    let field = if properties.get("rtf-block").is_some() {
        "rtf-block"
    } else {
        "RichTextBlock"
    };
    if let Some(paragraphs) = properties
        .pointer_mut(&format!("/{field}/attributed-string/Paragraphs"))
        .and_then(Value::as_array_mut)
    {
        let color = color_argb(&widget.text_color)?;
        for paragraph in paragraphs {
            if let Some(inlines) = paragraph["Inlines"].as_array_mut() {
                for inline in inlines {
                    inline["Size"] = json!(widget.font_size / scale * 0.75);
                    inline["Color"] = json!(color);
                }
            }
        }
    }
    Ok(())
}

fn apply_added_text_layout(properties: &mut Value, widget: &Widget) {
    let Some(layout) = &widget.text_layout else {
        return;
    };
    properties["AutoFitWidth"] = json!(!layout.wrap);
    let field = if properties.get("rtf-block").is_some() {
        "rtf-block"
    } else {
        "RichTextBlock"
    };
    if let Some(paragraphs) = properties
        .pointer_mut(&format!("/{field}/attributed-string/Paragraphs"))
        .and_then(Value::as_array_mut)
    {
        for paragraph in paragraphs {
            paragraph["HorizontalAlignment"] = json!(layout.horizontal);
            if let Some(inlines) = paragraph["Inlines"].as_array_mut() {
                for inline in inlines {
                    inline["Weight"] = json!(if layout.bold { 700 } else { 400 });
                }
            }
        }
    }
}

fn contained_text_id(graph: &Graph, node: &Node) -> Option<String> {
    array(&node.properties["diagram-objects"])
        .iter()
        .filter_map(reference_id)
        .find(|id| {
            graph
                .node(id)
                .is_some_and(|n| n.kind.ends_with(":RichTextPanel"))
        })
        .map(str::to_owned)
}

fn remove_visible_reference(parts: &mut BTreeMap<u64, Graph>, id: &str) {
    for graph in parts.values_mut() {
        if !id.starts_with(&format!("{}/", graph.package)) {
            continue;
        }
        for node in graph.objects.values_mut() {
            for field in ["diagram-objects", "DiagramObjects"] {
                if let Some(list) = node.properties[field].as_array_mut() {
                    list.retain(|v| reference_id(v) != Some(raw_id(id)));
                }
            }
        }
    }
}

fn set_kind(node: &mut Node, kind: WidgetKind) -> Result<()> {
    match kind {
        WidgetKind::Rectangle
        | WidgetKind::Text
        | WidgetKind::Button
        | WidgetKind::Ellipse
        | WidgetKind::Line => {
            node.kind = "Axure:DiagramObject:VectorShape".into();
            node.properties["Shape"] = json!(match kind {
                WidgetKind::Ellipse => 15,
                WidgetKind::Line => 29,
                _ => 0,
            });
            if !matches!(kind, WidgetKind::Ellipse | WidgetKind::Line) {
                let points = [0,1,2,3].map(|i|json!({
                    "Path":{"X":if i==1||i==2{1.0}else{0.0},"Y":if i>=2{1.0}else{0.0}},
                    "LowerHandle":{"X":if i==1||i==2{1.0}else{0.0},"Y":if i>=2{1.0}else{0.0}},
                    "HigherHandle":{"X":if i==1||i==2{1.0}else{0.0},"Y":if i>=2{1.0}else{0.0}},"HigherSegmentExists":true
                }));
                node.properties["ControlPoints"] = json!([points]);
            } else if kind == WidgetKind::Ellipse {
                let k = 0.2761423749153967;
                node.properties["ControlPoints"] = json!([[
                    {"Path":{"X":0.5,"Y":0.0},"LowerHandle":{"X":0.5-k,"Y":0.0},"HigherHandle":{"X":0.5+k,"Y":0.0},"HigherSegmentExists":true},
                    {"Path":{"X":1.0,"Y":0.5},"LowerHandle":{"X":1.0,"Y":0.5-k},"HigherHandle":{"X":1.0,"Y":0.5+k},"HigherSegmentExists":true},
                    {"Path":{"X":0.5,"Y":1.0},"LowerHandle":{"X":0.5+k,"Y":1.0},"HigherHandle":{"X":0.5-k,"Y":1.0},"HigherSegmentExists":true},
                    {"Path":{"X":0.0,"Y":0.5},"LowerHandle":{"X":0.0,"Y":0.5+k},"HigherHandle":{"X":0.0,"Y":0.5-k},"HigherSegmentExists":true}
                ]]);
            } else {
                node.properties["ControlPoints"] = json!([[
                    {"Path":{"X":0.0,"Y":0.5},"LowerHandle":{"X":0.0,"Y":0.5},"HigherHandle":{"X":0.0,"Y":0.5},"HigherSegmentExists":true},
                    {"Path":{"X":1.0,"Y":0.5},"LowerHandle":{"X":1.0,"Y":0.5},"HigherHandle":{"X":1.0,"Y":0.5},"HigherSegmentExists":false}
                ]]);
            }
        }
        WidgetKind::Image => node.kind = "Axure:DiagramObject:ImageBox".into(),
        WidgetKind::Input => {
            node.kind = "Axure:DiagramObject:TextBox".into();
            node.properties
                .as_object_mut()
                .unwrap()
                .remove("RichTextBlock");
            if !node.properties["TextBlock"].is_object() {
                node.properties["TextBlock"] = json!({"*type":"text-block","text":""});
            }
            node.properties["WordWrap"] = json!(false);
            node.properties["ReadOnly"] = json!(false);
            node.properties["PlaceholderText"] = json!("");
            node.properties["HideHintOnFocus"] = json!(false);
            node.properties["Type"] = json!(0);
            node.properties["MaxLength"] = json!(2147483647i32);
        }
        WidgetKind::Checkbox => node.kind = "Axure:DiagramObject:Checkbox".into(),
        WidgetKind::Radio | WidgetKind::TextArea | WidgetKind::Droplist | WidgetKind::ListBox => {
            ensure!(
                Some(node.kind.as_str()) == native_form_kind(kind),
                "原生保存暂不支持转换为{}，请从同类原生模板新增",
                kind.label()
            );
        }
        WidgetKind::Panel => bail!("原生保存暂不支持将基础元件转换为动态容器"),
    }
    Ok(())
}

fn native_form_kind(kind: WidgetKind) -> Option<&'static str> {
    match kind {
        WidgetKind::Radio => Some("Axure:DiagramObject:RadioButton"),
        WidgetKind::TextArea => Some("Axure:DiagramObject:TextArea"),
        WidgetKind::Droplist => Some("Axure:DiagramObject:ComboBox"),
        WidgetKind::ListBox => Some("Axure:DiagramObject:ListBox"),
        _ => None,
    }
}

/// SelectObject's string/selection fields were checked in the installed RP 11
/// form library. Old records are retained in case another source object uses them.
fn set_list_options(graph: &mut Graph, node: &mut Node, text: &str) {
    let mut options = Vec::new();
    if !text.is_empty() {
        for (index, text) in text.split('\n').enumerate() {
            let id = guid();
            graph.objects.insert(
                id.clone(),
                Node {
                    kind: "Axure:SelectObject".into(),
                    properties: json!({"object":text,"selected":index==0}),
                },
            );
            options.push(json!({"$ref":id,"$token":3}));
        }
    }
    node.properties["list-options"] = json!(options);
    node.properties["multiple"] = json!(false);
}

fn guid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:032x}",
        timestamp
            ^ ((std::process::id() as u128) << 72)
            ^ (COUNTER.fetch_add(1, Ordering::Relaxed) as u128) << 32
    )
}

fn add_widget(
    parts: &mut BTreeMap<u64, Graph>,
    package: &str,
    widget: &Widget,
    pages: &BTreeMap<u64, String>,
) -> Result<String> {
    ensure!(
        widget.kind != WidgetKind::Panel,
        "原生保存暂不支持新增动态容器，请使用矩形或其他基础元件"
    );
    let form_kind = native_form_kind(widget.kind);
    let (template_graph, template_id) = parts
        .values()
        .find_map(|g| {
            g.objects
                .iter()
                .find(|(_, n)| {
                    if let Some(kind) = form_kind {
                        n.kind == kind
                    } else {
                        n.kind == "Axure:DiagramObject:VectorShape"
                            && n.properties["Shape"].as_i64() == Some(0)
                    }
                })
                .map(|(id, _)| (g.clone(), id.clone()))
        })
        .with_context(|| if form_kind.is_some() {
            format!("此文件缺少{}原生模板，暂不支持在该原生文件中新增此类型；可在 RPShop 工程中使用", widget.kind.label())
        } else {
            "此文件没有可用于新增元件的原生形状模板".into()
        })?;
    let template = template_graph.node(&template_id).unwrap();
    let style_id = template
        .properties
        .get("MyStyle")
        .and_then(reference_id)
        .context("原生模板样式不存在")?;
    let style_graph = parts
        .values()
        .find(|g| g.node(style_id).is_some())
        .context("原生模板样式对象不存在")?
        .clone();
    let graph = parts
        .values_mut()
        .find(|g| g.package == package)
        .context("新增元件的原生页面不存在")?;
    let diagram = graph
        .root()
        .and_then(|n| n.properties.get("diagram"))
        .and_then(reference_id)
        .context("原生页面画布不存在")?
        .to_owned();
    let id = guid();
    let new_style_id = guid();
    graph.copy_node_from(&template_graph, &template_id, &id)?;
    graph.copy_node_from(&style_graph, style_id, &new_style_id)?;
    let mut node = graph.node(&id).unwrap().clone();
    let mut style = graph.node(&new_style_id).unwrap().clone();
    let mut text_child = if !widget.kind.is_plain_text_control() {
        if let Some(source_id) = contained_text_id(&template_graph, template) {
            let child_id = guid();
            graph.copy_node_from(&template_graph, &source_id, &child_id)?;
            let mut child = graph.node(&child_id).unwrap().clone();
            replace_reference(&mut child.properties, &source_id, &child_id);
            replace_reference(&mut child.properties, &template_id, &id);
            node.properties["diagram-objects"] = json!([{"$ref":child_id}]);
            Some((child_id, child))
        } else {
            None
        }
    } else {
        node.properties["diagram-objects"] = json!([]);
        None
    };
    replace_reference(&mut node.properties, &template_id, &id);
    if let Some(old_diagram) = template
        .properties
        .pointer("/name-block/property-dictionary/parent-diagram-id")
        .and_then(reference_id)
    {
        replace_reference(&mut node.properties, old_diagram, &diagram);
        if let Some((_, child)) = &mut text_child {
            replace_reference(&mut child.properties, old_diagram, &diagram);
        }
    }
    node.properties
        .as_object_mut()
        .unwrap()
        .remove("InteractionMap");
    node.properties["AdaptiveStyles"] = json!([]);
    node.properties["MyStyle"] = json!({"$ref":new_style_id});
    node.properties["name-block"] = json!({"*type":"text-block","text":widget.name,"property-dictionary":{
        "type":"label","diagram-object-id":{"$guid":id},"parent-diagram-id":{"$guid":diagram}}});
    node.properties["VisibleFromSettings"] = json!(!widget.hidden);
    node.properties["IsLocked"] = json!(widget.locked);
    for field in [
        "AutoFitHeight",
        "AutoFitWidth",
        "IsContained",
        "Disabled",
        "Selected",
        "Error",
    ] {
        node.properties[field] = json!(false);
    }
    node.properties["SubmitButton"] = json!({"$ref":EMPTY_ID});
    set_kind(&mut node, widget.kind)?;
    if widget.kind == WidgetKind::Radio {
        node.properties["radio-group"] =
            json!(widget.radio_group.as_deref().unwrap_or(&widget.name));
    }
    if widget.kind.is_option_list() {
        set_list_options(graph, &mut node, &widget.text);
    }
    if widget.kind.is_plain_text_control() {
        node.properties["ReadOnly"] = json!(false);
        node.properties["PlaceholderText"] = json!("");
        node.properties["HideHintOnFocus"] = json!(false);
        if widget.kind == WidgetKind::TextArea {
            node.properties["WordWrap"] = json!(true);
        }
    }
    if widget.kind != WidgetKind::Image
        && let Some(points) = &widget.vector_points
    {
        crate::vector_edit::validate_points(points)?;
        node.properties["ControlPoints"] = points.clone();
    }
    if matches!(
        widget.kind,
        WidgetKind::Image | WidgetKind::Checkbox | WidgetKind::Radio
    ) || widget.kind.is_plain_text_control()
    {
        node.properties
            .as_object_mut()
            .unwrap()
            .remove("ControlPoints");
    }
    if widget.kind.is_plain_text_control() {
        node.properties["TextBlock"]["property-dictionary"] = json!({"type":"plain-text-object",
            "diagram-object-label":widget.name,"text-object-id":{"$guid":id},"parent-diagram-id":{"$guid":diagram}});
    }
    if let Some((child_id, mut child)) = text_child {
        if child.properties.get("rtf-block").is_none() {
            child.properties["rtf-block"] = json!({"*type":"rich-text-block", "attributed-string":{
                "Version":{"Major":8,"Minor":0,"Build":0,"Revision":0},"Paragraphs":[]}});
        }
        child.properties["ContainedSize"] = json!({"Width":widget.width,"Height":widget.height});
        set_text(&mut child.properties, &widget.text);
        update_inlines(&mut child.properties, widget, 1.0)?;
        apply_added_text_layout(&mut child.properties, widget);
        graph.objects.insert(child_id, child);
    } else {
        set_text(
            &mut node.properties,
            if widget.kind.is_option_list() {
                ""
            } else {
                &widget.text
            },
        );
        update_inlines(&mut node.properties, widget, 1.0)?;
        apply_added_text_layout(&mut node.properties, widget);
    }
    style.properties["PropList"] = json!([]);
    for (key, value) in [
        (101, json!({"X":widget.x,"Y":widget.y})),
        (105, json!({"Width":widget.width,"Height":widget.height})),
        (
            7,
            json!({"fill-type":"solid","fill-color":color_argb(&widget.fill)?}),
        ),
        (
            8,
            json!({"fill-type":"solid","fill-color":color_argb(if widget.kind==WidgetKind::Line {&widget.fill}else{&widget.stroke})?}),
        ),
        (
            6,
            json!({"fill-type":"solid","fill-color":color_argb(&widget.text_color)?}),
        ),
        (2, json!(widget.font_size * 0.75)),
        (71, json!(widget.radius)),
        (70, json!(widget.opacity)),
        (ROTATION_PROPERTY, json!(widget.rotation)),
    ] {
        set_property(&mut style.properties, key, value);
    }
    if let Some(layout) = &widget.text_layout {
        set_property(&mut style.properties, 9, json!(layout.horizontal));
        set_property(&mut style.properties, 10, json!(layout.vertical));
        for (property, padding) in [11, 12, 13, 14].into_iter().zip(layout.padding) {
            set_property(&mut style.properties, property, json!(padding));
        }
    }
    if let Some(data) = &widget.image_data {
        set_property(
            &mut style.properties,
            27,
            json!({"Stream":{"$binary":data.split_once(',').context("图片格式无效")?.1}}),
        );
    }
    graph.objects.insert(id.clone(), node);
    graph.objects.insert(new_style_id, style);
    graph
        .objects
        .get_mut(&diagram)
        .context("原生画布对象不存在")?
        .properties["diagram-objects"]
        .as_array_mut()
        .context("原生画布元件列表无效")?
        .push(json!({"$ref":id}));
    let offset = *parts.iter().find(|(_, g)| g.package == package).unwrap().0;
    set_interaction(parts, offset, &id, &widget.interaction, pages)?;
    Ok(format!("{package}/{id}"))
}

fn set_interaction(
    parts: &mut BTreeMap<u64, Graph>,
    offset: u64,
    id: &str,
    interaction: &Interaction,
    pages: &BTreeMap<u64, String>,
) -> Result<()> {
    ensure!(
        matches!(interaction.action, Action::None | Action::Navigate),
        "原生保存暂不支持新增显示隐藏或设置文本交互，可保存单击跳转页面"
    );
    let graph = parts.get_mut(&offset).context("原生交互部件不存在")?;
    let has_authored = graph
        .node(id)
        .and_then(|node| node.properties.get("InteractionMap"))
        .and_then(|value| graph.resolve(value))
        .and_then(|map| map.properties["node-table"].as_object())
        .is_some_and(|table| {
            table
                .values()
                .filter_map(reference_id)
                .any(super::native_events::is_authored_case)
        });
    ensure!(
        !has_authored,
        "此元件包含新建事件，请通过事件与动作列表修改；原有交互已保留"
    );
    if interaction.action == Action::None {
        graph
            .objects
            .get_mut(id)
            .context("原生交互元件不存在")?
            .properties
            .as_object_mut()
            .unwrap()
            .remove("InteractionMap");
        return Ok(());
    }
    let target = interaction
        .target_page
        .and_then(|p| pages.get(&p))
        .context("原生跳转目标页面无效")?;
    let map = guid();
    let tree = guid();
    let event_node = guid();
    let case_node = guid();
    let action_node = guid();
    let event = guid();
    let case = guid();
    let action = guid();
    let target_id = guid();
    let literal = guid();
    let new_nodes = [
        (
            map.clone(),
            "Axure:InteractionTreeMap",
            json!({"node-table":{
            event_node.clone():{"$ref":event},case_node.clone():{"$ref":case},action_node.clone():{"$ref":action}},"tree":{"$ref":tree},"PackageMode":false}),
        ),
        (
            tree.clone(),
            "Axure:Tree",
            json!({"children":[{"$ref":event_node}],"tree":{"$ref":tree}}),
        ),
        (
            event_node.clone(),
            "Axure:Node",
            json!({"children":[{"$ref":case_node}],"parent-node":{"$ref":tree},"tree":{"$ref":tree}}),
        ),
        (
            case_node.clone(),
            "Axure:Node",
            json!({"children":[{"$ref":action_node}],"parent-node":{"$ref":event_node},"tree":{"$ref":tree}}),
        ),
        (
            action_node.clone(),
            "Axure:Node",
            json!({"children":[],"parent-node":{"$ref":case_node},"tree":{"$ref":tree}}),
        ),
        (
            event.clone(),
            "Axure:InteractionEvent",
            json!({"Disabled":false,"event-type":16,"master-containing-raised-event":{"$ref":EMPTY_ID,"$type":null,"$token":16},"raised-event-id":{"$guid":EMPTY_ID},"variable-id":{"$guid":EMPTY_ID}}),
        ),
        (
            case.clone(),
            "Axure:InteractionCase",
            json!({"Disabled":false,"Description":"","Condition":{"$ref":EMPTY_ID},"IsNewIfGroup":false}),
        ),
        (
            action.clone(),
            "Axure:InteractionLinkWindowAction",
            json!({"Disabled":false,"target":{"$ref":target_id},"link-type":1}),
        ),
        (
            target_id.clone(),
            "Axure:InteractionTarget",
            json!({"target-type":1,"target-page-id":{"$ref":target,"$type":"Axure:Page","$token":19},"target-literal":{"$ref":literal}}),
        ),
        (
            literal.clone(),
            "Axure:Expr:StringLiteral",
            json!({"value":"","variable-identifiers":[],"repeater-scope":{"$ref":EMPTY_ID},"TargetRepeaterScope":{"$ref":EMPTY_ID}}),
        ),
    ];
    for (id, kind, properties) in new_nodes {
        graph.objects.insert(
            id,
            Node {
                kind: kind.into(),
                properties,
            },
        );
    }
    graph.set_dictionary_key_token(&map, "/node-table", 3)?;
    graph
        .objects
        .get_mut(id)
        .context("原生交互元件不存在")?
        .properties["InteractionMap"] = json!({"$ref":map});
    Ok(())
}

fn replace_reference(value: &mut Value, old: &str, new: &str) {
    match value {
        Value::Object(map) => {
            for key in ["$ref", "$guid"] {
                if map.get(key).and_then(Value::as_str) == Some(old) {
                    map.insert(key.into(), json!(new));
                }
            }
            for nested in map.values_mut() {
                replace_reference(nested, old, new);
            }
        }
        Value::Array(values) => {
            for nested in values {
                replace_reference(nested, old, new);
            }
        }
        _ => {}
    }
}

fn raw_id(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

fn editable_native_id(baseline: &Project, bindings: &BTreeMap<u64, Binding>, id: u64) -> bool {
    bindings.contains_key(&id)
        || !baseline
            .pages
            .iter()
            .any(|page| page.widgets.iter().any(|widget| widget.id == id))
}

fn validate_native_vector_edit(original: &Node, old: &Widget, changed: &Widget) -> Result<()> {
    ensure!(
        original.kind.ends_with(":VectorShape") && changed.kind == old.kind,
        "原生路径镜像仅适用于未改变类型的矢量元件"
    );
    let source = old
        .vector_points
        .as_ref()
        .context("该元件未保留可编辑原生路径")?;
    let target = changed
        .vector_points
        .as_ref()
        .context("不能删除现有原生矢量路径")?;
    ensure!(
        original.properties.get("ControlPoints") == Some(source),
        "原生矢量路径与编辑基线不一致"
    );
    crate::vector_edit::validate_points(source)?;
    crate::vector_edit::validate_points(target)?;
    // Two reflections also cover floating-point cancellation from flipping an
    // object twice before saving. Every candidate retains all point metadata.
    for horizontal in 0..=2 {
        for vertical in 0..=2 {
            let mut points = source.clone();
            for _ in 0..horizontal {
                points = crate::vector_edit::mirrored_points(&points, true)?;
            }
            for _ in 0..vertical {
                points = crate::vector_edit::mirrored_points(&points, false)?;
            }
            if points != *target {
                continue;
            }
            let mut preview = old.image_data.clone();
            for _ in 0..horizontal {
                preview = preview
                    .as_deref()
                    .map(|image| crate::bitmap::flip_data_uri(image, true))
                    .transpose()?;
            }
            for _ in 0..vertical {
                preview = preview
                    .as_deref()
                    .map(|image| crate::bitmap::flip_data_uri(image, false))
                    .transpose()?;
            }
            if same_preview_pixels(preview.as_deref(), changed.image_data.as_deref())? {
                return Ok(());
            }
        }
    }
    bail!("原生保存仅支持可验证的路径镜像及其对应预览，不能用图片替换矢量路径")
}

fn same_preview_pixels(expected: Option<&str>, actual: Option<&str>) -> Result<bool> {
    if expected == actual {
        return Ok(true);
    }
    let (Some(expected), Some(actual)) = (expected, actual) else {
        return Ok(false);
    };
    let decode = |data: &str| -> Result<image::RgbaImage> {
        let (_, payload) = data.split_once(',').context("矢量预览图片格式无效")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(payload)?;
        Ok(image::load_from_memory(&bytes)?.to_rgba8())
    };
    Ok(decode(expected)? == decode(actual)?)
}

type NativeListKey = (u64, String, String);
type NativeListOrders = BTreeMap<NativeListKey, Vec<Value>>;

struct NativeOrderProjection<'a> {
    parts: &'a BTreeMap<u64, Graph>,
    widgets: BTreeMap<String, u64>,
    packages: BTreeMap<String, u64>,
    previews: BTreeMap<u64, Vec<u64>>,
    lists: std::cell::RefCell<BTreeSet<NativeListKey>>,
}

impl NativeOrderProjection<'_> {
    fn object(
        &self,
        offset: u64,
        id: &str,
        orders: &NativeListOrders,
        visiting: &mut BTreeSet<(u64, String)>,
        depth: usize,
    ) -> Result<Vec<u64>> {
        ensure!(depth <= 64, "原生层级嵌套超过限制");
        ensure!(
            visiting.insert((offset, id.to_owned())),
            "原生层级包含循环引用"
        );
        let graph = self.parts.get(&offset).context("原生层级部件不存在")?;
        let node = graph.node(id).context("原生层级对象不存在")?;
        let field = if node.kind.ends_with(":Layer") {
            Some("DiagramObjects")
        } else if matches!(node.kind.as_str(), "Axure:Diagram" | "Axure:PanelDiagram")
            || node.kind.ends_with(":Table")
            || node.kind.ends_with(":MenuObject")
        {
            Some("diagram-objects")
        } else {
            None
        };
        let mut result = Vec::new();
        if let Some(field) = field {
            let key = (offset, id.to_owned(), field.to_owned());
            if node.kind.ends_with(":Layer")
                || matches!(node.kind.as_str(), "Axure:Diagram" | "Axure:PanelDiagram")
            {
                self.lists.borrow_mut().insert(key.clone());
            }
            let references = orders
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or_else(|| array(&node.properties[field]));
            let references = if matches!(node.kind.as_str(), "Axure:Diagram" | "Axure:PanelDiagram")
            {
                direct_diagram_references(graph, references)?
            } else {
                references.to_vec()
            };
            for reference in &references {
                let child = reference_id(reference).context("原生层级包含无法识别的引用")?;
                result.extend(self.object(offset, child, orders, visiting, depth + 1)?);
            }
        } else {
            if let Some(widget) = self.widgets.get(&format!("{}/{id}", graph.package)) {
                result.push(*widget);
                if let Some(previews) = self.previews.get(widget) {
                    result.extend(previews);
                }
            }
            if node.kind.ends_with(":DynamicPanel") {
                if let Some(package) = array(&node.properties["panel-diagrams"])
                    .first()
                    .and_then(reference_id)
                {
                    let state_offset = *self
                        .packages
                        .get(package)
                        .context("动态面板状态层级不存在")?;
                    let state = &self.parts[&state_offset];
                    let diagram = state
                        .root()
                        .and_then(|root| root.properties.get("diagram"))
                        .and_then(reference_id)
                        .unwrap_or(&state.root);
                    result.extend(self.object(
                        state_offset,
                        diagram,
                        orders,
                        visiting,
                        depth + 1,
                    )?);
                }
            } else if node.kind.ends_with(":Repeater")
                && let Some(diagram) = node.properties.get("BaseDiagram").and_then(reference_id)
            {
                result.extend(self.object(offset, diagram, orders, visiting, depth + 1)?);
            }
        }
        visiting.remove(&(offset, id.to_owned()));
        Ok(result)
    }
}

/// Reorder only proven sibling lists. Groups, tables, states, and read-only
/// previews must remain contiguous blocks in their parent's flat projection.
/// Candidate changes are simulated and compared with the entire requested
/// page before any source list is changed.
fn patch_native_order(
    parts: &mut BTreeMap<u64, Graph>,
    package: &str,
    page: &Page,
    before: &Page,
    bindings: &BTreeMap<u64, Binding>,
    preview_parents: &BTreeMap<u64, u64>,
) -> Result<()> {
    let projection = native_order_projection(parts, page, before, bindings, preview_parents)?;
    let orders = projected_native_order(parts, package, page, projection)?;
    for ((offset, id, field), references) in orders {
        parts
            .get_mut(&offset)
            .unwrap()
            .objects
            .get_mut(&id)
            .unwrap()
            .properties[&field] = Value::Array(references);
    }
    Ok(())
}

fn native_order_projection<'a>(
    parts: &'a BTreeMap<u64, Graph>,
    page: &Page,
    before: &Page,
    bindings: &BTreeMap<u64, Binding>,
    preview_parents: &BTreeMap<u64, u64>,
) -> Result<NativeOrderProjection<'a>> {
    let mut previews: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for widget in &before.widgets {
        if let Some(parent) = preview_parents.get(&widget.id) {
            previews.entry(*parent).or_default().push(widget.id);
        }
    }
    let mut widgets = BTreeMap::new();
    let page_ids = page
        .widgets
        .iter()
        .chain(&before.widgets)
        .map(|widget| widget.id)
        .collect::<BTreeSet<_>>();
    for (id, binding) in bindings {
        if !page_ids.contains(id) {
            continue;
        }
        ensure!(
            widgets.insert(binding.object.clone(), *id).is_none(),
            "重复原生对象的层级映射不唯一，暂不支持重排"
        );
    }
    Ok(NativeOrderProjection {
        parts,
        widgets,
        packages: parts
            .iter()
            .map(|(offset, graph)| (graph.package.clone(), *offset))
            .collect(),
        previews,
        lists: std::cell::RefCell::new(BTreeSet::new()),
    })
}

fn projected_native_order(
    parts: &BTreeMap<u64, Graph>,
    package: &str,
    page: &Page,
    projection: NativeOrderProjection<'_>,
) -> Result<NativeListOrders> {
    let page_offset = *projection
        .packages
        .get(package)
        .context("原生页面层级不存在")?;
    let page_graph = &parts[&page_offset];
    let page_diagram = page_graph
        .root()
        .and_then(|root| root.properties.get("diagram"))
        .and_then(reference_id)
        .context("原生页面画布不存在")?;
    projection.object(
        page_offset,
        page_diagram,
        &NativeListOrders::new(),
        &mut BTreeSet::new(),
        0,
    )?;
    let reachable = projection.lists.borrow().clone();
    let positions = page
        .widgets
        .iter()
        .enumerate()
        .map(|(index, widget)| (widget.id, index))
        .collect::<BTreeMap<_, _>>();
    let mut orders = NativeListOrders::new();
    for (offset, graph) in parts.iter() {
        for (id, node) in &graph.objects {
            let field = if node.kind.ends_with(":Layer") {
                "DiagramObjects"
            } else if matches!(node.kind.as_str(), "Axure:Diagram" | "Axure:PanelDiagram") {
                "diagram-objects"
            } else {
                continue;
            };
            let Some(references) = node.properties[field].as_array() else {
                continue;
            };
            if !reachable.contains(&(*offset, id.clone(), field.to_owned())) {
                continue;
            }
            let diagram = matches!(node.kind.as_str(), "Axure:Diagram" | "Axure:PanelDiagram");
            let canonical = if diagram {
                direct_diagram_references(graph, references)?
            } else {
                references.clone()
            };
            let mut projected = Vec::with_capacity(canonical.len());
            let mut complete = true;
            for reference in &canonical {
                let Some(child) = reference_id(reference) else {
                    complete = false;
                    break;
                };
                let ids = projection.object(
                    *offset,
                    child,
                    &NativeListOrders::new(),
                    &mut BTreeSet::new(),
                    0,
                )?;
                let child_positions = ids
                    .iter()
                    .filter_map(|id| positions.get(id).copied())
                    .collect::<Vec<_>>();
                if ids.is_empty() || child_positions.len() != ids.len() {
                    complete = false;
                    break;
                }
                let first = *child_positions.iter().min().unwrap();
                let last = *child_positions.iter().max().unwrap();
                ensure!(
                    last - first + 1 == ids.len(),
                    "此层级操作会跨越原生父容器或动态面板状态；仅支持同一直接父容器内重排"
                );
                projected.push((first, reference.clone()));
            }
            if complete {
                projected.sort_by_key(|(position, _)| *position);
                let reordered = projected
                    .into_iter()
                    .map(|(_, reference)| reference)
                    .collect::<Vec<_>>();
                if reordered != canonical {
                    let reordered = if diagram {
                        rebuild_diagram_registry(
                            graph,
                            references,
                            &layer_registry_members(graph, references)?,
                            &reordered,
                        )?
                    } else {
                        reordered
                    };
                    orders.insert((*offset, id.clone(), field.into()), reordered);
                }
            }
        }
    }
    let offset = *projection
        .packages
        .get(package)
        .context("原生页面层级不存在")?;
    let graph = &parts[&offset];
    let diagram = graph
        .root()
        .and_then(|root| root.properties.get("diagram"))
        .and_then(reference_id)
        .context("原生页面画布不存在")?;
    let actual = projection.object(offset, diagram, &orders, &mut BTreeSet::new(), 0)?;
    let desired = page
        .widgets
        .iter()
        .map(|widget| widget.id)
        .collect::<Vec<_>>();
    ensure!(
        actual == desired,
        "此层级操作无法保持原生父容器、表格或状态结构；仅支持同一直接父容器内可完整映射的元件重排"
    );
    Ok(orders)
}

fn rebuild_diagram_registry(
    graph: &Graph,
    original: &[Value],
    members_before: &[Value],
    desired: &[Value],
) -> Result<Vec<Value>> {
    let old_members = members_before
        .iter()
        .filter_map(reference_id)
        .collect::<BTreeSet<_>>();
    // Preserve files that already use direct-only root lists. A diagram with
    // no existing groups uses the native registry convention for new groups.
    if !old_members.is_empty()
        && !original
            .iter()
            .any(|reference| reference_id(reference).is_some_and(|id| old_members.contains(id)))
    {
        return Ok(desired.to_vec());
    }
    let members = layer_registry_members(graph, desired)?;
    let new_members = members
        .iter()
        .filter_map(reference_id)
        .collect::<BTreeSet<_>>();
    let roots = desired
        .iter()
        .filter_map(reference_id)
        .collect::<BTreeSet<_>>();
    let originals = original
        .iter()
        .filter_map(|reference| reference_id(reference).map(|id| (id, reference)))
        .collect::<BTreeMap<_, _>>();
    let mut direct = desired.iter();
    let mut emitted = BTreeSet::new();
    let mut result = Vec::new();
    for reference in original {
        let id = reference_id(reference).context("原生注册表包含无法识别的引用")?;
        if old_members.contains(id) {
            if new_members.contains(id) && !roots.contains(id) {
                ensure!(emitted.insert(id.to_owned()), "原生注册表存在重复对象引用");
                result.push(reference.clone());
            }
        } else if let Some(reference) = direct.next() {
            let id = reference_id(reference).context("原生父层级引用无效")?;
            ensure!(
                emitted.insert(id.to_owned()),
                "原生注册表存在重复父对象引用"
            );
            result.push(reference.clone());
        }
    }
    for reference in direct {
        let id = reference_id(reference).context("原生父层级引用无效")?;
        ensure!(
            emitted.insert(id.to_owned()),
            "原生注册表存在重复父对象引用"
        );
        result.push(reference.clone());
    }
    for reference in &members {
        let id = reference_id(reference).context("原生组合成员引用无效")?;
        if emitted.insert(id.to_owned()) {
            result.push(originals.get(id).copied().unwrap_or(reference).clone());
        }
    }
    Ok(result)
}

struct NativeGroupAtom {
    reference: Value,
    groups: Vec<u64>,
    widgets: Vec<u64>,
}

struct NativeGroupMetadata {
    layers: BTreeMap<String, u64>,
    references: BTreeMap<u64, Value>,
}

fn collect_group_atoms(
    projection: &NativeOrderProjection<'_>,
    offset: u64,
    references: &[Value],
    metadata: &mut NativeGroupMetadata,
    path: &[u64],
    atoms: &mut Vec<NativeGroupAtom>,
    depth: usize,
) -> Result<()> {
    ensure!(depth <= 32, "原生组合嵌套超过限制");
    let graph = &projection.parts[&offset];
    for reference in references {
        let id = reference_id(reference).context("原生组合包含无法识别的引用")?;
        let node = graph.node(id).context("原生组合对象不存在")?;
        if node.kind.ends_with(":Layer") {
            let group = *metadata
                .layers
                .get(&format!("{}/{id}", graph.package))
                .context("原生组缺少编辑器绑定")?;
            metadata.references.insert(group, reference.clone());
            let mut nested = path.to_vec();
            nested.push(group);
            let start = atoms.len();
            collect_group_atoms(
                projection,
                offset,
                array(&node.properties["DiagramObjects"]),
                metadata,
                &nested,
                atoms,
                depth + 1,
            )?;
            if atoms[start..].iter().any(|atom| !atom.widgets.is_empty()) {
                continue;
            }
            // An entirely invisible subtree is one opaque source object. Its
            // descendants must stay attached even when unrelated widgets group.
            atoms.truncate(start);
        }
        atoms.push(NativeGroupAtom {
            reference: reference.clone(),
            groups: path.to_vec(),
            widgets: projection.object(
                offset,
                id,
                &NativeListOrders::new(),
                &mut BTreeSet::new(),
                0,
            )?,
        });
    }
    Ok(())
}

enum NativeGroupEntry {
    Object(Value),
    Group(u64, Vec<NativeGroupEntry>),
}

fn append_group_entry(entries: &mut Vec<NativeGroupEntry>, path: &[u64], reference: Value) {
    if let Some((&group, rest)) = path.split_first() {
        if !matches!(entries.last(), Some(NativeGroupEntry::Group(id,_)) if *id == group) {
            entries.push(NativeGroupEntry::Group(group, Vec::new()));
        }
        let Some(NativeGroupEntry::Group(_, children)) = entries.last_mut() else {
            unreachable!();
        };
        append_group_entry(children, rest, reference);
    } else {
        entries.push(NativeGroupEntry::Object(reference));
    }
}

fn group_entry_contains(
    entry: &NativeGroupEntry,
    id: &str,
    references: &BTreeMap<u64, Value>,
) -> bool {
    match entry {
        NativeGroupEntry::Object(reference) => reference_id(reference) == Some(id),
        NativeGroupEntry::Group(group, children) => {
            references.get(group).and_then(reference_id) == Some(id)
                || children
                    .iter()
                    .any(|child| group_entry_contains(child, id, references))
        }
    }
}

fn group_entries_mut(
    entries: &mut Vec<NativeGroupEntry>,
    group: u64,
) -> Option<&mut Vec<NativeGroupEntry>> {
    for entry in entries {
        if let NativeGroupEntry::Group(id, children) = entry {
            if *id == group {
                return Some(children);
            }
            if let Some(found) = group_entries_mut(children, group) {
                return Some(found);
            }
        }
    }
    None
}

// Objects with no editor projection still occupy a source sibling slot. Keep
// their exact reference in their original parent; a newly wrapped visible block
// takes the last selected source slot, so intervening opaque siblings precede it.
fn preserve_unmapped_group_atoms(
    graph: &Graph,
    direct: &[Value],
    metadata: &NativeGroupMetadata,
    atoms: &[NativeGroupAtom],
    entries: &mut Vec<NativeGroupEntry>,
) -> Result<()> {
    for atom in atoms.iter().filter(|atom| atom.widgets.is_empty()) {
        let siblings = if let Some(parent) = atom.groups.last() {
            let parent = metadata
                .references
                .get(parent)
                .and_then(reference_id)
                .and_then(|id| graph.node(id))
                .context("无法映射的对象缺少原生父组合")?;
            array(&parent.properties["DiagramObjects"])
        } else {
            direct
        };
        let id = reference_id(&atom.reference).context("原生未知对象引用无效")?;
        let source_position = siblings
            .iter()
            .position(|reference| reference_id(reference) == Some(id))
            .context("无法映射的对象不在原生父组合中")?;
        let target = if let Some(parent) = atom.groups.last() {
            group_entries_mut(entries, *parent)
                .context("原生组合含有空组合或未知对象，无法安全解除或移出该父组合")?
        } else {
            &mut *entries
        };
        let next = siblings[source_position + 1..]
            .iter()
            .filter_map(reference_id)
            .find_map(|id| {
                target
                    .iter()
                    .position(|entry| group_entry_contains(entry, id, &metadata.references))
            });
        let position = next.unwrap_or(target.len());
        target.insert(position, NativeGroupEntry::Object(atom.reference.clone()));
    }
    Ok(())
}

fn new_native_group(graph: &mut Graph, diagram: &str, children: Vec<Value>) -> Result<String> {
    let id = guid();
    let style_id = guid();
    let template = graph
        .objects
        .iter()
        .find(|(_, node)| node.kind.ends_with(":Layer"))
        .map(|(id, node)| (id.clone(), node.clone()));
    if let Some((template_id, template)) = &template {
        let source = graph.clone();
        graph.copy_node_from(&source, template_id, &id)?;
        let style = template.properties.get("MyStyle").and_then(reference_id);
        if let Some(style) = style.filter(|style| source.node(style).is_some()) {
            graph.copy_node_from(&source, style, &style_id)?;
        }
    }
    let node = graph.objects.entry(id.clone()).or_insert_with(|| Node {
        kind: "Axure:DiagramObject:Layer".into(),
        properties: json!({}),
    });
    if let Some((template_id, _)) = template {
        replace_reference(&mut node.properties, &template_id, &id);
    }
    node.properties["DiagramObjects"] = Value::Array(children);
    node.properties["name-block"] = json!({"*type":"text-block","text":"组合","property-dictionary":{
        "type":"label","diagram-object-id":{"$guid":id},"parent-diagram-id":{"$guid":diagram}}});
    node.properties["MyStyle"] = json!({"$ref":style_id,"$token":3});
    node.properties["Annotation"] = json!({"$ref":EMPTY_ID,"$token":3});
    node.properties["AdaptiveStyles"] = json!([]);
    node.properties["SubmitButton"] = json!({"$ref":EMPTY_ID,"$token":3});
    node.properties["SelectedStyleGroup"] = json!("");
    node.properties["tooltip"] = json!("");
    for field in ["IsLocked", "IsContained", "Disabled", "Selected"] {
        node.properties[field] = json!(false);
    }
    node.properties["VisibleFromSettings"] = json!(true);
    node.properties["Propagate"] = json!(true);
    node.properties
        .as_object_mut()
        .unwrap()
        .remove("InteractionMap");
    let style = graph
        .objects
        .entry(style_id.clone())
        .or_insert_with(|| Node {
            kind: "Axure:Style".into(),
            properties: json!({"Capabilities":0,"All":false,"style-name":""}),
        });
    style.properties["Id"] = json!({"$guid":style_id});
    style.properties["PropList"] = json!([]);
    Ok(id)
}

fn write_group_entries(
    graph: &mut Graph,
    diagram: &str,
    entries: &[NativeGroupEntry],
    groups: &BTreeMap<u64, GroupBinding>,
    references: &BTreeMap<u64, Value>,
) -> Result<Vec<Value>> {
    let mut result = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            NativeGroupEntry::Object(reference) => result.push(reference.clone()),
            NativeGroupEntry::Group(group, children) => {
                let children = write_group_entries(graph, diagram, children, groups, references)?;
                if let Some(binding) = groups.get(group) {
                    let node = graph
                        .objects
                        .get_mut(raw_id(&binding.object))
                        .context("原生组合绑定无效")?;
                    node.properties["DiagramObjects"] = Value::Array(children);
                    result.push(references.get(group).context("原生组父引用丢失")?.clone());
                } else {
                    let id = new_native_group(graph, diagram, children)?;
                    result.push(json!({"$ref":id,"$token":3}));
                }
            }
        }
    }
    Ok(result)
}

fn patch_native_groups(
    parts: &mut BTreeMap<u64, Graph>,
    package: &str,
    page: &Page,
    before: &Page,
    bindings: &BTreeMap<u64, Binding>,
    preview_parents: &BTreeMap<u64, u64>,
    groups: &BTreeMap<u64, GroupBinding>,
) -> Result<()> {
    let projection = native_order_projection(parts, page, before, bindings, preview_parents)?;
    let offset = *projection
        .packages
        .get(package)
        .context("组合页面部件不存在")?;
    let graph = &parts[&offset];
    let diagram = graph
        .root()
        .and_then(|root| root.properties.get("diagram"))
        .and_then(reference_id)
        .context("组合页面画布不存在")?
        .to_owned();
    let mut metadata = NativeGroupMetadata {
        layers: BTreeMap::new(),
        references: BTreeMap::new(),
    };
    for (group, binding) in groups {
        if binding.object.starts_with(&format!("{package}/")) {
            ensure!(
                metadata
                    .layers
                    .insert(binding.object.clone(), *group)
                    .is_none(),
                "原生组合映射不唯一"
            );
        }
    }
    let mut atoms = Vec::new();
    let original_registry = array(&graph.objects[&diagram].properties["diagram-objects"]).to_vec();
    let members_before = layer_registry_members(graph, &original_registry)?;
    let direct = direct_diagram_references(graph, &original_registry)?;
    collect_group_atoms(
        &projection,
        offset,
        &direct,
        &mut metadata,
        &[],
        &mut atoms,
        0,
    )?;
    let allowed = metadata.references.keys().copied().collect::<BTreeSet<_>>();
    let previous = before
        .widgets
        .iter()
        .map(|widget| (widget.id, widget))
        .collect::<BTreeMap<_, _>>();
    let current = page
        .widgets
        .iter()
        .enumerate()
        .map(|(index, widget)| (widget.id, (index, widget)))
        .collect::<BTreeMap<_, _>>();
    let mut positioned = Vec::with_capacity(atoms.len());
    for atom in atoms.iter().filter(|atom| !atom.widgets.is_empty()) {
        let mut target_path: Option<Vec<u64>> = None;
        let mut positions = Vec::new();
        for id in &atom.widgets {
            let (position, widget) = current.get(id).context("组合操作跨越了原生父容器")?;
            positions.push(*position);
            let suffix = previous
                .get(id)
                .map(|old| old.groups.strip_prefix(atom.groups.as_slice()))
                .unwrap_or(Some(&[]))
                .context("原生组合层级与编辑器不一致")?;
            ensure!(
                widget.groups.ends_with(suffix),
                "暂不支持跨表格、母版或动态面板状态改变组合结构"
            );
            let path = widget.groups[..widget.groups.len() - suffix.len()].to_vec();
            ensure!(
                target_path.as_ref().is_none_or(|target| *target == path),
                "暂不支持拆分表格、母版或动态面板状态中的部分元件进行组合"
            );
            target_path = Some(path);
        }
        let first = *positions.iter().min().unwrap();
        let last = *positions.iter().max().unwrap();
        ensure!(
            last - first + 1 == atom.widgets.len(),
            "组合操作会拆散原生复杂容器，未修改源文件"
        );
        let path = target_path.unwrap();
        ensure!(
            path.iter()
                .all(|id| !groups.contains_key(id) || allowed.contains(id)),
            "暂不支持跨动态面板状态或页面移动原生组合"
        );
        positioned.push((first, path, atom.reference.clone()));
    }
    positioned.sort_by_key(|(position, _, _)| *position);
    let mut entries = Vec::new();
    for (_, path, reference) in positioned {
        append_group_entry(&mut entries, &path, reference);
    }
    preserve_unmapped_group_atoms(graph, &direct, &metadata, &atoms, &mut entries)?;
    drop(projection);
    let graph = parts.get_mut(&offset).unwrap();
    let references = write_group_entries(graph, &diagram, &entries, groups, &metadata.references)?;
    let references =
        rebuild_diagram_registry(graph, &original_registry, &members_before, &references)?;
    graph.objects.get_mut(&diagram).unwrap().properties["diagram-objects"] =
        Value::Array(references);
    Ok(())
}

fn transform_preview(child: &Widget, before: &Widget, after: &Widget) -> Widget {
    let mut result = child.clone();
    let same_geometry = before.x == after.x
        && before.y == after.y
        && before.width == after.width
        && before.height == after.height
        && before.rotation == after.rotation;
    if !same_geometry {
        let sx = after.width / before.width;
        let sy = after.height / before.height;
        let (old_sin, old_cos) = (-before.rotation).to_radians().sin_cos();
        let (new_sin, new_cos) = after.rotation.to_radians().sin_cos();
        let dx = child.x + child.width * 0.5 - before.x - before.width * 0.5;
        let dy = child.y + child.height * 0.5 - before.y - before.height * 0.5;
        let local_x = (dx * old_cos - dy * old_sin) * sx;
        let local_y = (dx * old_sin + dy * old_cos) * sy;
        let (sin, cos) = (child.rotation - before.rotation).to_radians().sin_cos();
        let width_scale = (sx * cos).hypot(sy * sin);
        let height_scale = (sx * sin).hypot(sy * cos);
        result.width = (child.width * width_scale).clamp(1.0, 20_000.0);
        result.height = (child.height * height_scale).clamp(1.0, 20_000.0);
        result.x = after.x + after.width * 0.5 + local_x * new_cos
            - local_y * new_sin
            - result.width * 0.5;
        result.y = after.y + after.height * 0.5 + local_x * new_sin + local_y * new_cos
            - result.height * 0.5;
        result.rotation =
            (after.rotation + (sy * sin).atan2(sx * cos).to_degrees()).rem_euclid(360.0);
        result.font_size = (child.font_size * (sx * sy).sqrt()).clamp(4.0, 512.0);
        result.radius = (child.radius * sx.min(sy)).clamp(0.0, 20_000.0);
    }
    if before.hidden != after.hidden {
        result.hidden = after.hidden || (child.hidden && !before.hidden);
    }
    if before.groups != after.groups && child.groups.starts_with(&before.groups) {
        result.groups = after
            .groups
            .iter()
            .copied()
            .chain(child.groups[before.groups.len()..].iter().copied())
            .collect();
    }
    result
}

fn matching_preview(actual: &Widget, expected: &Widget) -> bool {
    let mut compared = actual.clone();
    for (actual, expected) in [
        (&mut compared.x, expected.x),
        (&mut compared.y, expected.y),
        (&mut compared.width, expected.width),
        (&mut compared.height, expected.height),
        (&mut compared.rotation, expected.rotation),
        (&mut compared.font_size, expected.font_size),
        (&mut compared.radius, expected.radius),
    ] {
        if (*actual - expected).abs() > expected.abs().max(1.0) * 0.00001 {
            return false;
        }
        *actual = expected;
    }
    compared == *expected
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn new_native_forms_require_their_own_template_and_options_are_typed_records() {
        for kind in [
            WidgetKind::Radio,
            WidgetKind::TextArea,
            WidgetKind::Droplist,
            WidgetKind::ListBox,
        ] {
            let mut parts = BTreeMap::new();
            let widget = Widget::new(2, kind, 0.0, 0.0);
            let error = add_widget(&mut parts, "page", &widget, &BTreeMap::new()).unwrap_err();
            assert!(error.to_string().contains("原生模板"));
            assert!(parts.is_empty());
            let mut node = Node {
                kind: "Axure:DiagramObject:VectorShape".into(),
                properties: json!({}),
            };
            assert!(set_kind(&mut node, kind).is_err());
        }
        let mut graph = Graph::new(
            [11, 0, 0, 0],
            EMPTY_ID.into(),
            "11111111111111111111111111111111".into(),
            BTreeMap::new(),
        );
        let mut node = Node {
            kind: "Axure:DiagramObject:ComboBox".into(),
            properties: json!({}),
        };
        set_list_options(&mut graph, &mut node, "First\nSecond");
        assert_eq!(
            list_options_text(&node.properties, &graph).as_deref(),
            Some("First\nSecond")
        );
        let options = node.properties["list-options"].as_array().unwrap();
        assert_eq!(
            graph.resolve(&options[0]).unwrap().properties["selected"],
            true
        );
        assert_eq!(
            graph.resolve(&options[1]).unwrap().properties["selected"],
            false
        );
        graph.objects.insert(EMPTY_ID.into(), node);
        let encoded = binary::encode(&graph).unwrap();
        let parsed = binary::parse(&encoded).unwrap();
        assert_eq!(
            list_options_text(&parsed.root().unwrap().properties, &parsed).as_deref(),
            Some("First\nSecond")
        );
    }

    #[test]
    fn installed_native_form_templates_save_to_copies_with_true_types() {
        let base = Path::new(r"D:\Program Files (x86)\Axure\Axure RP 11\DefaultSettings");
        let library = base.join("Libraries/Default.rplib");
        let project_path = base.join("Training/Quick Win.rp");
        if !library.is_file() || !project_path.is_file() {
            return;
        }
        let directory =
            std::env::temp_dir().join(format!("rpshop-forms-{}-{}", std::process::id(), guid()));
        std::fs::create_dir(&directory).unwrap();
        let source = std::fs::read(&library).unwrap();
        let mut native = open(&library).unwrap();
        let mut edited = native.project.clone();
        for kind in [
            WidgetKind::Radio,
            WidgetKind::TextArea,
            WidgetKind::Droplist,
            WidgetKind::ListBox,
        ] {
            let widget = edited
                .pages
                .iter_mut()
                .flat_map(|page| &mut page.widgets)
                .find(|widget| widget.kind == kind)
                .unwrap();
            widget.text = if kind.is_option_list() {
                "First\nSecond"
            } else {
                "RPShop form text"
            }
            .into();
            widget.name = format!("RPShop {}", kind.label());
            if kind == WidgetKind::Radio {
                widget.radio_group = Some("RPShop group".into());
            }
        }
        let destination = directory.join("forms.rplib");
        native.save(&edited, &destination).unwrap();
        let reopened = open(&destination).unwrap();
        for kind in [
            WidgetKind::Radio,
            WidgetKind::TextArea,
            WidgetKind::Droplist,
            WidgetKind::ListBox,
        ] {
            let widget = reopened
                .project
                .pages
                .iter()
                .flat_map(|page| &page.widgets)
                .find(|widget| widget.kind == kind)
                .unwrap();
            assert!(
                widget
                    .name
                    .ends_with(&format!(" / RPShop {}", kind.label())),
                "library gallery names retain their component prefix: {}",
                widget.name
            );
            assert_eq!(
                widget.text,
                if kind.is_option_list() {
                    "First\nSecond"
                } else {
                    "RPShop form text"
                }
            );
            if kind == WidgetKind::Radio {
                assert_eq!(widget.radio_group.as_deref(), Some("RPShop group"));
            }
        }
        assert_eq!(std::fs::read(&library).unwrap(), source);

        let source = std::fs::read(&project_path).unwrap();
        let mut native = open(&project_path).unwrap();
        let mut edited = native.project.clone();
        let id = edited.allocate_id();
        let mut widget = Widget::new(id, WidgetKind::Radio, 120.0, 160.0);
        widget.name = "RPShop radio template".into();
        widget.radio_group = Some("RPShop new group".into());
        widget.text = "Native radio".into();
        edited.pages[0].widgets.push(widget);
        let destination = directory.join("forms.rp");
        native.save(&edited, &destination).unwrap();
        let reopened = open(&destination).unwrap();
        let widget = reopened.project.pages[0]
            .widgets
            .iter()
            .find(|widget| widget.name == "RPShop radio template")
            .unwrap();
        assert_eq!(widget.kind, WidgetKind::Radio);
        assert_eq!(widget.text, "Native radio");
        assert_eq!(widget.radio_group.as_deref(), Some("RPShop new group"));
        assert_eq!(std::fs::read(&project_path).unwrap(), source);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn new_widget_ids_remain_editable_before_native_save() {
        let mut baseline = Project::blank();
        let id = baseline.allocate_id();
        baseline.pages[0]
            .widgets
            .push(Widget::new(id, WidgetKind::Rectangle, 0.0, 0.0));
        let mut bindings = BTreeMap::new();
        assert!(
            !editable_native_id(&baseline, &bindings, id),
            "known read-only previews stay read-only"
        );
        assert!(
            editable_native_id(&baseline, &bindings, baseline.next_id),
            "new unsaved widgets must be editable"
        );
        bindings.insert(
            id,
            Binding {
                object: "source/object".into(),
                scale: 1.0,
            },
        );
        assert!(editable_native_id(&baseline, &bindings, id));
    }

    #[test]
    fn native_vector_mirror_requires_matching_geometry_and_preview() {
        let points = json!([[{"Path":{"X":0.0,"Y":0.0},"LowerHandle":null,"HigherHandle":null,"HigherSegmentExists":true},
            {"Path":{"X":1.0,"Y":1.0},"LowerHandle":null,"HigherHandle":null,"HigherSegmentExists":false}]]);
        let original = Node {
            kind: "Axure:DiagramObject:VectorShape".into(),
            properties: json!({"ControlPoints":points}),
        };
        let mut old = Widget::new(1, WidgetKind::Image, 0.0, 0.0);
        old.vector_points = Some(points.clone());
        old.rotation = 25.0;
        let image = image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        old.image_data = Some(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(encoded.into_inner())
        ));
        let mut changed = old.clone();
        crate::vector_edit::flip_widget(&mut changed, true).unwrap();
        validate_native_vector_edit(&original, &old, &changed).unwrap();
        let mut wrong = changed.clone();
        wrong.image_data = old.image_data.clone();
        assert!(validate_native_vector_edit(&original, &old, &wrong).is_err());
        wrong.vector_points.as_mut().unwrap()[0][0]["Path"]["X"] = json!(0.37);
        assert!(validate_native_vector_edit(&original, &old, &wrong).is_err());
        crate::vector_edit::flip_widget(&mut changed, false).unwrap();
        validate_native_vector_edit(&original, &old, &changed).unwrap();
        crate::vector_edit::flip_widget(&mut changed, true).unwrap();
        validate_native_vector_edit(&original, &old, &changed).unwrap();
    }

    fn order_fixture() -> (BTreeMap<u64, Graph>, Page, BTreeMap<u64, Binding>) {
        let id = |value: u64| format!("{value:032x}");
        let package = id(100);
        let nodes = [
            (101, "Axure:Page", json!({"diagram":{"$ref":id(102)}})),
            (
                102,
                "Axure:Diagram",
                json!({"diagram-objects":[
                {"$ref":id(1),"$token":3},{"$ref":id(103),"$token":3},
                {"$ref":id(4),"$token":3},{"$ref":id(104),"$token":3},{"$ref":id(7),"$token":3}],
                "unrecognized-setting":{"keep":42}}),
            ),
            (
                103,
                "Axure:DiagramObject:Layer",
                json!({"DiagramObjects":[{"$ref":id(2),"$token":3},{"$ref":id(3),"$token":3}],"unknown-group-data":17}),
            ),
            (
                104,
                "Axure:DiagramObject:Table",
                json!({"diagram-objects":[{"$ref":id(5)},{"$ref":id(6)}],"cells":[{"$ref":id(5)},{"$ref":id(6)}]}),
            ),
        ];
        let mut objects = nodes
            .into_iter()
            .map(|(i, kind, properties)| {
                (
                    id(i),
                    Node {
                        kind: kind.into(),
                        properties,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        for number in 1..=7 {
            objects.insert(
                id(number),
                Node {
                    kind: "Axure:DiagramObject:VectorShape".into(),
                    properties: json!({"Shape":0}),
                },
            );
        }
        let graph = Graph::new([11, 0, 0, 0], id(101), package.clone(), objects);
        let page = Page {
            id: 200,
            name: "Order fixture".into(),
            width: 1000.0,
            height: 1000.0,
            canvas: crate::model::CanvasMode::Fixed,
            background: "#FFFFFF".into(),
            widgets: (1..=7)
                .map(|i| Widget::new(i, WidgetKind::Rectangle, 0.0, 0.0))
                .collect(),
        };
        let bindings = (1..=7)
            .map(|i| {
                (
                    i,
                    Binding {
                        object: format!("{package}/{}", id(i)),
                        scale: 1.0,
                    },
                )
            })
            .collect();
        (BTreeMap::from([(10, graph)]), page, bindings)
    }

    fn reordered(page: &Page, ids: &[u64]) -> Page {
        let mut result = page.clone();
        result.widgets = ids
            .iter()
            .map(|id| page.widgets.iter().find(|w| w.id == *id).unwrap().clone())
            .collect();
        result
    }

    #[test]
    fn native_order_supports_root_siblings_beside_layers_and_tables() {
        let (mut parts, before, bindings) = order_fixture();
        let package = parts[&10].package.clone();
        let table_before = parts[&10]
            .node(&format!("{:032x}", 104))
            .unwrap()
            .properties
            .clone();
        let after = reordered(&before, &[4, 2, 3, 1, 5, 6, 7]);
        patch_native_order(
            &mut parts,
            &package,
            &after,
            &before,
            &bindings,
            &BTreeMap::new(),
        )
        .unwrap();
        let diagram = &parts[&10]
            .node(&format!("{:032x}", 102))
            .unwrap()
            .properties;
        let order = array(&diagram["diagram-objects"])
            .iter()
            .map(|v| reference_id(v).unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(order, [4, 103, 1, 104, 7].map(|n| format!("{n:032x}")));
        assert_eq!(diagram["unrecognized-setting"]["keep"], 42);
        assert_eq!(diagram["diagram-objects"][0]["$token"], 3);
        assert_eq!(
            parts[&10]
                .node(&format!("{:032x}", 104))
                .unwrap()
                .properties,
            table_before
        );
        let bytes = binary::encode(&parts[&10]).unwrap();
        assert_eq!(
            binary::encode(&binary::parse(&bytes).unwrap()).unwrap(),
            bytes
        );
    }

    #[test]
    fn native_order_reorders_layer_siblings_without_changing_root_refs() {
        let (mut parts, before, bindings) = order_fixture();
        let package = parts[&10].package.clone();
        let root_before = parts[&10]
            .node(&format!("{:032x}", 102))
            .unwrap()
            .properties
            .clone();
        let after = reordered(&before, &[1, 3, 2, 4, 5, 6, 7]);
        patch_native_order(
            &mut parts,
            &package,
            &after,
            &before,
            &bindings,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            parts[&10]
                .node(&format!("{:032x}", 102))
                .unwrap()
                .properties,
            root_before
        );
        let group = &parts[&10]
            .node(&format!("{:032x}", 103))
            .unwrap()
            .properties;
        assert_eq!(
            reference_id(&group["DiagramObjects"][0]),
            Some(format!("{:032x}", 3).as_str())
        );
        assert_eq!(group["unknown-group-data"], 17);
    }

    #[test]
    fn native_order_rejects_cross_parent_and_table_internal_changes_atomically() {
        for ids in [[2, 1, 3, 4, 5, 6, 7], [1, 2, 3, 4, 6, 5, 7]] {
            let (mut parts, before, bindings) = order_fixture();
            let original = binary::encode(&parts[&10]).unwrap();
            let package = parts[&10].package.clone();
            let after = reordered(&before, &ids);
            assert!(
                patch_native_order(
                    &mut parts,
                    &package,
                    &after,
                    &before,
                    &bindings,
                    &BTreeMap::new()
                )
                .is_err()
            );
            assert_eq!(binary::encode(&parts[&10]).unwrap(), original);
        }
    }

    type GroupedOrderFixture = (
        BTreeMap<u64, Graph>,
        Page,
        BTreeMap<u64, Binding>,
        BTreeMap<u64, GroupBinding>,
    );
    fn grouped_order_fixture() -> GroupedOrderFixture {
        let (parts, mut page, bindings) = order_fixture();
        for widget in &mut page.widgets {
            if matches!(widget.id, 2 | 3) {
                widget.groups = vec![300];
            }
        }
        let groups = BTreeMap::from([(
            300,
            GroupBinding {
                object: format!("{}/{:032x}", parts[&10].package, 103),
            },
        )]);
        (parts, page, bindings, groups)
    }

    #[test]
    fn native_groups_wrap_existing_layers_and_preserve_opaque_tables() {
        let (mut parts, before, bindings, groups) = grouped_order_fixture();
        let package = parts[&10].package.clone();
        let table = parts[&10]
            .node(&format!("{:032x}", 104))
            .unwrap()
            .properties
            .clone();
        let mut after = before.clone();
        for widget in &mut after.widgets {
            if matches!(widget.id, 1..=3) {
                widget.groups.insert(0, 301);
            }
        }
        patch_native_groups(
            &mut parts,
            &package,
            &after,
            &before,
            &bindings,
            &BTreeMap::new(),
            &groups,
        )
        .unwrap();
        patch_native_order(
            &mut parts,
            &package,
            &after,
            &before,
            &bindings,
            &BTreeMap::new(),
        )
        .unwrap();
        let graph = &parts[&10];
        let root =
            array(&graph.node(&format!("{:032x}", 102)).unwrap().properties["diagram-objects"]);
        let group = graph.resolve(&root[0]).unwrap();
        assert!(group.kind.ends_with(":Layer"));
        assert_eq!(
            reference_id(&group.properties["DiagramObjects"][0]),
            Some(format!("{:032x}", 1).as_str())
        );
        assert_eq!(
            reference_id(&group.properties["DiagramObjects"][1]),
            Some(format!("{:032x}", 103).as_str())
        );
        assert_eq!(
            graph.node(&format!("{:032x}", 104)).unwrap().properties,
            table
        );
        assert_eq!(
            graph.node(&format!("{:032x}", 103)).unwrap().properties["unknown-group-data"],
            17
        );
        let bytes = binary::encode(graph).unwrap();
        assert_eq!(
            binary::encode(&binary::parse(&bytes).unwrap()).unwrap(),
            bytes
        );
    }

    #[test]
    fn native_group_changes_preserve_invisible_subtrees_and_registry_references() {
        let (mut parts, before, bindings, mut groups) = grouped_order_fixture();
        let id = |value: u64| format!("{value:032x}");
        let package = parts[&10].package.clone();
        let outer = json!({"$ref":id(150),"$token":3,"unknown-reference-setting":47});
        let inner = json!({"$ref":id(151),"$token":3});
        let graph = parts.get_mut(&10).unwrap();
        graph.objects.insert(
            id(150),
            Node {
                kind: "Axure:DiagramObject:Layer".into(),
                properties: json!({"DiagramObjects":[inner],"unknown-setting":19}),
            },
        );
        graph.objects.insert(
            id(151),
            Node {
                kind: "Axure:DiagramObject:Layer".into(),
                properties: json!({"DiagramObjects":[],"unknown-setting":23}),
            },
        );
        let registry = graph.objects.get_mut(&id(102)).unwrap().properties["diagram-objects"]
            .as_array_mut()
            .unwrap();
        registry.insert(1, outer.clone());
        registry.push(inner.clone());
        for (group, object) in [(400, 150), (401, 151)] {
            groups.insert(
                group,
                GroupBinding {
                    object: format!("{package}/{}", id(object)),
                },
            );
        }
        let untouched = [
            graph.node(&id(150)).unwrap().properties.clone(),
            graph.node(&id(151)).unwrap().properties.clone(),
        ];
        let mut after = before.clone();
        for widget in &mut after.widgets {
            if matches!(widget.id, 1..=3) {
                widget.groups.insert(0, 301);
            }
        }
        patch_native_groups(
            &mut parts,
            &package,
            &after,
            &before,
            &bindings,
            &BTreeMap::new(),
            &groups,
        )
        .unwrap();
        patch_native_order(
            &mut parts,
            &package,
            &after,
            &before,
            &bindings,
            &BTreeMap::new(),
        )
        .unwrap();
        let graph = &parts[&10];
        let registry = array(&graph.node(&id(102)).unwrap().properties["diagram-objects"]);
        let direct = direct_diagram_references(graph, registry).unwrap();
        assert_eq!(
            direct[0], outer,
            "opaque sibling stays outside the newly wrapped group"
        );
        assert_eq!(
            registry
                .iter()
                .find(|reference| reference_id(reference) == Some(id(151).as_str())),
            Some(&inner)
        );
        for (index, object) in [150, 151].into_iter().enumerate() {
            assert_eq!(
                graph.node(&id(object)).unwrap().properties,
                untouched[index]
            );
        }
        assert_eq!(layer_registry_members(graph, registry).unwrap().len(), 5);
    }

    #[test]
    fn native_ungroup_expands_refs_without_editing_child_geometry_or_unknown_group_data() {
        let (mut parts, before, bindings, groups) = grouped_order_fixture();
        let package = parts[&10].package.clone();
        let old_group = parts[&10]
            .node(&format!("{:032x}", 103))
            .unwrap()
            .properties
            .clone();
        let old_child = parts[&10]
            .node(&format!("{:032x}", 2))
            .unwrap()
            .properties
            .clone();
        let mut after = before.clone();
        for widget in &mut after.widgets {
            widget.groups.clear();
        }
        patch_native_groups(
            &mut parts,
            &package,
            &after,
            &before,
            &bindings,
            &BTreeMap::new(),
            &groups,
        )
        .unwrap();
        patch_native_order(
            &mut parts,
            &package,
            &after,
            &before,
            &bindings,
            &BTreeMap::new(),
        )
        .unwrap();
        let root = &parts[&10]
            .node(&format!("{:032x}", 102))
            .unwrap()
            .properties["diagram-objects"];
        assert_eq!(reference_id(&root[1]), Some(format!("{:032x}", 2).as_str()));
        assert_eq!(reference_id(&root[2]), Some(format!("{:032x}", 3).as_str()));
        assert_eq!(
            parts[&10]
                .node(&format!("{:032x}", 103))
                .unwrap()
                .properties,
            old_group
        );
        assert_eq!(
            parts[&10].node(&format!("{:032x}", 2)).unwrap().properties,
            old_child
        );
    }

    #[test]
    fn native_groups_reject_extracting_one_table_cell_without_mutation() {
        let (mut parts, before, bindings, groups) = grouped_order_fixture();
        let original = binary::encode(&parts[&10]).unwrap();
        let package = parts[&10].package.clone();
        let mut after = before.clone();
        after.widgets.iter_mut().find(|w| w.id == 5).unwrap().groups = vec![301];
        assert!(
            patch_native_groups(
                &mut parts,
                &package,
                &after,
                &before,
                &bindings,
                &BTreeMap::new(),
                &groups
            )
            .is_err()
        );
        assert_eq!(binary::encode(&parts[&10]).unwrap(), original);
    }

    fn close(left: f32, right: f32) {
        assert!((left - right).abs() < 0.001, "{left} != {right}");
    }

    #[test]
    fn master_preview_follows_translation_rotation_and_uniform_scale() {
        let mut parent = Widget::new(1, WidgetKind::Panel, 100.0, 100.0);
        parent.width = 200.0;
        parent.height = 100.0;
        let mut child = Widget::new(2, WidgetKind::Rectangle, 120.0, 110.0);
        child.width = 40.0;
        child.height = 20.0;
        child.locked = true;
        let mut moved = parent.clone();
        moved.x += 50.0;
        moved.y -= 20.0;
        let preview = transform_preview(&child, &parent, &moved);
        close(preview.x, 170.0);
        close(preview.y, 90.0);
        assert!(preview.locked);
        let mut rotated = parent.clone();
        rotated.rotation = 90.0;
        let preview = transform_preview(&child, &parent, &rotated);
        close(preview.x, 210.0);
        close(preview.y, 80.0);
        close(preview.rotation, 90.0);
        let mut scaled = parent.clone();
        scaled.width *= 2.0;
        scaled.height *= 2.0;
        let preview = transform_preview(&child, &parent, &scaled);
        close(preview.x, 140.0);
        close(preview.y, 120.0);
        close(preview.width, 80.0);
        close(preview.height, 40.0);
    }

    #[test]
    fn unchanged_master_preview_has_exactly_unchanged_floats() {
        let mut parent = Widget::new(1, WidgetKind::Panel, 100.25, 99.8);
        parent.rotation = 27.333;
        let mut child = Widget::new(2, WidgetKind::Rectangle, 121.7, 84.31);
        child.rotation = -12.35;
        assert_eq!(transform_preview(&child, &parent, &parent), child);
    }

    #[test]
    fn updates_legacy_rich_text_without_changing_its_schema() {
        let mut properties = json!({"rtf-block": {
            "*type":"rich-text-block", "property-dictionary":{"type":"text-object"},
            "attributed-string":{"Version":{"Major":8,"Minor":0,"Build":5,"Revision":0},
            "Paragraphs":[{"HorizontalAlignment":1,"Inlines":[{"Text":"Before","Size":10.0}]}]}
        }});
        set_text(&mut properties, "After\nNext");
        assert_eq!(widget_text(&properties), "After\nNext");
        assert!(properties.get("RichTextBlock").is_none());
        assert_eq!(
            properties["rtf-block"]["attributed-string"]["Version"]["Major"],
            8
        );
        assert_eq!(
            properties["rtf-block"]["property-dictionary"]["type"],
            "text-object"
        );
        assert_eq!(
            properties["rtf-block"]["attributed-string"]["Paragraphs"][1]["HorizontalAlignment"],
            1
        );
    }

    fn check_native_fixture(variable: &str, legacy: bool) {
        let Some(path) = std::env::var_os(variable) else {
            return;
        };
        let path = PathBuf::from(path);
        let original = std::fs::read(&path).expect("read caller-selected fixture");
        let directory = std::env::temp_dir().join(format!("rpshop-compatibility-{}", guid()));
        std::fs::create_dir(&directory).unwrap();
        let destination = directory.join("roundtrip.rp");
        let mut native = super::super::open(&path).expect("open caller-selected fixture");
        for (offset, graph) in &native.parts {
            assert_eq!(
                binary::encode(graph).unwrap(),
                native.source.part(*offset).unwrap(),
                "every original object package must roundtrip byte-for-byte"
            );
        }
        if legacy {
            assert_eq!(
                native.parts.len(),
                37,
                "fixture object package count changed"
            );
        }
        native.save(&native.project.clone(), &destination).unwrap();
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            original,
            "unchanged save must be byte exact"
        );

        let mut edited = native.project.clone();
        let (page_index, widget_index) = edited
            .pages
            .iter()
            .enumerate()
            .find_map(|(pi, page)| {
                page.widgets.iter().enumerate().find_map(|(wi, widget)| {
                    let binding = native.bindings.get(&widget.id)?;
                    let source =
                        get_node(&native.parts, &owners(&native.parts), &binding.object).ok()?;
                    let has_text = !widget.text.is_empty();
                    let has_legacy_child = contained_text_id(
                        native
                            .parts
                            .get(owners(&native.parts).get(&binding.object)?)?,
                        source,
                    )
                    .is_some();
                    (has_text && (!legacy || has_legacy_child)).then_some((pi, wi))
                })
            })
            .expect("fixture must contain an editable text widget");
        let widget = &mut edited.pages[page_index].widgets[widget_index];
        let id = widget.id;
        widget.text = "RPShop 保存验证\n第二行".into();
        widget.font_size = 22.0;
        widget.rotation = 23.5;
        native.save(&edited, &destination).unwrap();
        let reopened = super::super::open(&destination).unwrap();
        let actual = reopened
            .project
            .pages
            .iter()
            .flat_map(|p| &p.widgets)
            .find(|w| w.id == id)
            .unwrap();
        assert_eq!(actual.text, "RPShop 保存验证\n第二行");
        assert!((actual.font_size - 22.0).abs() < 0.001);
        assert!((actual.rotation - 23.5).abs() < 0.001);
        if legacy {
            let binding = &reopened.bindings[&id];
            let own = owners(&reopened.parts);
            let graph = &reopened.parts[&own[&binding.object]];
            let node = graph.node(raw_id(&binding.object)).unwrap();
            let text_id = contained_text_id(graph, node).unwrap();
            assert!(node.properties.get("RichTextBlock").is_none());
            assert_eq!(
                widget_text(&graph.node(&text_id).unwrap().properties),
                actual.text
            );
            if let Some(preview) = reopened
                .project
                .pages
                .iter()
                .flat_map(|p| &p.widgets)
                .find(|w| !reopened.is_editable_widget(w.id))
            {
                let mut invalid = reopened.project.clone();
                invalid
                    .pages
                    .iter_mut()
                    .flat_map(|p| &mut p.widgets)
                    .find(|w| w.id == preview.id)
                    .unwrap()
                    .text
                    .push('!');
                let mut document = super::super::open(&destination).unwrap();
                assert!(
                    document
                        .save(&invalid, &directory.join("rejected.rp"))
                        .is_err(),
                    "master preview edits must never write through to a shared master"
                );
            }
            if let Some((&child_id, &parent_id)) = native.preview_parents.iter().next() {
                let before = native.project.clone();
                let mut moved = before.clone();
                let parent = moved
                    .pages
                    .iter_mut()
                    .flat_map(|p| &mut p.widgets)
                    .find(|w| w.id == parent_id)
                    .unwrap();
                parent.x += 37.0;
                parent.y += 19.0;
                parent.rotation += 30.0;
                native.refresh_previews(&mut moved, &before);
                let preview = moved
                    .pages
                    .iter()
                    .flat_map(|p| &p.widgets)
                    .find(|w| w.id == child_id)
                    .unwrap()
                    .clone();
                native.refresh_previews(&mut moved, &before);
                assert_eq!(
                    moved
                        .pages
                        .iter()
                        .flat_map(|p| &p.widgets)
                        .find(|w| w.id == child_id)
                        .unwrap(),
                    &preview,
                    "drag refresh must always start from drag.before, without cumulative transforms"
                );
                let master_bytes = native
                    .parts
                    .iter()
                    .filter(|(_, graph)| {
                        graph.root().is_some_and(|node| node.kind == "Axure:Master")
                    })
                    .map(|(offset, graph)| (*offset, binary::encode(graph).unwrap()))
                    .collect::<BTreeMap<_, _>>();
                native.save(&moved, &destination).unwrap();
                let saved = native
                    .project
                    .pages
                    .iter()
                    .flat_map(|p| &p.widgets)
                    .find(|w| w.id == child_id)
                    .unwrap();
                close(saved.x, preview.x);
                close(saved.y, preview.y);
                close(saved.rotation, preview.rotation);
                for (offset, bytes) in master_bytes {
                    assert_eq!(
                        binary::encode(&native.parts[&offset]).unwrap(),
                        bytes,
                        "moving a master instance must never change its shared definition"
                    );
                }
            }
            let mut expected_texts = native
                .project
                .pages
                .iter()
                .flat_map(|p| &p.widgets)
                .map(|w| w.text.clone())
                .collect::<Vec<_>>();
            let mut added_project = native.project.clone();
            let added_id = added_project.allocate_id();
            let mut added = Widget::new(added_id, WidgetKind::Rectangle, 120.0, 160.0);
            added.text = "RPShop 独立新增验证".into();
            added.font_size = 18.0;
            added.rotation = 12.0;
            expected_texts.push(added.text.clone());
            added_project.pages[page_index].widgets.push(added);
            native.save(&added_project, &destination).unwrap();
            let mut actual_texts = native
                .project
                .pages
                .iter()
                .flat_map(|p| &p.widgets)
                .map(|w| w.text.clone())
                .collect::<Vec<_>>();
            expected_texts.sort();
            actual_texts.sort();
            assert_eq!(
                actual_texts, expected_texts,
                "new legacy text objects must not share a caption with their template"
            );
            let added = native
                .project
                .pages
                .iter()
                .flat_map(|p| &p.widgets)
                .find(|w| w.text == "RPShop 独立新增验证")
                .unwrap();
            assert!((added.font_size - 18.0).abs() < 0.001);
            assert!((added.rotation - 12.0).abs() < 0.001);
        }
        let pair = native
            .project
            .pages
            .iter()
            .enumerate()
            .find_map(|(page_index, page)| {
                let package = native.pages.get(&page.id)?;
                let graph = native
                    .parts
                    .values()
                    .find(|graph| graph.package == *package)?;
                let diagram = graph
                    .root()?
                    .properties
                    .get("diagram")
                    .and_then(reference_id)?;
                let root_ids = array(&graph.node(diagram)?.properties["diagram-objects"])
                    .iter()
                    .filter_map(reference_id)
                    .collect::<BTreeSet<_>>();
                page.widgets.windows(2).find_map(|pair| {
                    let eligible = pair.iter().all(|widget| {
                        widget.groups.is_empty()
                            && native.bindings.get(&widget.id).is_some_and(|binding| {
                                binding.object.starts_with(&format!("{package}/"))
                                    && root_ids.contains(raw_id(&binding.object))
                                    && graph.node(raw_id(&binding.object)).is_some_and(|node| {
                                        matches!(
                                            node.kind.rsplit(':').next(),
                                            Some(
                                                "VectorShape"
                                                    | "ImageBox"
                                                    | "TextBox"
                                                    | "TextArea"
                                                    | "Checkbox"
                                            )
                                        )
                                    })
                            })
                    });
                    eligible.then_some((page_index, pair[0].id, pair[1].id))
                })
            })
            .expect("fixture needs two adjacent ordinary root widgets for grouping");
        let mut grouped = native.project.clone();
        let group_id = grouped.allocate_id();
        for widget in &mut grouped.pages[pair.0].widgets {
            if widget.id == pair.1 {
                widget.name = "RPShop group member one".into();
                widget.groups = vec![group_id];
            }
            if widget.id == pair.2 {
                widget.name = "RPShop group member two".into();
                widget.groups = vec![group_id];
            }
        }
        native.save(&grouped, &destination).unwrap();
        let members = native
            .project
            .pages
            .iter()
            .flat_map(|page| &page.widgets)
            .filter(|widget| {
                matches!(
                    widget.name.as_str(),
                    "RPShop group member one" | "RPShop group member two"
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].groups.len(), 1);
        assert_eq!(members[0].groups, members[1].groups);
        assert!(native.group_bindings.contains_key(&members[0].groups[0]));
        let mut ungrouped = native.project.clone();
        for widget in ungrouped
            .pages
            .iter_mut()
            .flat_map(|page| &mut page.widgets)
        {
            if matches!(
                widget.name.as_str(),
                "RPShop group member one" | "RPShop group member two"
            ) {
                widget.groups.clear();
            }
        }
        native.save(&ungrouped, &destination).unwrap();
        assert!(
            native
                .project
                .pages
                .iter()
                .flat_map(|page| &page.widgets)
                .filter(|widget| matches!(
                    widget.name.as_str(),
                    "RPShop group member one" | "RPShop group member two"
                ))
                .all(|widget| widget.groups.is_empty())
        );
        let target = native
            .project
            .pages
            .iter()
            .flat_map(|page| &page.widgets)
            .find(|widget| {
                native.is_editable_widget(widget.id)
                    && widget.vector_points.is_some()
                    && widget.kind == WidgetKind::Image
                    && widget.image_data.is_some()
            })
            .or_else(|| {
                native
                    .project
                    .pages
                    .iter()
                    .flat_map(|page| &page.widgets)
                    .find(|widget| {
                        native.is_editable_widget(widget.id) && widget.vector_points.is_some()
                    })
            })
            .expect("fixture needs an editable native vector path")
            .id;
        let mut mirrored = native.project.clone();
        let widget = mirrored
            .pages
            .iter_mut()
            .flat_map(|page| &mut page.widgets)
            .find(|widget| widget.id == target)
            .unwrap();
        crate::vector_edit::flip_widget(widget, true).unwrap();
        crate::vector_edit::flip_widget(widget, false).unwrap();
        let expected_points = widget.vector_points.clone().unwrap();
        let expected_rotation = widget.rotation;
        native.save(&mirrored, &destination).unwrap();
        let widget = native
            .project
            .pages
            .iter()
            .flat_map(|page| &page.widgets)
            .find(|widget| widget.id == target)
            .unwrap();
        assert_eq!(widget.vector_points.as_ref(), Some(&expected_points));
        close(widget.rotation, expected_rotation);
        assert!(
            !native.is_bitmap_widget(target),
            "vector previews must keep their native vector identity"
        );
        let binding = &native.bindings[&target];
        let native_node = get_node(&native.parts, &owners(&native.parts), &binding.object).unwrap();
        assert!(native_node.kind.ends_with(":VectorShape"));
        assert_eq!(native_node.properties["ControlPoints"], expected_points);
        let state = native.session_state();
        let mut restored = super::super::open(&destination).unwrap();
        let mut legacy_state = state.clone();
        legacy_state["version"] = json!(1);
        assert!(restored.restore_session_state(&legacy_state).is_err());
        let mut incomplete_state = state.clone();
        incomplete_state
            .as_object_mut()
            .unwrap()
            .remove("preview_parents");
        assert!(restored.restore_session_state(&incomplete_state).is_err());
        restored.restore_session_state(&state).unwrap();
        assert_eq!(restored.project, native.project);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "source fixture must stay untouched"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn caller_selected_legacy_fixture_text_rotation_and_preservation() {
        check_native_fixture("RPSHOP_NATIVE_FIXTURE", true);
    }

    #[test]
    fn caller_selected_current_fixture_text_rotation_and_preservation() {
        check_native_fixture("RPSHOP_NATIVE_V11_FIXTURE", false);
    }
}
