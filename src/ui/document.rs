use super::*;

impl App {
    pub(super) fn remember(&mut self, before: Project) {
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

    pub(super) fn undo(&mut self) {
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

    pub(super) fn redo(&mut self) {
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

    pub(super) fn save_project(&mut self, save_as: bool) -> bool {
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

    pub(super) fn write_document(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
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

    pub(super) fn request_replace(&mut self, action: &str) {
        if self.project != self.saved {
            self.confirm = Some(action.into());
        } else {
            self.replace(action);
        }
    }

    pub(super) fn replace(&mut self, action: &str) {
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

    pub(super) fn export_file(&mut self, svg: bool) {
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
}
