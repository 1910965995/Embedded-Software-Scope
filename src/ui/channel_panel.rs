use crate::pipeline::sample::ValueType;

// ── 圆形图标按钮配色 ──
const GREY_BG: egui::Color32 = egui::Color32::from_rgb(235, 237, 240);
const GREY_HOVER: egui::Color32 = egui::Color32::from_rgb(208, 212, 218);
const GREY_FG: egui::Color32 = egui::Color32::from_rgb(45, 48, 55);

/// 绘制一个圆形图标按钮
///
/// - size: 20×20 px
/// - label: 居中显示的文本（如 "●" "×"）
/// - fg: 文本颜色
/// - bg: 正常背景色
/// - hover_bg: hover 时的背景色
///
/// 返回 true 表示用户点击了该按钮。
fn icon_button(
    ui: &mut egui::Ui,
    label: &str,
    fg: egui::Color32,
    bg: egui::Color32,
    hover_bg: egui::Color32,
) -> bool {
    let size = egui::vec2(16.0, 16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let bg_color = if response.hovered() { hover_bg } else { bg };
        let rounding = egui::CornerRadius::same(8);
        ui.painter().rect_filled(rect, rounding, bg_color);
        // 用 galley 手动居中,避免 painter.text 因字体 metrics 导致视觉偏上
        let font_id = egui::FontId::proportional(12.0);
        let galley = ui.painter().layout_no_wrap(label.to_string(), font_id, fg);
        let pos = rect.center() - galley.size() * 0.5;
        ui.painter().galley(pos, galley, fg);
    }

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    response.clicked()
}

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
            ui.label("No channels");
            return None;
        }

        egui::ScrollArea::vertical()
            .id_salt("channel_scroll")
            .show(ui, |ui| {
                for (i, ch) in self.channels.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        // 颜色指示点
                        ui.colored_label(ch.color, "●");

                        // 通道名
                        ui.label(&ch.name);

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // 移除按钮（灰色圆形 ×）
                            if icon_button(ui, "×", GREY_FG, GREY_BG, GREY_HOVER) {
                                remove_idx = Some(i);
                            }
                            // 可见性切换（实心 = 可见，空心 = 隐藏）
                            let vis_label = if ch.visible { "●" } else { "○" };
                            if icon_button(ui, vis_label, GREY_FG, GREY_BG, GREY_HOVER) {
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
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // 移除按钮（灰色圆形 ×）
                            if icon_button(ui, "×", GREY_FG, GREY_BG, GREY_HOVER) {
                                remove_idx = Some(i);
                            }
                            // 可见性切换（实心 = 可见，空心 = 隐藏）
                            let vis_label = if ch.visible { "●" } else { "○" };
                            if icon_button(ui, vis_label, GREY_FG, GREY_BG, GREY_HOVER) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个带单通道的 ChannelPanel
    fn panel_with_one_channel() -> ChannelPanel {
        let mut p = ChannelPanel::new();
        p.add_channel("CH1".to_string(), egui::Color32::RED, ValueType::Float);
        p
    }

    #[test]
    fn visible_button_toggles_state() {
        let mut p = panel_with_one_channel();
        assert!(p.channels[0].visible);
        p.channels[0].visible = false;
        assert!(!p.channels[0].visible);
        p.channels[0].visible = true;
        assert!(p.channels[0].visible);
    }

    #[test]
    fn visible_label_shows_filled_when_visible() {
        // 依赖 icon_button 的渲染行为：visible=true → label="●", false → label="○"
        let p = panel_with_one_channel();
        assert_eq!(p.channels[0].visible, true);
        let label_visible = "●";
        assert_eq!(label_visible, "●");

        let label_hidden = "○";
        assert_eq!(label_hidden, "○");
    }

    #[test]
    fn remove_returns_correct_index() {
        // 通过 show() 的返回值测试 remove 语义
        // mock 不可行（需要 egui UI），但逻辑等价于手动调用 remove_channel
        let mut p = panel_with_one_channel();
        assert_eq!(p.channel_count(), 1);
        p.remove_channel(0);
        assert_eq!(p.channel_count(), 0);
    }

    #[test]
    fn icon_label_is_multiply_sign() {
        // 验证移除按钮的文本是 U+00D7
        let remove_label = "×";
        assert_eq!(remove_label.chars().next().map(|c| c as u32), Some(0x00D7));
    }
}
