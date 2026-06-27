/// 采集状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AcquisitionState {
    /// 空闲（未开始）
    Idle,
    /// 采集中
    Running,
    /// 已暂停
    Paused,
}

/// 用户操作命令
#[derive(Debug, Clone, Copy)]
pub enum AcquisitionCommand {
    Start,
    Pause,
    Stop,
}

/// 控制面板
///
/// 提供开始/暂停/停止按钮，显示采样率和已采集点数。
pub struct ControlPanel {
    pub state: AcquisitionState,
    pub sample_rate: u32,
    pub total_samples: u64,
    pub target_count: Option<u64>,
    /// 实际达到的采样率（由外部计算后更新）
    pub actual_rate_hz: f64,
    /// 显示窗口大小（采样点数），超过则丢弃旧数据
    pub window_size: usize,
}

/// 采样率范围
pub const RATE_MIN: u32 = 1;
pub const RATE_MAX: u32 = 15_000;
/// 窗口大小范围
pub const WINDOW_MIN: usize = 1;
pub const WINDOW_MAX: usize = 10_000;

impl ControlPanel {
    pub fn new(sample_rate: u32, target_count: Option<u64>) -> Self {
        let sample_rate = sample_rate.clamp(RATE_MIN, RATE_MAX);
        Self {
            state: AcquisitionState::Idle,
            sample_rate,
            total_samples: 0,
            target_count,
            actual_rate_hz: 0.0,
            window_size: 2000,
        }
    }

    /// 渲染控制面板，返回用户操作
    ///
    /// 返回 `Some(AcquisitionCommand)` 表示用户点击了按钮。
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<AcquisitionCommand> {
        let mut cmd = None;

        ui.horizontal(|ui| {
            // 开始/暂停按钮
            match self.state {
                AcquisitionState::Idle | AcquisitionState::Paused => {
                    if ui.button("▶ Start").clicked() {
                        cmd = Some(AcquisitionCommand::Start);
                    }
                }
                AcquisitionState::Running => {
                    if ui.button("⏸ Pause").clicked() {
                        cmd = Some(AcquisitionCommand::Pause);
                    }
                }
            }

            // 停止按钮
            if self.state != AcquisitionState::Idle {
                if ui.button("⏹ Stop").clicked() {
                    cmd = Some(AcquisitionCommand::Stop);
                }
            }

            ui.separator();

            // 状态显示
            let state_text = match self.state {
                AcquisitionState::Idle => "Idle",
                AcquisitionState::Running => "Running",
                AcquisitionState::Paused => "Paused",
            };
            ui.label(format!("State: {}", state_text));

            // 采样率：仅在 Idle 时可修改（点击可输入数字，1-30000 Hz）
            let is_idle = self.state == AcquisitionState::Idle;
            ui.add_enabled_ui(is_idle, |ui| {
                ui.label("Rate:");
                ui.add(
                    egui::DragValue::new(&mut self.sample_rate)
                        .range(RATE_MIN..=RATE_MAX)
                        .clamp_existing_to_range(true)
                        .speed(100.0)
                        .suffix(" Hz")
                );
            });

            // 显示窗口大小：仅在 Idle 时可修改（点击可输入数字，1-10000 点）
            ui.add_enabled_ui(is_idle, |ui| {
                ui.label("Window:");
                ui.add(
                    egui::DragValue::new(&mut self.window_size)
                        .range(WINDOW_MIN..=WINDOW_MAX)
                        .clamp_existing_to_range(true)
                        .speed(10.0)
                        .suffix(" pts")
                );
            });

            let duration = self.total_samples as f64 / self.sample_rate as f64;
            ui.label(format!("Samples: {} ({:.1}s)", self.total_samples, duration));

            // 运行中显示实际采样率
            if self.state == AcquisitionState::Running && self.actual_rate_hz > 0.0 {
                let pct = self.actual_rate_hz / self.sample_rate as f64 * 100.0;
                let rate_label = format!(
                    "Actual: {:.0} Hz ({:.0}%)",
                    self.actual_rate_hz, pct
                );
                // 实际速率低于目标 90% 时用警告色显示
                if pct < 90.0 {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 160, 60),
                        rate_label,
                    ).on_hover_text(
                        "DAP-Link 吞吐量不足，实际采样率低于目标。\n波形时间轴已自动使用真实时间戳，波形形状应正确。\n降低采样率可获得更均匀的采样间隔。"
                    );
                } else {
                    ui.label(rate_label);
                }
            }

            if let Some(target) = self.target_count {
                let pct = (self.total_samples as f64 / target as f64 * 100.0).min(100.0);
                ui.label(format!("Progress: {:.0}%", pct));
            }
        });

        cmd
    }

    /// 更新状态（由外部调用）
    pub fn set_running(&mut self) {
        self.state = AcquisitionState::Running;
    }

    pub fn set_paused(&mut self) {
        self.state = AcquisitionState::Paused;
    }

    pub fn set_stopped(&mut self) {
        self.state = AcquisitionState::Idle;
    }

    pub fn update_count(&mut self, total: u64) {
        self.total_samples = total;
    }
}
