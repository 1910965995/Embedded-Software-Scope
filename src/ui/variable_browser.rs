use std::collections::HashSet;
use crate::elf::{ElfContext, ElfVariable};

/// 变量浏览器状态（左侧「变量」标签页的内容）
pub struct VariableBrowser {
    /// 搜索框文字
    search_text: String,
    /// 已勾选的变量路径（与通道面板联动）
    pub checked_paths: HashSet<String>,
    /// 缓存的上次搜索文字（用于检测变化）
    cached_search: String,
    /// 缓存的过滤结果（变量在 ctx.variables 中的索引）
    /// 仅在 search_text 变化时重建
    cached_filtered: Vec<usize>,
    /// 缓存对应的 ELF 文件路径（ELF 重新加载时重建）
    cached_elf_path: String,
}

impl VariableBrowser {
    pub fn new() -> Self {
        Self {
            search_text: String::new(),
            checked_paths: HashSet::new(),
            cached_search: String::new(),
            cached_filtered: Vec::new(),
            cached_elf_path: String::new(),
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
            ui.label("No ELF file loaded");
            return vec![];
        }

        let ctx = elf_ctx.unwrap();
        let mut changes = Vec::new();

        // 搜索框
        ui.add(
            egui::TextEdit::singleline(&mut self.search_text)
                .hint_text("🔍 Search variable name..."),
        );
        ui.add_space(4.0);

        // 统计信息
        let total = ctx.variables.len();
        let checked_count = self.checked_paths.len();
        ui.label(format!("Variables: {} | Selected: {}", total, checked_count));

        ui.separator();

        // 检测是否需要重建过滤缓存（搜索文字变化或 ELF 重新加载）
        let elf_path = ctx.file_path.clone();
        let need_rebuild = self.search_text != self.cached_search
            || elf_path != self.cached_elf_path;

        if need_rebuild {
            self.cached_search = self.search_text.clone();
            self.cached_elf_path = elf_path;
            self.cached_filtered.clear();

            if self.search_text.is_empty() {
                // 空搜索：包含所有变量
                self.cached_filtered.extend(0..ctx.variables.len());
            } else {
                let lower = self.search_text.to_lowercase();
                for (i, v) in ctx.variables.iter().enumerate() {
                    if v.name.to_lowercase().contains(&lower)
                        || v.path.to_lowercase().contains(&lower)
                    {
                        self.cached_filtered.push(i);
                    }
                }
            }
        }

        // 从缓存构建显示列表（仅借用，无重新过滤）
        let filtered: Vec<&ElfVariable> = self
            .cached_filtered
            .iter()
            .map(|&i| &ctx.variables[i])
            .collect();

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
                        .as_deref()
                        .unwrap_or("Globals");

                    if current_group.as_deref() != Some(group) {
                        current_group = Some(group.to_string());
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(group)
                                .small()
                                .color(egui::Color32::from_rgb(110, 118, 129)),
                        );
                    }

                    // 单行：Checkbox + 变量名
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
