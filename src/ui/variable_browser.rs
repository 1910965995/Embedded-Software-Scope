use std::collections::HashSet;
use crate::elf::{ElfContext, ElfVariable};

/// 变量浏览器状态（左侧「变量」标签页的内容）
pub struct VariableBrowser {
    /// 搜索框文字
    search_text: String,
    /// 已勾选的变量路径（与通道面板联动）
    pub checked_paths: HashSet<String>,
}

impl VariableBrowser {
    pub fn new() -> Self {
        Self {
            search_text: String::new(),
            checked_paths: HashSet::new(),
        }
    }

    /// 渲染变量浏览器 UI
    ///
    /// 返回 `Vec<(path, is_checked)>` 表示本帧用户勾选/取消的变更。
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        elf_ctx: Option<&ElfContext>,
    ) -> Vec<(String, bool)> {
        if elf_ctx.is_none() {
            ui.label("未加载 ELF 文件");
            ui.label("点击 📁 Open ELF 加载固件");
            return vec![];
        }

        let ctx = elf_ctx.unwrap();
        let mut changes = Vec::new();

        // 搜索框
        ui.add(
            egui::TextEdit::singleline(&mut self.search_text)
                .hint_text("🔍 搜索变量名或地址..."),
        );
        ui.add_space(4.0);

        // 统计信息
        let total = ctx.variables.len();
        let checked_count = self.checked_paths.len();
        ui.label(format!("变量: {} 个 | 已选: {} 个", total, checked_count));

        ui.separator();

        // 构建过滤后的显示列表
        let filtered: Vec<&ElfVariable> = if self.search_text.is_empty() {
            ctx.variables.iter().collect()
        } else {
            let lower = self.search_text.to_lowercase();
            ctx.variables
                .iter()
                .filter(|v| {
                    v.name.to_lowercase().contains(&lower)
                        || v.path.to_lowercase().contains(&lower)
                        || format!("0x{:x}", v.address).contains(&lower)
                })
                .collect()
        };

        // 按 source_file 分组显示，ScrollArea 限制最大高度
        egui::ScrollArea::vertical()
            .id_salt("variable_scroll")
            .auto_shrink([false, true])
            .max_height(400.0)
            .show(ui, |ui| {
                let mut current_group: Option<String> = None;

                for var in &filtered {
                    let group = var
                        .source_file
                        .clone()
                        .unwrap_or_else(|| "全局变量".to_string());

                    if current_group.as_deref() != Some(&group) {
                        current_group = Some(group.clone());
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(&group)
                                .small()
                                .color(egui::Color32::from_rgb(110, 118, 129)),
                        );
                    }

                    // 单行：Checkbox + 变量名 + 右对齐地址
                    let indent = if var.parent_path.is_some() { 20.0 } else { 8.0 };

                    ui.horizontal(|ui| {
                        ui.add_space(indent);
                        let mut is_checked = self.checked_paths.contains(&var.path);
                        if ui.checkbox(&mut is_checked, &var.name).changed() {
                            if is_checked {
                                self.checked_paths.insert(var.path.clone());
                            } else {
                                self.checked_paths.remove(&var.path);
                            }
                            changes.push((var.path.clone(), is_checked));
                        }

                        // 右对齐地址
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(format!("0x{:08X}", var.address))
                                        .small()
                                        .color(egui::Color32::from_rgb(140, 140, 140)),
                                );
                            },
                        );
                    });
                }
            });

        changes
    }

    /// 获取已勾选的路径集合
    pub fn checked(&self) -> &HashSet<String> {
        &self.checked_paths
    }

    /// 清空选择
    pub fn clear(&mut self) {
        self.checked_paths.clear();
    }
}
