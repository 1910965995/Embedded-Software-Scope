use crate::pipeline::sample::Sample;
use crate::pipeline::engine::{PipelineEngine, PipelineHandle};
use super::display_buffer::DisplayBuffer;
use super::waveform::WaveformPanel;
use super::controls::{ControlPanel, AcquisitionState, AcquisitionCommand};
use super::cursor::CursorState;

/// 默认颜色调色板（6 种颜色循环使用）
const CHANNEL_COLORS: [egui::Color32; 6] = [
    egui::Color32::from_rgb(255, 68, 68),   // 红
    egui::Color32::from_rgb(68, 255, 68),   // 绿
    egui::Color32::from_rgb(68, 68, 255),   // 蓝
    egui::Color32::from_rgb(255, 255, 68),  // 黄
    egui::Color32::from_rgb(255, 68, 255),  // 品红
    egui::Color32::from_rgb(68, 255, 255),  // 青
];

/// DAP Sampler 主应用
///
/// 实现 eframe::App trait，是 egui 窗口的核心。
pub struct DapSamplerApp {
    /// 流水线句柄（启动后 Some，停止后 None）
    pipeline: Option<PipelineHandle>,
    /// 流水线引擎（启动前持有，启动时消费）
    engine: Option<PipelineEngine>,
    /// 显示缓冲区
    display_buf: DisplayBuffer,
    /// 波形面板
    waveform: WaveformPanel,
    /// 控制面板
    controls: ControlPanel,
    /// 光标状态
    cursor: CursorState,
    /// 临时采样缓冲区（每帧复用，避免重复分配）
    temp_buf: Vec<Sample>,
    /// 采样间隔（微秒）
    interval_us: f64,
}

impl DapSamplerApp {
    /// 创建应用（传入已初始化的 PipelineEngine）
    pub fn new(
        engine: PipelineEngine,
        addresses: Vec<String>,
        rate_hz: u32,
        target_count: Option<u64>,
    ) -> Self {
        let num_channels = addresses.len();
        let channel_names: Vec<String> = addresses
            .iter()
            .enumerate()
            .map(|(i, a)| format!("CH{} {}", i + 1, a))
            .collect();
        let channel_colors: Vec<egui::Color32> = (0..num_channels)
            .map(|i| CHANNEL_COLORS[i % CHANNEL_COLORS.len()])
            .collect();
        let interval_us = 1_000_000.0 / rate_hz as f64;

        Self {
            pipeline: None,
            engine: Some(engine),
            display_buf: DisplayBuffer::new(200_000), // 10 秒 @ 20kHz
            waveform: WaveformPanel::new(channel_names, channel_colors, interval_us),
            controls: ControlPanel::new(rate_hz, target_count),
            cursor: CursorState::new(),
            temp_buf: (0..1024).map(|_| Sample { seq: 0, values: vec![] }).collect(),
            interval_us,
        }
    }

    /// 启动采集
    fn start_acquisition(&mut self) {
        if let Some(engine) = self.engine.take() {
            match engine.start() {
                Ok(handle) => {
                    self.pipeline = Some(handle);
                    self.controls.set_running();
                }
                Err(e) => {
                    log::error!("Failed to start acquisition: {}", e);
                }
            }
        }
    }

    /// 停止采集
    fn stop_acquisition(&mut self) {
        if let Some(handle) = self.pipeline.take() {
            handle.stop();
        }
        self.controls.set_stopped();
    }

    /// 暂停采集
    fn pause_acquisition(&mut self) {
        // 暂停 = 停止流水线但保留显示数据
        self.stop_acquisition();
        self.controls.set_paused();
    }

    /// 消费环形缓冲区数据到显示缓冲区
    fn drain_pipeline(&mut self) {
        // 先从 pipeline 读取数据到 temp_buf
        let n = if let Some(ref handle) = self.pipeline {
            handle.drain_samples(&mut self.temp_buf)
        } else {
            0
        };

        // 然后追加到显示缓冲区（避免同时借用 pipeline 和 display_buf）
        if n > 0 {
            self.display_buf.push_batch(&self.temp_buf[..n]);
            self.controls.update_count(self.display_buf.next_seq());
        }
    }
}

impl eframe::App for DapSamplerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 每帧消费新数据
        self.drain_pipeline();

        // 检查是否达到目标采样数
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
                if let Some(cmd) = self.controls.show(ui) {
                    match cmd {
                        AcquisitionCommand::Start => {
                            if self.controls.state == AcquisitionState::Idle {
                                self.start_acquisition();
                            } else if self.controls.state == AcquisitionState::Paused {
                                // 暂停后重新开始：需要重新创建 engine
                                // 简化处理：不支持暂停恢复，仅支持停止后重新开始
                            }
                        }
                        AcquisitionCommand::Pause => self.pause_acquisition(),
                        AcquisitionCommand::Stop => self.stop_acquisition(),
                    }
                }
            });
        });

        // ---- 图例（通道开关） ----
        egui::SidePanel::left("legend_panel")
            .resizable(false)
            .default_width(180.0)
            .show(ctx, |ui| {
                ui.heading("Channels");
                ui.separator();
                for i in 0..self.waveform.channel_count() {
                    let names = self.waveform.channel_names();
                    let name = names[i].to_string();
                    let color = CHANNEL_COLORS[i % CHANNEL_COLORS.len()];
                    ui.horizontal(|ui| {
                        ui.colored_label(color, "●");
                        if ui.button(&name).clicked() {
                            self.waveform.toggle_channel(i);
                        }
                    });
                }
                ui.separator();

                // 光标信息
                ui.heading("Cursor");
                let interval_us = self.interval_us;
                if let Some(r) = self.cursor.get_result(
                    self.display_buf.all(),
                    self.display_buf.oldest_seq(),
                    interval_us,
                ) {
                    ui.label(format!("T: {:.6}s", r.time_sec));
                    for (i, v) in r.values.iter().enumerate() {
                        ui.label(format!("  CH{}: {:.4}", i + 1, v));
                    }

                    // 双光标差值
                    if self.cursor.cursor2.is_some() {
                        if let Some((dt, dv)) = self.cursor.delta(
                            self.display_buf.all(),
                            self.display_buf.oldest_seq(),
                            interval_us,
                        ) {
                            ui.separator();
                            ui.label(format!("ΔT: {:.6}s", dt));
                            for (i, v) in dv.iter().enumerate() {
                                ui.label(format!("  ΔCH{}: {:.4}", i + 1, v));
                            }
                        }
                    }
                } else {
                    ui.label("Click waveform to place cursor");
                }

                ui.separator();
                if ui.button("Clear Cursor").clicked() {
                    self.cursor.clear();
                }
            });

        // ---- 中央波形区域 ----
        egui::CentralPanel::default().show(ctx, |ui| {
            let available_width = ui.available_width();
            self.waveform.show(
                ui,
                self.display_buf.all(),
                available_width,
                &self.cursor,
            );
        });

        // 请求持续刷新（60fps）
        ctx.request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // 窗口关闭时停止采集
        self.stop_acquisition();
    }
}
