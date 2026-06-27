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
    /// 变量类型
    pub value_type: ValueType,
    /// 刷新周期（秒）
    pub refresh_period_s: f32,
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
}

/// Watch 面板（示波器下方）
///
/// 显示已添加到示波器的变量列表，包含：
/// - 变量名称
/// - 变量类型
/// - 变量当前值（可编辑，回车后写入 MCU）
/// - 可配置的刷新周期
pub struct WatchPanel {
    /// 变量列表
    entries: Vec<WatchEntry>,
    /// 待处理的写入请求
    pending_writes: Vec<WriteRequest>,
}

impl WatchPanel {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            pending_writes: Vec::new(),
        }
    }

    /// 设置 Watch 列表（与示波器通道同步）
    /// names: 变量名列表, types: 变量类型列表
    pub fn sync_from_channels(&mut self, names: &[String], types: &[ValueType]) {
        // 保留已有的 refresh_period_s 和 display_value
        let old_entries: Vec<(String, f32, String)> = self.entries
            .iter()
            .map(|e| (e.name.clone(), e.refresh_period_s, e.display_value.clone()))
            .collect();

        self.entries = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let vt = types.get(i).copied().unwrap_or(ValueType::Uint32);
                // 尝试保留旧配置
                let (period, display) = old_entries
                    .iter()
                    .find(|(n, _, _)| n == name)
                    .map(|(_, p, d)| (*p, d.clone()))
                    .unwrap_or((0.1, "--".to_string()));
                WatchEntry {
                    name: name.clone(),
                    value_type: vt,
                    refresh_period_s: period,
                    refresh_counter: 0,
                    display_value: display,
                    edit_buffer: String::new(),
                    editing: false,
                    refresh_buffer: String::new(),
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

    /// 渲染 Watch 面板（Excel 风格带边框表格）
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.entries.is_empty() {
            ui.label("暂无 Watch 变量");
            return;
        }

        let col_widths: [f32; 4] = [130.0, 70.0, 170.0, 90.0];
        let row_h = 24.0;
        let header_h = 26.0;
        let total_w: f32 = col_widths.iter().sum();

        let border = egui::Stroke::new(1.0, egui::Color32::from_rgb(205, 205, 210));
        let header_bg = egui::Color32::from_rgb(238, 240, 244);
        let stripe_bg = egui::Color32::from_rgb(249, 250, 251);
        let text_dark = egui::Color32::from_rgb(45, 48, 55);
        let text_blue = egui::Color32::from_rgb(70, 130, 200);
        let text_amber = egui::Color32::from_rgb(170, 110, 30);

        // 收集本帧产生的写入请求
        let mut new_writes: Vec<WriteRequest> = Vec::new();

        egui::ScrollArea::vertical()
            .id_salt("watch_table_scroll")
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, _viewport| {
                // ---- 表头 ----
                let (hdr_rect, _) = ui.allocate_exact_size(
                    egui::Vec2::new(total_w, header_h),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(hdr_rect, 0.0, header_bg);
                let headers = ["Name", "Type", "Value", "Refresh"];
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

                    // Type
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[1], row_h),
                    );
                    ui.painter().text(
                        r.left_center() + egui::Vec2::new(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        entry.value_type.label(),
                        egui::FontId::proportional(12.0),
                        text_dark,
                    );
                    x += col_widths[1];

                    // Value（可编辑文本框）
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[2], row_h),
                    );
                    let cell_inset = r.shrink2(egui::Vec2::new(4.0, 1.0));
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cell_inset), |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut entry.edit_buffer)
                                    .font(egui::FontId::proportional(12.0))
                                    .text_color(text_dark)
                                    .desired_width(f32::INFINITY)
                                    .hint_text(&entry.display_value),
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
                    x += col_widths[2];

                    // Refresh（文本输入框，单位秒）
                    let r = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(col_widths[3], row_h),
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
                    x += col_widths[3];

                    // 行网格线
                    paint_grid(ui.painter(), row_rect, &col_widths, border);
                }
            });

        // 将本帧产生的写入请求加入队列
        self.pending_writes.extend(new_writes);
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
/// 支持格式：
/// - Float: "3.14", "-1.5"
/// - Uint32: "0x12345678", "1000"
/// - Int32: "-100", "0xFF"
/// - Uint16: "0x1234", "1000"
/// - Int16: "-100"
/// - Uint8: "0xFF", "200"
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
        ValueType::Uint32 => parse_int(s, 0xFFFFFFFF, 0),
        ValueType::Int32 => {
            let v: i32 = if s.starts_with("0x") || s.starts_with("0X") {
                i32::from_str_radix(&s[2..], 16).ok()?
            } else {
                s.parse().ok()?
            };
            Some(v as u32)
        }
        ValueType::Uint16 => {
            let v = if s.starts_with("0x") || s.starts_with("0X") {
                u16::from_str_radix(&s[2..], 16).ok()?
            } else {
                s.parse().ok()?
            };
            Some(v as u32)
        }
        ValueType::Int16 => {
            let v: i16 = if s.starts_with("0x") || s.starts_with("0X") {
                i16::from_str_radix(&s[2..], 16).ok()?
            } else {
                s.parse().ok()?
            };
            Some(v as u32)
        }
        ValueType::Uint8 => {
            let v = if s.starts_with("0x") || s.starts_with("0X") {
                u8::from_str_radix(&s[2..], 16).ok()?
            } else {
                s.parse().ok()?
            };
            Some(v as u32)
        }
        ValueType::Int8 => {
            let v: i8 = if s.starts_with("0x") || s.starts_with("0X") {
                i8::from_str_radix(&s[2..], 16).ok()?
            } else {
                s.parse().ok()?
            };
            Some(v as u32)
        }
    }
}

/// 解析无符号整数
fn parse_int(s: &str, _max: u32, _min: u32) -> Option<u32> {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        u32::from_str_radix(&s[2..], 16).ok()
    } else {
        s.parse().ok()
    }
}

/// 格式化变量值
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
        ValueType::Uint32 => format!("0x{:08X} ({})", raw, raw),
        ValueType::Int32 => {
            let v = raw as i32;
            format!("{} (0x{:08X})", v, raw)
        }
        ValueType::Uint16 => format!("0x{:04X} ({})", raw & 0xFFFF, raw & 0xFFFF),
        ValueType::Int16 => {
            let v = (raw & 0xFFFF) as i16;
            format!("{} (0x{:04X})", v, raw & 0xFFFF)
        }
        ValueType::Uint8 => format!("0x{:02X} ({})", raw & 0xFF, raw & 0xFF),
        ValueType::Int8 => {
            let v = (raw & 0xFF) as i8;
            format!("{} (0x{:02X})", v, raw & 0xFF)
        }
    }
}
