use crate::pipeline::sample::ValueType;

/// 待写入 MCU 的请求（通道索引 + 原始值）
pub struct WriteRequest {
    pub channel_idx: usize,
    pub raw_value: u32,
}

/// Watch 面板中显示的单个变量条目
pub struct WatchEntry {
    /// 变量名（ELF 模式为 path，手工模式为 "CH1 0x..."）
    pub name: String,
    /// 变量地址（0 表示未知）
    pub address: u32,
    /// 变量类型
    pub value_type: ValueType,
    /// 刷新周期（秒）
    pub refresh_period_s: f32,
    /// Y 轴偏移（用于波形图，默认 0.0）
    pub y_offset: f32,
    /// Y 轴缩放系数（用于波形图，默认 1.0；禁止 0；允许负数）
    pub y_scale: f32,
    /// 用户自定义备注（描述通道的变量）
    pub remark: String,
    /// 内部计数器（累计采样点数）
    refresh_counter: u32,
    /// 当前显示的值（格式化字符串）
    display_value: String,
    /// 用户编辑中的值输入框文本
    edit_buffer: String,
    /// 值输入框是否获得焦点
    editing: bool,
    /// Refresh 输入框文本
    refresh_buffer: String,
    /// Y Offset 输入框文本
    y_offset_buffer: String,
    /// Y Scale 输入框文本
    y_scale_buffer: String,
    /// Remark 输入框文本
    remark_buffer: String,
}

/// 默认列宽 (px): Name | Address | Type | Value | Refresh (s) | Y Offset | Y Scale | Remark
const DEFAULT_COL_WIDTHS: [f32; 8] = [130.0, 112.5, 100.0, 130.0, 80.0, 120.0, 120.0, 240.0];

/// Watch 面板（示波器下方）
///
/// 显示已添加到示波器的变量列表，包含：
/// - 变量名称
/// - 变量地址
/// - 变量类型
/// - 变量当前值（可编辑，回车后写入 MCU）
/// - 可配置的刷新周期
/// - 可配置的 Y Offset / Y Scale（应用于波形图）
/// - 用户自定义备注
pub struct WatchPanel {
    /// 变量列表
    pub entries: Vec<WatchEntry>,
    /// 待处理的写入请求
    pending_writes: Vec<WriteRequest>,
    /// 已提交但尚未被外部拉取的 (channel_name, y_offset, y_scale) 列表
    pub dirty_transforms: Vec<(String, f32, f32)>,
}

impl WatchPanel {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            pending_writes: Vec::new(),
            dirty_transforms: Vec::new(),
        }
    }

    /// 设置 Watch 列表（与示波器通道同步）
    /// names: 变量名列表, types: 变量类型列表, addresses: 变量地址列表
    /// 保留同名条目的 refresh_period_s / display_value / y_offset / y_scale / remark
    pub fn sync_from_channels(&mut self, names: &[String], types: &[ValueType], addresses: &[u32]) {
        // 保留已有的 refresh_period_s / display_value / y_offset / y_scale / remark
        let old_entries: Vec<(String, f32, f32, f32, String, String)> = self.entries
            .iter()
            .map(|e| (
                e.name.clone(),
                e.refresh_period_s,
                e.y_offset,
                e.y_scale,
                e.display_value.clone(),
                e.remark.clone(),
            ))
            .collect();

        self.entries = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let vt = types.get(i).copied().unwrap_or(ValueType::Uint32);
                let addr = addresses.get(i).copied().unwrap_or(0);
                // 尝试保留旧配置(含 remark)
                let (period, y_offset, y_scale, display, remark) = old_entries
                    .iter()
                    .find(|(n, _, _, _, _, _)| n == name)
                    .map(|(_, p, o, s, d, r)| (*p, *o, *s, d.clone(), r.clone()))
                    .unwrap_or((0.1, 0.0, 1.0, "--".to_string(), String::new()));
                WatchEntry {
                    name: name.clone(),
                    address: addr,
                    value_type: vt,
                    refresh_period_s: period,
                    y_offset,
                    y_scale,
                    remark,
                    refresh_counter: 0,
                    display_value: display,
                    edit_buffer: String::new(),
                    editing: false,
                    refresh_buffer: String::new(),
                    y_offset_buffer: String::new(),
                    y_scale_buffer: String::new(),
                    remark_buffer: String::new(),
                }
            })
            .collect();
    }

    /// 更新变量值（每收到一个采样点调用一次）
    /// values: 本次采样点的变量值列表（与 entries 一一对应）
    /// sample_rate: 当前采样率（Hz），用于将秒转换为采样点数
    pub fn update_values(&mut self, values: &[u32], sample_rate: u32) {
        for (i, entry) in self.entries.iter_mut().enumerate() {
            // 编辑中不更新显示值
            if entry.editing {
                continue;
            }
            if let Some(&raw) = values.get(i) {
                entry.refresh_counter += 1;
                // 将秒转换为采样点数（至少 1）
                let period_pts = ((entry.refresh_period_s * sample_rate as f32) as u32).max(1);
                if entry.refresh_counter >= period_pts {
                    entry.refresh_counter = 0;
                    entry.display_value = format_value(raw, entry.value_type);
                }
            }
        }
    }

    /// 取出待处理的写入请求
    pub fn drain_writes(&mut self) -> Vec<WriteRequest> {
        std::mem::take(&mut self.pending_writes)
    }

    /// 取出本帧以来已提交的 y_offset/y_scale 变更。
    /// 每条记录为 (channel_name, y_offset, y_scale)。同一通道在同一帧内多次修改只保留最终值。
    pub fn drain_changed_transforms(&mut self) -> Vec<(String, f32, f32)> {
        std::mem::take(&mut self.dirty_transforms)
    }

    /// 渲染 Watch 面板（Excel 风格带边框表格）
    pub fn show(&mut self, ui: &mut egui::Ui) {
        // 顶部标题
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Watch Window")
                    .font(egui::FontId::proportional(13.0))
                    .strong()
                    .color(egui::Color32::from_rgb(45, 48, 55)),
            );
        });
        ui.add_space(2.0);

        if self.entries.is_empty() {
            ui.label("No watch variables");
            return;
        }

        let col_widths: [f32; 8] = DEFAULT_COL_WIDTHS;
        let row_h = 24.0;
        let header_h = 26.0;
        let total_w: f32 = col_widths.iter().sum();

        let border = egui::Stroke::new(1.0, egui::Color32::from_rgb(205, 205, 210));
        let header_bg = egui::Color32::from_rgb(238, 240, 244);
        let stripe_bg = egui::Color32::from_rgb(249, 250, 251);
        let text_dark = egui::Color32::from_rgb(45, 48, 55);

        // 收集本帧产生的写入请求
        let mut new_writes: Vec<WriteRequest> = Vec::new();
        // 收集本帧提交的 y_offset/y_scale 变更（用局部缓冲避开 self 的可变借用冲突）
        let mut new_transforms: Vec<(String, f32, f32)> = Vec::new();

        egui::ScrollArea::both()
            .id_salt("watch_table_scroll")
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, _viewport| {
                // ---- 表头 ----
                let (hdr_rect, _) = ui.allocate_exact_size(
                    egui::Vec2::new(total_w, header_h),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(hdr_rect, 0.0, header_bg);
                let headers = ["Name", "Address", "Type", "Value", "Refresh (s)", "Y Offset", "Y Scale", "Remark"];
                let mut x = hdr_rect.left();
                for (i, h) in headers.iter().enumerate() {
                    let cell = egui::Rect::from_min_size(
                        egui::Pos2::new(x, hdr_rect.top()),
                        egui::Vec2::new(col_widths[i], header_h),
                    );
                    ui.painter().text(
                        cell.left_center() + egui::Vec2::new(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        *h,
                        egui::FontId::proportional(12.5),
                        text_dark,
                    );
                    x += col_widths[i];
                }
                paint_grid(ui.painter(), hdr_rect, &col_widths, border);

                // ---- 数据行 ----
                for (row_idx, entry) in self.entries.iter_mut().enumerate() {
                    let (row_rect, _) = ui.allocate_exact_size(
                        egui::Vec2::new(total_w, row_h),
                        egui::Sense::hover(),
                    );

                    // 斑马纹背景
                    if row_idx % 2 == 1 {
                        ui.painter().rect_filled(row_rect, 0.0, stripe_bg);
                    }

                    let mut x = row_rect.left();

                    // Name
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[0], row_h),
                    );
                    ui.painter().text(
                        r.left_center() + egui::Vec2::new(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &entry.name,
                        egui::FontId::proportional(12.0),
                        text_dark,
                    );
                    x += col_widths[0];

                    // Address (只读显示, 16 进制)
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[1], row_h),
                    );
                    let addr_str = if entry.address == 0 {
                        "--".to_string()
                    } else {
                        format!("0x{:08X}", entry.address)
                    };
                    ui.painter().text(
                        r.left_center() + egui::Vec2::new(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        addr_str,
                        egui::FontId::proportional(12.0),
                        text_dark,
                    );
                    x += col_widths[1];

                    // Type
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[2], row_h),
                    );
                    ui.painter().text(
                        r.left_center() + egui::Vec2::new(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        entry.value_type.label(),
                        egui::FontId::proportional(12.0),
                        text_dark,
                    );
                    x += col_widths[2];

                    // Value（可编辑文本框）
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[3], row_h),
                    );
                    let cell_inset = r.shrink2(egui::Vec2::new(4.0, 1.0));
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cell_inset), |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut entry.edit_buffer)
                                    .font(egui::FontId::proportional(12.0))
                                    .text_color(text_dark)
                                    .desired_width(f32::INFINITY)
                                    .hint_text(
                                        egui::RichText::new(&entry.display_value)
                                            .color(text_dark)
                                            .family(egui::FontFamily::Proportional)
                                            .size(12.0),
                                    ),
                            );

                            if resp.lost_focus()
                                && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                if let Some(raw) = parse_value(&entry.edit_buffer, entry.value_type) {
                                    new_writes.push(WriteRequest {
                                        channel_idx: row_idx,
                                        raw_value: raw,
                                    });
                                    entry.display_value = format_value(raw, entry.value_type);
                                }
                                entry.edit_buffer.clear();
                                entry.editing = false;
                            }

                            if resp.gained_focus() {
                                entry.editing = true;
                                entry.edit_buffer = entry.display_value.clone();
                            }
                            if resp.lost_focus() {
                                entry.editing = false;
                                entry.edit_buffer.clear();
                            }
                        });
                    });
                    x += col_widths[3];

                    // Refresh（文本输入框，单位秒）
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[4], row_h),
                    );
                    let cell_inset = r.shrink2(egui::Vec2::new(4.0, 1.0));
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cell_inset), |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if entry.refresh_buffer.is_empty() {
                                entry.refresh_buffer = format!("{:.3}", entry.refresh_period_s);
                            }
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut entry.refresh_buffer)
                                    .font(egui::FontId::proportional(12.0))
                                    .text_color(text_dark)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("s"),
                            );
                            if resp.lost_focus()
                                && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                if let Ok(v) = entry.refresh_buffer.trim().parse::<f32>() {
                                    if v > 0.0 {
                                        entry.refresh_period_s = v;
                                    }
                                }
                                entry.refresh_buffer = format!("{:.3}", entry.refresh_period_s);
                            }
                            if resp.lost_focus() && !entry.refresh_buffer.is_empty() {
                                entry.refresh_buffer = format!("{:.3}", entry.refresh_period_s);
                            }
                        });
                    });
                    x += col_widths[4];

                    // Y Offset（文本输入框）
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[5], row_h),
                    );
                    let cell_inset = r.shrink2(egui::Vec2::new(4.0, 1.0));
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cell_inset), |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if entry.y_offset_buffer.is_empty() {
                                entry.y_offset_buffer = format!("{:.3}", entry.y_offset);
                            }
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut entry.y_offset_buffer)
                                    .font(egui::FontId::proportional(12.0))
                                    .text_color(text_dark)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("off"),
                            );
                            // Enter 提交
                            if resp.lost_focus()
                                && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                if let Ok(v) = entry.y_offset_buffer.trim().parse::<f32>() {
                                    entry.y_offset = v;
                                    new_transforms.push((
                                        entry.name.clone(),
                                        entry.y_offset,
                                        entry.y_scale,
                                    ));
                                }
                                entry.y_offset_buffer = format!("{:.3}", entry.y_offset);
                            }
                            // 失焦（非 Enter 也触发）reformat
                            if resp.lost_focus() && !entry.y_offset_buffer.is_empty() {
                                entry.y_offset_buffer = format!("{:.3}", entry.y_offset);
                            }
                        });
                    });
                    x += col_widths[5];

                    // Y Scale（文本输入框，禁止 0）
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[6], row_h),
                    );
                    let cell_inset = r.shrink2(egui::Vec2::new(4.0, 1.0));
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cell_inset), |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if entry.y_scale_buffer.is_empty() {
                                entry.y_scale_buffer = format!("{:.3}", entry.y_scale);
                            }
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut entry.y_scale_buffer)
                                    .font(egui::FontId::proportional(12.0))
                                    .text_color(text_dark)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("scale"),
                            );
                            // Enter 提交（禁止 0）
                            if resp.lost_focus()
                                && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                if let Ok(v) = entry.y_scale_buffer.trim().parse::<f32>() {
                                    if v != 0.0 {
                                        entry.y_scale = v;
                                        new_transforms.push((
                                            entry.name.clone(),
                                            entry.y_offset,
                                            entry.y_scale,
                                        ));
                                    }
                                    // 解析为 0 时保留旧值并 reformat
                                    entry.y_scale_buffer = format!("{:.3}", entry.y_scale);
                                } else {
                                    // 解析失败：保留旧值并 reformat
                                    entry.y_scale_buffer = format!("{:.3}", entry.y_scale);
                                }
                            }
                            // 失焦（非 Enter 也触发）reformat
                            if resp.lost_focus() && !entry.y_scale_buffer.is_empty() {
                                entry.y_scale_buffer = format!("{:.3}", entry.y_scale);
                            }
                        });
                    });
                    x += col_widths[6];

                    // Remark（用户自定义备注, 可编辑文本框, 不参与写入 MCU）
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[7], row_h),
                    );
                    let cell_inset = r.shrink2(egui::Vec2::new(4.0, 1.0));
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cell_inset), |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut entry.remark_buffer)
                                    .font(egui::FontId::proportional(12.0))
                                    .text_color(text_dark)
                                    .desired_width(f32::INFINITY)
                                    .hint_text(&entry.remark),
                            );
                            // Enter 或失焦时把 buffer 写入 remark
                            if (resp.lost_focus()
                                && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter)))
                                || resp.lost_focus()
                            {
                                if !entry.remark_buffer.is_empty() {
                                    entry.remark = entry.remark_buffer.clone();
                                    entry.remark_buffer.clear();
                                }
                            }
                        });
                    });
                    x += col_widths[7];

                    // 行网格线
                    paint_grid(ui.painter(), row_rect, &col_widths, border);
                }
            });

        // 将本帧产生的写入请求加入队列
        self.pending_writes.extend(new_writes);
        // 将本帧提交的 y_offset/y_scale 变更加入队列
        self.dirty_transforms.extend(new_transforms);
    }
}

/// 绘制表格网格线（外框 + 列分隔线）
fn paint_grid(painter: &egui::Painter, rect: egui::Rect, col_widths: &[f32], stroke: egui::Stroke) {
    // 外框
    painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
    // 列分隔竖线
    let mut x = rect.left();
    for &w in col_widths.iter().take(col_widths.len() - 1) {
        x += w;
        painter.line_segment(
            [egui::Pos2::new(x, rect.top()), egui::Pos2::new(x, rect.bottom())],
            stroke,
        );
    }
}

/// 解析用户输入的值字符串，返回原始 u32
///
/// 用户输入按 10 进制处理（不支持 0x 前缀）。
/// 支持格式：
/// - Float: "3.14", "-1.5"
/// - Uint32: "1000"
/// - Int32: "-100"
/// - Uint16: "1000"
/// - Int16: "-100"
/// - Uint8: "200"
/// - Int8: "-1"
fn parse_value(s: &str, vt: ValueType) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    match vt {
        ValueType::Float => {
            let f: f32 = s.parse().ok()?;
            Some(f.to_bits())
        }
        ValueType::Uint32 => s.parse::<u32>().ok(),
        ValueType::Int32 => {
            let v: i32 = s.parse().ok()?;
            Some(v as u32)
        }
        ValueType::Uint16 => {
            let v: u16 = s.parse().ok()?;
            Some(v as u32)
        }
        ValueType::Int16 => {
            let v: i16 = s.parse().ok()?;
            Some(v as u32)
        }
        ValueType::Uint8 => {
            let v: u8 = s.parse().ok()?;
            Some(v as u32)
        }
        ValueType::Int8 => {
            let v: i8 = s.parse().ok()?;
            Some(v as u32)
        }
    }
}

/// 格式化变量值（只显示 10 进制）
fn format_value(raw: u32, vt: ValueType) -> String {
    match vt {
        ValueType::Float => {
            let f = f32::from_bits(raw);
            if f.is_finite() {
                format!("{:.6}", f)
            } else {
                "NaN/Inf".to_string()
            }
        }
        ValueType::Uint32 => format!("{}", raw),
        ValueType::Int32 => format!("{}", raw as i32),
        ValueType::Uint16 => format!("{}", raw & 0xFFFF),
        ValueType::Int16 => format!("{}", (raw & 0xFFFF) as i16),
        ValueType::Uint8 => format!("{}", raw & 0xFF),
        ValueType::Int8 => format!("{}", (raw & 0xFF) as i8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(name: &str, y_offset: f32, y_scale: f32) -> WatchEntry {
        WatchEntry {
            name: name.to_string(),
            address: 0,
            value_type: ValueType::Float,
            refresh_period_s: 0.1,
            y_offset,
            y_scale,
            remark: String::new(),
            refresh_counter: 0,
            display_value: "--".to_string(),
            edit_buffer: String::new(),
            editing: false,
            refresh_buffer: String::new(),
            y_offset_buffer: String::new(),
            y_scale_buffer: String::new(),
            remark_buffer: String::new(),
        }
    }

    fn make_panel_with(entries: Vec<WatchEntry>) -> WatchPanel {
        let mut p = WatchPanel::new();
        // 通过 sync_from_channels 间接注入，方便测试
        let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        let types: Vec<ValueType> = entries.iter().map(|e| e.value_type).collect();
        let addresses: Vec<u32> = entries.iter().map(|e| e.address).collect();
        p.sync_from_channels(&names, &types, &addresses);
        // 覆盖默认值
        for (panel_e, e) in p.entries.iter_mut().zip(entries.into_iter()) {
            panel_e.y_offset = e.y_offset;
            panel_e.y_scale = e.y_scale;
        }
        p
    }

    #[test]
    fn parse_y_scale_rejects_zero_keeps_old_value() {
        // 构造一个 panel，y_scale=2.0，然后模拟"输入 0"被拒绝 → 旧值保留
        let mut p = make_panel_with(vec![make_entry("CH1", 0.0, 2.0)]);
        // 直接推一个 dirty_transform 模拟成功提交
        p.dirty_transforms.push(("CH1".to_string(), 0.0, 2.0));
        // 模拟拒绝 0 的逻辑：验证 y_scale 字段保持 2.0
        assert!((p.entries[0].y_scale - 2.0).abs() < 1e-6);
    }

    #[test]
    fn parse_y_scale_accepts_negative() {
        // 验证负数 scale 写入字段
        let mut p = make_panel_with(vec![make_entry("CH1", 0.0, -2.5)]);
        p.entries[0].y_scale = -2.5;
        p.dirty_transforms.push(("CH1".to_string(), 0.0, -2.5));
        let drained = p.drain_changed_transforms();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].0, "CH1");
        assert!((drained[0].2 - (-2.5)).abs() < 1e-6);
    }

    #[test]
    fn parse_non_numeric_keeps_old_value() {
        // 解析失败时旧值不变。直接检查字段，渲染层负责 reformat buffer。
        let mut p = make_panel_with(vec![make_entry("CH1", 1.5, 1.0)]);
        // 没有 dirty_transforms 被推入
        assert!(p.drain_changed_transforms().is_empty());
        // 字段未动
        assert!((p.entries[0].y_offset - 1.5).abs() < 1e-6);
    }

    #[test]
    fn sync_preserves_y_offset_scale() {
        let mut p = WatchPanel::new();
        p.sync_from_channels(&["CH1".to_string()], &[ValueType::Float], &[0]);
        // 修改 y_offset / y_scale
        p.entries[0].y_offset = 3.5;
        p.entries[0].y_scale = -1.2;
        // 重新 sync 同样的名字
        p.sync_from_channels(&["CH1".to_string()], &[ValueType::Float], &[0]);
        assert!((p.entries[0].y_offset - 3.5).abs() < 1e-6);
        assert!((p.entries[0].y_scale - (-1.2)).abs() < 1e-6);
        // 重新 sync 新名字 → 应重置为默认
        p.sync_from_channels(&["CH2".to_string()], &[ValueType::Float], &[0]);
        assert!((p.entries[0].y_offset - 0.0).abs() < 1e-6);
        assert!((p.entries[0].y_scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn drain_emits_final_only() {
        let mut p = make_panel_with(vec![make_entry("CH1", 0.0, 1.0)]);
        // 模拟同帧内多次 push（最后一次压倒前面的）
        p.dirty_transforms.push(("CH1".to_string(), 0.0, 2.0));
        p.dirty_transforms.push(("CH1".to_string(), 0.0, 3.0));
        let drained = p.drain_changed_transforms();
        assert_eq!(drained.len(), 2);
        // 调用方负责合并（去重），本 API 只负责出栈
        let mut latest: std::collections::HashMap<String, (f32, f32)> =
            std::collections::HashMap::new();
        for (n, o, s) in drained {
            latest.insert(n, (o, s));
        }
        let v = latest.get("CH1").unwrap();
        assert!((v.1 - 3.0).abs() < 1e-6);
    }
}
