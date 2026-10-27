use super::*;

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        if !self.import_warnings.is_empty() && !self.screenshot_mode {
            let mut open = true;
            egui::Window::new(RichText::new("原生文件打开结果").size(20.).strong())
                .frame(dialog_style::frame(ctx))
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
                    dialog_style::actions(ui, |ui| {
                        if ui.add(dialog_style::primary("知道了", PURPLE)).clicked() {
                            self.import_warnings.clear();
                        }
                    });
                });
            if !open {
                self.import_warnings.clear();
            }
        }
        if let Some(id) = self.editing {
            let mut open = true;
            egui::Window::new(RichText::new("编辑文本").size(20.).strong())
                .frame(dialog_style::frame(ctx))
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
                    dialog_style::actions(ui, |ui| {
                        if ui.add(dialog_style::primary("完成", PURPLE)).clicked() {
                            self.editing = None;
                        }
                    });
                });
            if !open {
                self.editing = None;
            }
        }
        if let Some(action) = self.confirm.clone() {
            dialog_style::modal(ctx, "unsaved-project").show(ctx, |ui| {
                ui.set_width(440.0);
                dialog_style::title(ui, "保存当前修改？");
                ui.label("工程包含尚未保存的修改。保存后再继续操作。");
                ui.add_space(10.0);
                dialog_style::actions(ui, |ui| {
                    if ui
                        .add(dialog_style::primary("保存并继续", PURPLE))
                        .clicked()
                        && self.save_project(false)
                    {
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
                        self.cancel_replace();
                    }
                });
            });
        }
        if self.help {
            egui::Window::new(RichText::new("关于 RPShop").size(20.).strong())
                .frame(dialog_style::frame(ctx))
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
