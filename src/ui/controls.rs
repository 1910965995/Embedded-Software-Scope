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
}

impl ControlPanel {
    pub fn new(sample_rate: u32, target_count: Option<u64>) -> Self {
        Self {
            state: AcquisitionState::Idle,
            sample_rate,
            total_samples: 0,
            target_count,
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

            let duration = self.total_samples as f64 / self.sample_rate as f64;
            ui.label(format!("Rate: {} Hz", self.sample_rate));
            ui.label(format!("Samples: {} ({:.1}s)", self.total_samples, duration));

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
