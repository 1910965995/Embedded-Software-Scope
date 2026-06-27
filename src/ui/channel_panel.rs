use crate::pipeline::sample::ValueType;

/// 通道面板中显示的通道信息
pub struct ChannelEntry {
    pub name: String,
    pub color: egui::Color32,
    pub visible: bool,
    pub value_type: ValueType,
}

/// 通道管理面板（左侧「通道」标签页的内容）
///
/// 显示已添加到示波器的变量通道列表，支持：
/// - 颜色标识
/// - 名称显示
/// - 可见性切换
/// - 移除按钮
/// - 光标测量数据
pub struct ChannelPanel {
    /// 通道列表
    pub channels: Vec<ChannelEntry>,
}

impl ChannelPanel {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    /// 渲染通道面板
    ///
    /// 返回 `Option<usize>` 表示用户点击了移除按钮的通道索引。
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<usize> {
        let mut remove_idx: Option<usize> = None;

        if self.channels.is_empty() {
            ui.label("暂无通道");
            return None;
        }

        egui::ScrollArea::vertical()
            .id_salt("channel_scroll")
            .show(ui, |ui| {
                for (i, ch) in self.channels.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        // 颜色指示点
                        ui.colored_label(ch.color, "●");

                        // 通道名 + 类型
                        ui.label(&ch.name);
                        ui.label(
                            egui::RichText::new(format!("[{}]", ch.value_type.label()))
                                .small()
                                .color(egui::Color32::from_rgb(100, 180, 255)),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // 移除按钮
                            if ui.small_button("✕").clicked() {
                                remove_idx = Some(i);
                            }
                            // 可见性切换
                            let eye = if ch.visible { "👁" } else { "⊘" };
                            if ui.small_button(eye).clicked() {
                                ch.visible = !ch.visible;
                            }
                        });
                    });
                }
            });

        remove_idx
    }

    /// 紧凑渲染（用于侧边栏变量浏览器下方）
    ///
    /// 只显示颜色、名称、可见性切换和移除按钮。
    pub fn show_compact(&mut self, ui: &mut egui::Ui) -> Option<usize> {
        let mut remove_idx: Option<usize> = None;

        egui::ScrollArea::vertical()
            .id_salt("channel_compact_scroll")
            .max_height(200.0)
            .show(ui, |ui| {
                for (i, ch) in self.channels.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.colored_label(ch.color, "●");
                        ui.label(&ch.name);
                        ui.label(
                            egui::RichText::new(format!("[{}]", ch.value_type.label()))
                                .small()
                                .color(egui::Color32::from_rgb(100, 180, 255)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✕").clicked() {
                                remove_idx = Some(i);
                            }
                            let eye = if ch.visible { "👁" } else { "⊘" };
                            if ui.small_button(eye).clicked() {
                                ch.visible = !ch.visible;
                            }
                        });
                    });
                }
            });

        remove_idx
    }

    /// 通道数量
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// 添加通道
    pub fn add_channel(&mut self, name: String, color: egui::Color32, value_type: ValueType) {
        if self.channels.len() < 8 {
            self.channels.push(ChannelEntry {
                name,
                color,
                visible: true,
                value_type,
            });
        }
    }

    /// 移除通道
    pub fn remove_channel(&mut self, index: usize) {
        if index < self.channels.len() {
            self.channels.remove(index);
        }
    }

    /// 按路径移除通道
    pub fn remove_by_name(&mut self, name: &str) {
        self.channels.retain(|c| c.name != name);
    }

    /// 检查通道是否存在
    pub fn has_channel(&self, name: &str) -> bool {
        self.channels.iter().any(|c| c.name == name)
    }

    /// 获取通道可见性列表
    pub fn visibility(&self) -> Vec<bool> {
        self.channels.iter().map(|c| c.visible).collect()
    }
}
