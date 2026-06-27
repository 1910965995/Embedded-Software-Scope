use crate::pipeline::sample::ValueType;

/// Watch 面板中显示的单个变量条目
pub struct WatchEntry {
    /// 变量名（ELF 模式为 path，手工模式为 "CH1 0x..."）
    pub name: String,
    /// 变量类型
    pub value_type: ValueType,
    /// 刷新周期（每 N 个采样点刷新一次显示）
    pub refresh_period: u32,
    /// 内部计数器，用于刷新周期控制
    refresh_counter: u32,
    /// 当前显示的值（格式化字符串）
    display_value: String,
}

/// Watch 面板（示波器下方）
///
/// 显示已添加到示波器的变量列表，包含：
/// - 变量名称
/// - 变量类型
/// - 变量当前值（按刷新周期更新）
/// - 可配置的刷新周期
pub struct WatchPanel {
    /// 变量列表
    entries: Vec<WatchEntry>,
}

impl WatchPanel {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 设置 Watch 列表（与示波器通道同步）
    /// names: 变量名列表, types: 变量类型列表
    pub fn sync_from_channels(&mut self, names: &[String], types: &[ValueType]) {
        // 保留已有的 refresh_period 和 display_value
        let old_entries: Vec<(String, u32, String)> = self.entries
            .iter()
            .map(|e| (e.name.clone(), e.refresh_period, e.display_value.clone()))
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
                    .unwrap_or((1, "--".to_string()));
                WatchEntry {
                    name: name.clone(),
                    value_type: vt,
                    refresh_period: period,
                    refresh_counter: 0,
                    display_value: display,
                }
            })
            .collect();
    }

    /// 更新变量值（每收到一个采样点调用一次）
    /// values: 本次采样点的变量值列表（与 entries 一一对应）
    pub fn update_values(&mut self, values: &[u32]) {
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if let Some(&raw) = values.get(i) {
                entry.refresh_counter += 1;
                if entry.refresh_counter >= entry.refresh_period {
                    entry.refresh_counter = 0;
                    entry.display_value = format_value(raw, entry.value_type);
                }
            }
        }
    }

    /// 渲染 Watch 面板（表格形式）
    ///
    /// 使用 egui::Grid 而非 egui_extras::TableBuilder，避免 TableBuilder 内部
    /// ScrollArea 的 min/max scrolled height 与 TopBottomPanel resize 形成循环
    /// 依赖，导致面板高度无法手动调整。
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.entries.is_empty() {
            ui.label("暂无 Watch 变量");
            ui.label("在左侧变量浏览器中勾选变量以添加");
            return;
        }

        // 表头（固定不滚动）
        egui::Grid::new("watch_header")
            .num_columns(4)
            .striped(false)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.strong("Name");
                ui.strong("Type");
                ui.strong("Value");
                ui.strong("Refresh");
                ui.end_row();
            });

        ui.separator();

        // 数据行（可滚动）
        egui::ScrollArea::vertical()
            .id_salt("watch_body")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("watch_body")
                    .num_columns(4)
                    .striped(true)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        for entry in self.entries.iter_mut() {
                            // 变量名
                            ui.label(&entry.name);

                            // 类型
                            ui.label(
                                egui::RichText::new(entry.value_type.label())
                                    .small()
                                    .color(egui::Color32::from_rgb(100, 180, 255)),
                            );

                            // 当前值
                            ui.label(
                                egui::RichText::new(&entry.display_value)
                                    .color(egui::Color32::from_rgb(255, 220, 100))
                                    .monospace(),
                            );

                            // 刷新周期
                            ui.add(
                                egui::DragValue::new(&mut entry.refresh_period)
                                    .range(1..=10000)
                                    .speed(0.1)
                                    .suffix(" pt"),
                            );

                            ui.end_row();
                        }
                    });
            });
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
