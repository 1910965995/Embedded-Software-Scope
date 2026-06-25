use std::sync::Arc;
use crate::pipeline::sample::Sample;
use crate::pipeline::sample::ValueType;
use crate::pipeline::engine::{PipelineEngine, PipelineHandle};
use crate::usb::transfer::BulkTransfer;
use crate::dap::protocol::DapProtocol;
use crate::dap::swd::SwdLink;
use crate::elf::{ElfContext, ElfParser};
use super::display_buffer::DisplayBuffer;
use super::waveform::WaveformPanel;
use super::controls::{ControlPanel, AcquisitionState, AcquisitionCommand};
use super::cursor::CursorState;
use super::variable_browser::VariableBrowser;
use super::channel_panel::ChannelPanel;

/// 默认颜色调色板（8 种颜色，支持最多 8 通道）
const CHANNEL_COLORS: [egui::Color32; 8] = [
    egui::Color32::from_rgb(255, 68, 68),   // 红
    egui::Color32::from_rgb(68, 255, 68),   // 绿
    egui::Color32::from_rgb(68, 68, 255),   // 蓝
    egui::Color32::from_rgb(255, 200, 68),  // 橙
    egui::Color32::from_rgb(255, 68, 255),  // 品红
    egui::Color32::from_rgb(68, 255, 255),  // 青
    egui::Color32::from_rgb(180, 180, 180), // 灰
    egui::Color32::from_rgb(255, 150, 150), // 粉
];

/// 侧边栏标签页
#[derive(Clone, Copy, PartialEq)]
enum SidebarTab {
    Channels,
    Variables,
}

/// DAP Sampler 主应用
pub struct DapSamplerApp {
    pipeline: Option<PipelineHandle>,
    /// 活跃的 USB 连接（Idle 时为 None，Start 时建立，Stop 时释放）
    active_usb: Option<Arc<BulkTransfer>>,
    /// 手工模式地址（通过 --addresses 传入）
    manual_addresses: Vec<u32>,
    display_buf: DisplayBuffer,
    waveform: WaveformPanel,
    controls: ControlPanel,
    cursor: CursorState,
    temp_buf: Vec<Sample>,
    interval_us: f64,
    has_new_data: bool,

    // P4: ELF / 变量浏览器
    elf_ctx: Option<ElfContext>,
    variable_browser: VariableBrowser,
    channel_panel: ChannelPanel,
    active_tab: SidebarTab,
    #[allow(dead_code)]
    manual_channel_names: Vec<String>,
    #[allow(dead_code)]
    manual_value_types: Vec<ValueType>,
    rate_hz: u32,
    #[allow(dead_code)]
    target_count: Option<u64>,
}

impl DapSamplerApp {
    pub fn new(
        manual_addresses: Vec<u32>,
        addresses: Vec<String>,
        rate_hz: u32,
        target_count: Option<u64>,
        elf_ctx: Option<ElfContext>,
    ) -> Self {
        let interval_us = 1_000_000.0 / rate_hz as f64;

        let manual_channel_names: Vec<String> = addresses
            .iter()
            .enumerate()
            .map(|(i, a)| format!("CH{} {}", i + 1, a))
            .collect();
        let manual_value_types: Vec<ValueType> = vec![ValueType::Float; addresses.len()];

        let num_channels = manual_channel_names.len();
        let channel_colors: Vec<egui::Color32> = (0..num_channels)
            .map(|i| CHANNEL_COLORS[i % CHANNEL_COLORS.len()])
            .collect();

        let waveform = if !manual_channel_names.is_empty() {
            WaveformPanel::new(
                manual_channel_names.clone(),
                channel_colors,
                interval_us,
                manual_value_types.clone(),
            )
        } else {
            WaveformPanel::new(vec![], vec![], interval_us, vec![])
        };

        let mut channel_panel = ChannelPanel::new();
        if elf_ctx.is_none() {
            for (i, name) in manual_channel_names.iter().enumerate() {
                let color = CHANNEL_COLORS[i % CHANNEL_COLORS.len()];
                channel_panel.add_channel(name.clone(), color, ValueType::Float);
            }
        }

        Self {
            pipeline: None,
            active_usb: None,  // USB 连接推迟到 Start 时建立
            manual_addresses,
            display_buf: DisplayBuffer::new(200_000),
            waveform,
            controls: ControlPanel::new(rate_hz, target_count),
            cursor: CursorState::new(),
            temp_buf: (0..1024).map(|_| Sample { seq: 0, values: vec![] }).collect(),
            interval_us,
            has_new_data: false,
            elf_ctx,
            variable_browser: VariableBrowser::new(),
            channel_panel,
            active_tab: SidebarTab::Channels,
            manual_channel_names,
            manual_value_types,
            rate_hz,
            target_count,
        }
    }

    fn start_acquisition(&mut self) {
        // 1. 确定要采集的地址和通道信息
        let (addresses, channel_names, channel_colors, types) =
            if !self.manual_addresses.is_empty() {
                // 手工模式：使用预解析的地址
                let names: Vec<String> = self.manual_channel_names.clone();
                let colors: Vec<egui::Color32> = (0..names.len())
                    .map(|i| CHANNEL_COLORS[i % CHANNEL_COLORS.len()])
                    .collect();
                let types = vec![ValueType::Float; names.len()];
                (self.manual_addresses.clone(), names, colors, types)
            } else {
                // ELF 模式：从 channel_panel 动态获取
                let channels = &self.channel_panel.channels;
                if channels.is_empty() {
                    return;
                }

                let addresses: Vec<u32> = channels
                    .iter()
                    .filter_map(|ch| {
                        self.elf_ctx.as_ref().and_then(|ctx| {
                            ctx.variables
                                .iter()
                                .find(|v| v.path == ch.name || v.name == ch.name)
                                .map(|v| v.address)
                        })
                    })
                    .collect();

                if addresses.is_empty() {
                    return;
                }

                let names: Vec<String> = channels.iter().map(|c| c.name.clone()).collect();
                let colors: Vec<egui::Color32> = channels.iter().map(|c| c.color).collect();
                let types: Vec<ValueType> = channels.iter().map(|c| c.value_type).collect();
                (addresses, names, colors, types)
            };

        if addresses.is_empty() {
            return;
        }

        // 2. 连接 USB + 初始化 SWD（仅在尚未连接时）
        if self.active_usb.is_none() {
            let mut swd = match SwdLink::new() {
                Ok(s) => s,
                Err(e) => {
                    log::error!("连接 DAP-Link 失败: {}", e);
                    return;
                }
            };
            if let Err(e) = swd.init() {
                log::error!("SWD 初始化失败: {}", e);
                return;
            }
            let (usb, _) = swd.into_parts();
            self.active_usb = Some(Arc::new(usb));
        }

        // 3. 创建 PipelineEngine
        let engine = PipelineEngine::new(
            Arc::clone(self.active_usb.as_ref().unwrap()),
            DapProtocol::new(),
            addresses,
            self.rate_hz,
        );

        // 4. 更新波形面板（通道名和类型）
        self.waveform =
            WaveformPanel::new(channel_names, channel_colors, self.interval_us, types);

        // 5. 清空旧数据，从头开始采集
        self.display_buf = DisplayBuffer::new(200_000);
        self.cursor.clear();
        self.controls.update_count(0);

        // 6. 启动采集
        match engine.start() {
            Ok(handle) => {
                self.pipeline = Some(handle);
                self.controls.set_running();
            }
            Err(e) => log::error!("启动采集失败: {}", e),
        }
    }

    fn stop_pipeline(&mut self) {
        if let Some(handle) = self.pipeline.take() {
            handle.stop();
        }
    }

    fn stop_acquisition(&mut self) {
        self.stop_pipeline();
        self.controls.set_stopped();
        // 释放 USB 设备，让其他工具（如 Keil）可以使用 DAP-Link
        self.active_usb = None;
    }

    fn pause_acquisition(&mut self) {
        self.stop_pipeline();
        self.controls.set_paused();
        // Pause 时保持 USB 连接，恢复采集更快
    }

    fn drain_pipeline(&mut self) {
        let n = if let Some(ref handle) = self.pipeline {
            handle.drain_samples(&mut self.temp_buf)
        } else {
            0
        };
        if n > 0 {
            self.display_buf.push_batch(&self.temp_buf[..n]);
            self.controls.update_count(self.display_buf.next_seq());
            self.has_new_data = true;
        }
    }

    fn apply_variable_changes(&mut self, changes: Vec<(String, bool)>) {
        for (path, is_checked) in changes {
            if is_checked {
                if !self.channel_panel.has_channel(&path) {
                    let ch_count = self.channel_panel.channel_count();
                    if ch_count >= 8 {
                        continue;
                    }
                    let color = CHANNEL_COLORS[ch_count % CHANNEL_COLORS.len()];
                    let value_type = self.elf_ctx.as_ref()
                        .and_then(|ctx| ctx.variables.iter().find(|v| v.path == path))
                        .map(|v| v.value_type)
                        .unwrap_or(ValueType::Float);
                    self.channel_panel.add_channel(path, color, value_type);
                }
            } else {
                self.channel_panel.remove_by_name(&path);
            }
        }
    }

    fn sync_channel_visibility(&mut self) {
        let vis = self.channel_panel.visibility();
        for (i, visible) in vis.iter().enumerate() {
            self.waveform.set_channel_visible(i, *visible);
        }
    }

    fn show_cursor_info(&mut self, ui: &mut egui::Ui) {
        ui.heading("Cursor");
        ui.label("Click: Cursor 1 | Click again: Cursor 2");
        let interval_us = self.interval_us;
        let types = self.waveform.value_types().to_vec();
        if let Some(r) = self.cursor.get_result(
            self.display_buf.all(),
            self.display_buf.oldest_seq(),
            interval_us,
            &types,
        ) {
            ui.label(format!("T: {:.6}s", r.time_sec));
            for (i, v) in r.values.iter().enumerate() {
                let name = self.waveform.channel_names().get(i).copied().unwrap_or("?");
                ui.label(format!("  {}: {:.4}", name, v));
            }
            if self.cursor.cursor2.is_some() {
                if let Some((dt, dv)) = self.cursor.delta(
                    self.display_buf.all(),
                    self.display_buf.oldest_seq(),
                    interval_us,
                    &types,
                ) {
                    ui.separator();
                    ui.label(format!("dT: {:.6}s", dt));
                    for (i, v) in dv.iter().enumerate() {
                        ui.label(format!("  dCH{}: {:.4}", i + 1, v));
                    }
                }
            }
        } else {
            ui.label("(no cursor placed)");
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Clear Cursor").clicked() {
                self.cursor.clear();
            }
            if ui.button("Auto Fit Y").clicked() {
                self.waveform.request_auto_fit_y();
            }
        });
    }
}

impl eframe::App for DapSamplerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_pipeline();

        if let Some(target) = self.controls.target_count {
            if self.controls.total_samples >= target && self.pipeline.is_some() {
                self.stop_acquisition();
            }
        }

        // ---- 顶部控制栏 ----
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("DAP Sampler");
                ui.separator();

                // Open ELF 按钮（P4）
                if ui.button("\u{1f4c1} Open ELF").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("ELF", &["elf", "axf", "out", ""])
                        .pick_file()
                    {
                        match ElfParser::load(&path) {
                            Ok(ctx) => {
                                log::info!("ELF loaded: {} variables", ctx.variables.len());
                                self.elf_ctx = Some(ctx);
                                self.active_tab = SidebarTab::Variables;
                            }
                            Err(e) => log::error!("ELF load failed: {}", e),
                        }
                    }
                }

                ui.separator();

                if let Some(cmd) = self.controls.show(ui) {
                    match cmd {
                        AcquisitionCommand::Start => {
                            if self.controls.state == AcquisitionState::Idle
                                || self.controls.state == AcquisitionState::Paused
                            {
                                self.start_acquisition();
                            }
                        }
                        AcquisitionCommand::Pause => self.pause_acquisition(),
                        AcquisitionCommand::Stop => self.stop_acquisition(),
                    }
                }

                // 检测采样率变化（ComboBox 仅在 Idle 时可编辑）
                if self.controls.sample_rate != self.rate_hz {
                    self.rate_hz = self.controls.sample_rate;
                    self.interval_us = 1_000_000.0 / self.rate_hz as f64;
                    self.waveform.set_interval(self.interval_us);
                    log::info!("采样率已变更为 {} Hz", self.rate_hz);
                }
            });
        });

        // ---- 左侧竖排标签栏 ----
        let has_elf = self.elf_ctx.is_some();
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(280.0)
            .width_range(220.0..=400.0)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(36.0);
                        render_tab_button(
                            ui, "\u{901a}\u{9053}", &mut self.active_tab, SidebarTab::Channels,
                            self.channel_panel.channel_count(),
                        );
                        if has_elf {
                            render_tab_button(
                                ui, "\u{53d8}\u{91cf}", &mut self.active_tab, SidebarTab::Variables,
                                self.elf_ctx.as_ref().map_or(0, |e| e.variables.len()),
                            );
                        }
                    });

                    ui.separator();

                    ui.vertical(|ui| {
                        match self.active_tab {
                            SidebarTab::Channels => {
                                ui.heading("\u{901a}\u{9053}");
                                ui.separator();
                                if let Some(remove_idx) = self.channel_panel.show(ui) {
                                    let name = self.channel_panel.channels[remove_idx].name.clone();
                                    self.variable_browser.checked_paths.remove(&name);
                                    self.channel_panel.remove_channel(remove_idx);
                                }
                                ui.separator();
                                self.show_cursor_info(ui);
                            }
                            SidebarTab::Variables => {
                                ui.heading("\u{53d8}\u{91cf}\u{6d4f}\u{89c8}\u{5668}");
                                ui.separator();
                                let changes = self.variable_browser.show(
                                    ui,
                                    self.elf_ctx.as_ref(),
                                );
                                if !changes.is_empty() {
                                    self.apply_variable_changes(changes);
                                }
                            }
                        }
                    });
                });
            });

        // ---- 中央波形区域 ----
        self.sync_channel_visibility();
        let has_new_data = self.has_new_data;
        self.has_new_data = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            let available_width = ui.available_width();
            if let Some(seq) = self.waveform.show(
                ui,
                self.display_buf.all(),
                available_width,
                has_new_data,
            ) {
                self.cursor.click(seq);
            }
        });

        ctx.request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_acquisition();
    }
}

/// 确保 DapSamplerApp 被销毁时释放 USB 设备（即使 on_exit 未被调用）
impl Drop for DapSamplerApp {
    fn drop(&mut self) {
        if let Some(handle) = self.pipeline.take() {
            handle.stop();
        }
        // 显式释放 USB，确保 Keil 等工具可以立即使用 DAP-Link
        self.active_usb = None;
    }
}

fn render_tab_button(
    ui: &mut egui::Ui,
    label: &str,
    active_tab: &mut SidebarTab,
    this_tab: SidebarTab,
    badge_count: usize,
) {
    let is_active = *active_tab == this_tab;

    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::new(36.0, 44.0),
        egui::Sense::click(),
    );

    let bg_color = if is_active {
        egui::Color32::from_rgba_premultiplied(40, 40, 50, 200)
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 0.0, bg_color);

    if is_active {
        let indicator = egui::Rect::from_min_size(
            rect.left_top(),
            egui::Vec2::new(2.0, rect.height()),
        );
        ui.painter().rect_filled(indicator, 0.0, egui::Color32::from_rgb(233, 69, 96));
    }

    let text_color = if is_active {
        egui::Color32::from_rgb(233, 69, 96)
    } else {
        egui::Color32::from_rgb(110, 118, 129)
    };

    let char_spacing = 14.0;
    let chars: Vec<char> = label.chars().collect();
    let total_height = chars.len() as f32 * char_spacing;
    let start_y = rect.center().y - total_height / 2.0 + char_spacing / 2.0;

    for (i, ch) in chars.iter().enumerate() {
        let pos = egui::Pos2::new(rect.center().x, start_y + i as f32 * char_spacing);
        ui.painter().text(
            pos,
            egui::Align2::CENTER_CENTER,
            *ch,
            egui::FontId::proportional(12.0),
            text_color,
        );
    }

    if badge_count > 0 {
        let badge_pos = egui::Pos2::new(rect.right() - 4.0, rect.top() + 6.0);
        ui.painter().text(
            badge_pos,
            egui::Align2::RIGHT_CENTER,
            badge_count.to_string(),
            egui::FontId::proportional(9.0),
            egui::Color32::from_rgb(180, 180, 180),
        );
    }

    if response.clicked() {
        *active_tab = this_tab;
    }
}
