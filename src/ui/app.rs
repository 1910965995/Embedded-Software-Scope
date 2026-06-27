use std::sync::Arc;
use std::time::Instant;
use crate::pipeline::sample::Sample;
use crate::pipeline::sample::ValueType;
use crate::pipeline::engine::{PipelineEngine, PipelineHandle};
use crate::usb::transfer::BulkTransfer;
use crate::dap::protocol::DapProtocol;
use crate::dap::swd::SwdLink;
use crate::elf::{ElfContext, ElfParser};
use super::display_buffer::DisplayBuffer;
use super::waveform::{WaveformPanel, WaveformDisplayMode};
use super::controls::{ControlPanel, AcquisitionState, AcquisitionCommand};
use super::cursor::CursorState;
use super::variable_browser::VariableBrowser;
use super::channel_panel::ChannelPanel;
use super::watch_panel::WatchPanel;

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

/// DAP Sampler 主应用
pub struct DapSamplerApp {
    pipeline: Option<PipelineHandle>,
    /// 活跃的 USB 连接（Idle 时为 None，Start 时建立，Stop 时释放）
    active_usb: Option<Arc<BulkTransfer>>,
    /// 手工模式地址（通过 --addresses 传入）
    manual_addresses: Vec<u32>,
    /// 采集开始时间（用于计算实际采样率）
    acquisition_start: Option<Instant>,
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
    /// Watch 面板（示波器下方）
    watch_panel: WatchPanel,
    /// 左侧面板各模块的展开/收起状态
    var_browser_open: bool,
    channels_open: bool,
    cursor_open: bool,
    #[allow(dead_code)]
    manual_channel_names: Vec<String>,
    manual_value_types: Vec<ValueType>,
    rate_hz: u32,
    #[allow(dead_code)]
    target_count: Option<u64>,
    /// 波形显示模式
    display_mode: WaveformDisplayMode,
    /// 显示窗口大小（采样点数）
    window_size: usize,
}

impl DapSamplerApp {
    pub fn new(
        manual_addresses: Vec<u32>,
        addresses: Vec<String>,
        rate_hz: u32,
        target_count: Option<u64>,
        elf_ctx: Option<ElfContext>,
        manual_types: Vec<ValueType>,
    ) -> Self {
        let interval_us = 1_000_000.0 / rate_hz as f64;

        let manual_channel_names: Vec<String> = addresses
            .iter()
            .enumerate()
            .map(|(i, a)| format!("CH{} {}", i + 1, a))
            .collect();
        let manual_value_types: Vec<ValueType> = if manual_types.is_empty() {
            vec![ValueType::Uint32; addresses.len()]
        } else {
            manual_types
        };

        let display_mode = WaveformDisplayMode::Line;

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
                display_mode,
            )
        } else {
            WaveformPanel::new(vec![], vec![], interval_us, vec![], display_mode)
        };

        let mut channel_panel = ChannelPanel::new();
        if elf_ctx.is_none() {
            for (i, name) in manual_channel_names.iter().enumerate() {
                let color = CHANNEL_COLORS[i % CHANNEL_COLORS.len()];
                let vt = manual_value_types.get(i).copied().unwrap_or(ValueType::Uint32);
                channel_panel.add_channel(name.clone(), color, vt);
            }
        }

        // 初始化 Watch 面板
        let mut watch_panel = WatchPanel::new();
        if !manual_channel_names.is_empty() {
            watch_panel.sync_from_channels(&manual_channel_names, &manual_value_types);
        }

        Self {
            pipeline: None,
            active_usb: None,  // USB 连接推迟到 Start 时建立
            manual_addresses,
            acquisition_start: None,
            display_buf: DisplayBuffer::new(200_000),
            waveform,
            controls: ControlPanel::new(rate_hz, target_count),
            cursor: CursorState::new(),
            temp_buf: (0..1024).map(|_| Sample { seq: 0, timestamp_sec: 0.0, values: vec![] }).collect(),
            interval_us,
            has_new_data: false,
            elf_ctx,
            variable_browser: VariableBrowser::new(),
            channel_panel,
            watch_panel,
            var_browser_open: true,
            channels_open: true,
            cursor_open: true,
            manual_channel_names,
            manual_value_types,
            rate_hz,
            target_count,
            display_mode,
            window_size: 2000,
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
                let types = self.manual_value_types.clone();
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
            WaveformPanel::new(channel_names.clone(), channel_colors, self.interval_us, types.clone(), self.display_mode);

        // 5. 清空旧数据，用当前窗口大小创建新缓冲区
        self.display_buf = DisplayBuffer::new(self.window_size);
        self.cursor.clear();
        self.controls.update_count(0);

        // 同步 Watch 面板
        self.watch_panel.sync_from_channels(&channel_names, &types);

        // 6. 启动采集
        match engine.start() {
            Ok(handle) => {
                self.pipeline = Some(handle);
                self.controls.set_running();
                self.acquisition_start = Some(Instant::now());
            }
            Err(e) => log::error!("启动采集失败: {}", e),
        }
    }

    fn stop_pipeline(&mut self) {
        if let Some(handle) = self.pipeline.take() {
            log::info!("开始停止流水线，等待子线程退出...");
            handle.stop();
            log::info!("流水线停止完成");
        }
        self.acquisition_start = None;
        self.controls.actual_rate_hz = 0.0;
    }

    fn stop_acquisition(&mut self) {
        self.stop_pipeline();
        self.controls.set_stopped();
        // 释放 USB 设备：发送 DAP_Disconnect + release_interface
        // 不发送 DAP_Disconnect 会导致 DAP-Link 固件认为调试器仍连接着
        if let Some(usb) = self.active_usb.take() {
            match Arc::try_unwrap(usb) {
                Ok(mut bulk) => {
                    bulk.release();
                }
                Err(arc) => {
                    if let Some(bulk) = Arc::get_mut(&mut arc.clone()) {
                        bulk.release();
                    }
                    drop(arc);
                }
            }
        }
    }

    /// 强制释放 DAP-Link USB 设备
    ///
    /// 先停止流水线（如果在运行），然后显式调用 release_interface + reset。
    /// 如果没有活跃的 USB 连接，也尝试直接扫描 USB 总线释放被占用的设备。
    #[allow(dead_code)]
    fn release_daplink(&mut self) {
        log::info!("用户请求释放 DAP-Link");
        // 先停止流水线
        self.stop_pipeline();
        self.controls.set_stopped();

        if let Some(usb) = self.active_usb.take() {
            // 有活跃的 USB 连接，显式释放
            match Arc::try_unwrap(usb) {
                Ok(mut bulk) => {
                    bulk.release();
                    log::info!("DAP-Link 已显式释放");
                }
                Err(arc) => {
                    if let Some(bulk) = Arc::get_mut(&mut arc.clone()) {
                        bulk.release();
                        log::info!("DAP-Link 已通过 get_mut 释放");
                    } else {
                        log::warn!("USB Arc 仍有活动引用，无法显式释放，依赖 Drop 自动释放");
                    }
                    drop(arc);
                }
            }
        } else {
            // 没有活跃的 USB 连接，尝试直接扫描并释放
            log::info!("没有活跃的 USB 连接，尝试直接扫描释放 DAP-Link");
            force_release_all_daplink();
        }
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
            // 更新 Watch 面板的变量值
            for sample in &self.temp_buf[..n] {
                self.watch_panel.update_values(&sample.values);
            }
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

        // 同步 Watch 面板
        let names: Vec<String> = self.channel_panel.channels.iter().map(|c| c.name.clone()).collect();
        let types: Vec<ValueType> = self.channel_panel.channels.iter().map(|c| c.value_type).collect();
        self.watch_panel.sync_from_channels(&names, &types);
    }

    fn sync_channel_visibility(&mut self) {
        let vis = self.channel_panel.visibility();
        for (i, visible) in vis.iter().enumerate() {
            self.waveform.set_channel_visible(i, *visible);
        }
    }

    fn show_cursor_info(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("点击波形放置光标 1\n再次点击放置光标 2")
                .small()
                .color(egui::Color32::from_rgb(0x8b, 0x94, 0x9e)),
        );
        let types = self.waveform.value_types().to_vec();
        if let Some(r) = self.cursor.get_result(
            self.display_buf.all(),
            self.display_buf.oldest_seq(),
            &types,
        ) {
            ui.separator();
            ui.label(
                egui::RichText::new(format!("T: {:.6} s", r.time_sec))
                    .monospace()
                    .color(egui::Color32::from_rgb(0xff, 0xe0, 0x6e)),
            );
            for (i, v) in r.values.iter().enumerate() {
                let name = self.waveform.channel_names().get(i).copied().unwrap_or("?");
                ui.label(
                    egui::RichText::new(format!("  {}: {:.4}", name, v))
                        .monospace(),
                );
            }
            if self.cursor.cursor2.is_some() {
                if let Some((dt, dv)) = self.cursor.delta(
                    self.display_buf.all(),
                    self.display_buf.oldest_seq(),
                    &types,
                ) {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("\u{0394}T: {:.6} s", dt))
                            .monospace()
                            .color(egui::Color32::from_rgb(0x58, 0xa6, 0xff)),
                    );
                    for (i, v) in dv.iter().enumerate() {
                        ui.label(
                            egui::RichText::new(format!("  dCH{}: {:.4}", i + 1, v))
                                .monospace(),
                        );
                    }
                }
            }
        } else {
            // 未放置光标时不显示任何内容
        }
    }
}

impl eframe::App for DapSamplerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_pipeline();

        // 计算实际采样率
        if let Some(start) = self.acquisition_start {
            let elapsed = start.elapsed().as_secs_f64();
            if elapsed > 0.1 {
                self.controls.actual_rate_hz = self.controls.total_samples as f64 / elapsed;
            }
        }

        if let Some(target) = self.controls.target_count {
            if self.controls.total_samples >= target && self.pipeline.is_some() {
                self.stop_acquisition();
            }
        }

        // ---- 顶部标题栏（精简：标题 + 状态 + ELF + 释放）----
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("DAP Sampler");
                ui.separator();

                // 状态指示（带颜色圆点）
                let (state_text, state_color) = match self.controls.state {
                    AcquisitionState::Idle => ("Idle", egui::Color32::from_rgb(0x8b, 0x94, 0x9e)),
                    AcquisitionState::Running => ("Running", egui::Color32::from_rgb(0x39, 0xd3, 0x53)),
                    AcquisitionState::Paused => ("Paused", egui::Color32::from_rgb(0xe8, 0xa4, 0x00)),
                };
                ui.colored_label(state_color, format!("● {}", state_text));

                ui.separator();

                // Open ELF 按钮
                if ui.button("\u{1f4c1} Open ELF").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("ELF", &["elf", "axf", "out", ""])
                        .pick_file()
                    {
                        match ElfParser::load(&path) {
                            Ok(ctx) => {
                                log::info!("ELF loaded: {} variables", ctx.variables.len());
                                self.elf_ctx = Some(ctx);
                            }
                            Err(e) => log::error!("ELF load failed: {}", e),
                        }
                    }
                }

                // 右对齐：采样信息 + 释放按钮
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 采集进度
                    if let Some(target) = self.controls.target_count {
                        let pct = (self.controls.total_samples as f64 / target as f64 * 100.0).min(100.0);
                        ui.label(format!("Progress: {:.0}%", pct));
                        ui.separator();
                    }

                    // 实际采样率（运行中）
                    if self.controls.state == AcquisitionState::Running && self.controls.actual_rate_hz > 0.0 {
                        let pct = self.controls.actual_rate_hz / self.controls.sample_rate as f64 * 100.0;
                        let rate_label = format!("Actual: {:.0} Hz ({:.0}%)", self.controls.actual_rate_hz, pct);
                        if pct < 90.0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(0xff, 0xa0, 0x3c),
                                rate_label,
                            ).on_hover_text(
                                "DAP-Link 吞吐量不足，实际采样率低于目标。\n波形时间轴已使用真实时间戳，波形形状应正确。\n降低采样率可获得更均匀的采样间隔。"
                            );
                        } else {
                            ui.label(rate_label);
                        }
                        ui.separator();
                    }

                    // 采样计数
                    if self.controls.total_samples > 0 {
                        let duration = self.controls.total_samples as f64 / self.controls.sample_rate.max(1) as f64;
                        ui.label(format!("Samples: {} ({:.1}s)", self.controls.total_samples, duration));
                    }
                });
            });
        });

        // ---- 左侧面板：变量浏览器 + 通道 + 光标（可展开/收起）----
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(220.0)
            .width_range(220.0..=400.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("sidebar_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // --- 变量浏览器模块 ---
                        egui::CollapsingHeader::new("变量浏览器")
                            .default_open(self.var_browser_open)
                            .id_salt("var_browser_header")
                            .show(ui, |ui| {
                                let changes = self.variable_browser.show(
                                    ui,
                                    self.elf_ctx.as_ref(),
                                );
                                if !changes.is_empty() {
                                    self.apply_variable_changes(changes);
                                }
                            });

                        ui.separator();

                        // --- 通道模块 ---
                        egui::CollapsingHeader::new(
                            format!("通道 ({}/8)", self.channel_panel.channel_count())
                        )
                            .default_open(self.channels_open)
                            .id_salt("channels_header")
                            .show(ui, |ui| {
                                if let Some(remove_idx) = self.channel_panel.show(ui) {
                                    let name = self.channel_panel.channels[remove_idx].name.clone();
                                    self.variable_browser.checked_paths.remove(&name);
                                    self.channel_panel.remove_channel(remove_idx);
                                    // 同步 Watch 面板
                                    let names: Vec<String> = self.channel_panel.channels.iter().map(|c| c.name.clone()).collect();
                                    let types: Vec<ValueType> = self.channel_panel.channels.iter().map(|c| c.value_type).collect();
                                    self.watch_panel.sync_from_channels(&names, &types);
                                }
                            });

                        ui.separator();

                        // --- 光标模块 ---
                        egui::CollapsingHeader::new("光标")
                            .default_open(self.cursor_open)
                            .id_salt("cursor_header")
                            .show(ui, |ui| {
                                self.show_cursor_info(ui);
                            });
                    });
            });

        // ---- 底部 Watch 面板 ----
        egui::TopBottomPanel::bottom("watch_panel")
            .resizable(true)
            .default_height(140.0)
            .min_height(60.0)
            .show(ctx, |ui| {
                self.watch_panel.show(ui);
            });

        // ---- 中央：工具栏 + 波形 ----
        self.sync_channel_visibility();
        let has_new_data = self.has_new_data;
        self.has_new_data = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            // 工具栏：采集控制 + 参数 + 显示模式
            ui.horizontal_wrapped(|ui| {
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

                // 检测采样率变化
                if self.controls.sample_rate != self.rate_hz {
                    self.rate_hz = self.controls.sample_rate;
                    self.interval_us = 1_000_000.0 / self.rate_hz as f64;
                    self.waveform.set_interval(self.interval_us);
                    log::info!("采样率已变更为 {} Hz", self.rate_hz);
                }

                // 检测窗口大小变化
                if self.controls.window_size != self.window_size {
                    self.window_size = self.controls.window_size;
                    self.display_buf.set_max_samples(self.window_size);
                    log::info!("显示窗口大小已变更为 {} 个点", self.window_size);
                }

                ui.separator();

                // 波形显示模式切换
                ui.label("Display:");
                let mut mode = self.display_mode;
                egui::ComboBox::from_id_salt("display_mode")
                    .selected_text(match mode {
                        WaveformDisplayMode::Line => "Line",
                        WaveformDisplayMode::Point => "Point",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut mode, WaveformDisplayMode::Line, "Line");
                        ui.selectable_value(&mut mode, WaveformDisplayMode::Point, "Point");
                    });
                if mode != self.display_mode {
                    self.display_mode = mode;
                    self.waveform.set_display_mode(mode);
                }
            });

            ui.separator();

            // 波形
            let available_width = ui.available_width();
            let buffer_offset = self.display_buf.oldest_seq();
            if let Some(seq) = self.waveform.show(
                ui,
                self.display_buf.all(),
                buffer_offset,
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
        // 显式释放 USB：发送 DAP_Disconnect + release_interface
        if let Some(usb) = self.active_usb.take() {
            match Arc::try_unwrap(usb) {
                Ok(mut bulk) => { bulk.release(); }
                Err(arc) => {
                    if let Some(bulk) = Arc::get_mut(&mut arc.clone()) { bulk.release(); }
                    drop(arc);
                }
            }
        }
    }
}

/// 强制扫描并释放所有 CMSIS-DAP 设备
///
/// 当程序没有活跃的 USB 连接但设备仍被占用时使用。
/// 遍历 USB 总线，找到 DAP-Link 设备，打开后 release 所有接口再关闭。
#[allow(dead_code)]
fn force_release_all_daplink() {
    use rusb::{Context, UsbContext};
    use crate::usb::device::KNOWN_DEVICES;

    let context = match Context::new() {
        Ok(c) => c,
        Err(e) => {
            log::error!("创建 USB Context 失败: {}", e);
            return;
        }
    };

    let devices = match context.devices() {
        Ok(d) => d,
        Err(e) => {
            log::error!("枚举 USB 设备失败: {}", e);
            return;
        }
    };

    let mut released_count = 0;
    for device in devices.iter() {
        let desc = match device.device_descriptor() {
            Ok(d) => d,
            Err(_) => continue,
        };
        let vid = desc.vendor_id();
        let pid = desc.product_id();

        // 检查是否为已知 DAP-Link 设备
        let known = KNOWN_DEVICES.iter().any(|&(v, p)| v == vid && p == pid);
        if !known {
            // 也检查产品字符串
            match device.open() {
                Ok(handle) => {
                    let is_cmsis = desc.product_string_index()
                        .and_then(|idx| handle.read_string_descriptor_ascii(idx).ok())
                        .map(|s| s.to_uppercase().contains("CMSIS-DAP"))
                        .unwrap_or(false);
                    if !is_cmsis {
                        continue;
                    }
                }
                Err(_) => continue,
            }
        }

        log::info!("找到 DAP-Link 设备 VID={:04X} PID={:04X}，尝试释放", vid, pid);

        match device.open() {
            Ok(handle) => {
                // 尝试 release 所有接口
                if let Ok(config) = device.config_descriptor(0) {
                    for interface in config.interfaces() {
                        for alt in interface.descriptors() {
                            let iface_num = alt.interface_number();
                            log::info!("  release interface {}", iface_num);
                            match handle.release_interface(iface_num) {
                                Ok(()) => {
                                    log::info!("  interface {} 已释放", iface_num);
                                    released_count += 1;
                                }
                                Err(e) => {
                                    log::warn!("  release interface {} 失败: {}", iface_num, e);
                                }
                            }
                        }
                    }
                }
                // 重置设备
                log::info!("  reset USB device");
                if let Err(e) = handle.reset() {
                    log::warn!("  reset 失败: {}", e);
                }
            }
            Err(e) => {
                log::warn!("  打开设备失败: {}", e);
            }
        }
    }

    if released_count > 0 {
        log::info!("共释放 {} 个接口", released_count);
    } else {
        log::info!("未找到需要释放的 DAP-Link 设备（可能已释放或未连接）");
    }
}
