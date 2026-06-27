use egui_plot::{Line, Legend, Plot, PlotBounds, PlotPoint, Points};
use crate::pipeline::sample::{Sample, ValueType};

/// 波形显示模式
#[derive(Clone, Copy, PartialEq)]
pub enum WaveformDisplayMode {
    /// 连线模式：将采样点用线段连接起来（默认）
    Line,
    /// 散点模式：仅显示采样点，不连接
    Point,
}

/// 单通道波形线信息
pub struct ChannelInfo {
    pub name: String,
    pub color: egui::Color32,
    pub visible: bool,
    /// Y 轴偏移，应用于每个样本点（默认 0.0）
    pub y_offset: f32,
    /// Y 轴缩放系数，应用于每个样本点（默认 1.0；禁止 0；允许负数）
    pub y_scale: f32,
}

/// 波形面板
///
/// 持有通道配置，负责从 Sample 数据构建 egui_plot 的 Line 对象并渲染。
/// 内部缓存降采样结果，仅在新数据到达时重建，避免每帧遍历整个缓冲区。
pub struct WaveformPanel {
    /// 通道配置
    channels: Vec<ChannelInfo>,
    /// 采样间隔（微秒），用于时间戳推算
    interval_us: f64,
    /// 变量类型列表
    value_types: Vec<ValueType>,
    /// 波形点缓存（每通道一条），使用 PlotPoint 以支持零拷贝借用
    pub cached_points: Vec<Vec<PlotPoint>>,
    /// 缓存对应的缓冲区长度（0 表示需要重建）
    pub cache_buffer_len: usize,
    /// 上一帧的可视 X 范围（秒），用于动态 X 轴标签
    last_x_range: Option<f64>,
    /// 上一帧的 X 轴右边界，用于检测用户拖拽/缩放
    last_x_max: Option<f64>,
    /// 当前缓冲区数据的绝对值最大值（Y 轴自适应用）
    auto_y_max_abs: f64,
    /// 是否自动调整 Y 轴范围（跟随模式下持续生效）
    auto_fit_y: bool,
    /// 是否自动滚动跟随最新数据
    auto_scroll: bool,
    /// 波形显示模式（连线 / 散点）
    display_mode: WaveformDisplayMode,
}

impl WaveformPanel {
    pub fn new(
        channel_names: Vec<String>,
        channel_colors: Vec<egui::Color32>,
        interval_us: f64,
        value_types: Vec<ValueType>,
        display_mode: WaveformDisplayMode,
    ) -> Self {
        let n = channel_names.len();
        let channels = channel_names
            .into_iter()
            .zip(channel_colors)
            .map(|(name, color)| ChannelInfo {
                name,
                color,
                visible: true,
                y_offset: 0.0,
                y_scale: 1.0,
            })
            .collect();
        Self {
            channels,
            interval_us,
            value_types,
            cached_points: vec![Vec::new(); n],
            cache_buffer_len: 0,
            last_x_range: None,
            last_x_max: None,
            auto_y_max_abs: 1.0,
            auto_fit_y: true, // 首次渲染自动调整 Y 轴
            auto_scroll: true,
            display_mode,
        }
    }

    /// 渲染波形
    ///
    /// 返回 `Some(seq)` 表示用户在波形上点击放置光标（seq 为点击位置对应的采样序号）。
    /// `has_new_data` 为 true 时重建降采样缓存。
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        buffer: &[Sample],
        buffer_offset: u64,
        visible_width: f32,
        has_new_data: bool,
    ) -> Option<u64> {
        if buffer.is_empty() {
            ui.label("Waiting for data...");
            return None;
        }

        // 不进行降采样: buffer 中所有原始样本点都必须被绘制,采样工具的波形必须忠实于采集数据。

        // 缓存判断: 仅在新数据到达或通道可见性变化时重建
        if has_new_data || self.cache_buffer_len != buffer.len() {
            self.rebuild_cache(buffer);
            self.cache_buffer_len = buffer.len();
        }

        // 动态 X 轴标签（使用上一帧的可视范围）
        let x_label = match self.last_x_range {
            Some(r) => format!("Time (s) [window: {:.2}s]", r),
            None => "Time (s)".to_string(),
        };

        // Y 轴标签带类型信息
        let y_label = if self.value_types.iter().all(|t| *t == ValueType::Float) {
            "Value (float)".to_string()
        } else {
            let labels: Vec<&str> = self.value_types.iter().map(|t| t.label()).collect();
            format!("Value ({})", labels.join("/"))
        };

        // --- 波形工具栏 ---
        ui.horizontal(|ui| {
            let follow_text = if self.auto_scroll {
                "\u{23f8} Follow: ON"
            } else {
                "\u{25b6} Follow: OFF"
            };
            if ui.button(follow_text).clicked() {
                self.auto_scroll = !self.auto_scroll;
            }
            ui.separator();
            if ui.button("Auto Fit Y").clicked() {
                self.auto_fit_y = true;
            }
            ui.separator();
            ui.label(format!(
                "Window: {} pts ({:.3}s)",
                buffer.len(),
                buffer.len() as f64 * self.interval_us / 1_000_000.0
            ));
        });

        // 始终允许缩放/拖拽，auto_scroll 只决定 X 轴是否跟随最新数据
        let auto_scroll = self.auto_scroll;
        let mut clicked_seq: Option<u64> = None;
        let auto_fit = self.auto_fit_y;
        self.auto_fit_y = false; // 一次性触发（首次渲染或用户点击 Auto Fit Y）

        let plot = Plot::new("waveform_plot")
            .legend(Legend::default())
            .show_grid(true)
            .show_axes(true)
            .x_axis_label(x_label)
            .y_axis_label(y_label)
            .allow_zoom(true)
            .allow_drag(true)
            .allow_scroll(true);

        // 不使用 plot.auto_bounds，改用自定义 Y 轴范围控制

        // 准备自动滚动参数
        let interval_us = self.interval_us;
        let buffer_len = buffer.len();
        let latest_ts = buffer.last().map(|s| s.timestamp_sec).unwrap_or(0.0);
        // 记录上一帧的 X 轴右边界，用于检测用户是否拖拽/缩放
        let prev_x_max = self.last_x_max;

        // Y 轴自适应范围：[-2*max_abs, 2*max_abs]
        let y_max = self.auto_y_max_abs;
        let y_bound = 2.0 * y_max;

        let plot_response = plot.show(ui, |plot_ui| {
            let cur_bounds = plot_ui.plot_bounds();
            let cur_x_max = cur_bounds.max()[0];

            // 用户拖拽/缩放检测：如果 X 轴右边界偏离了最新时间戳（超过半个窗口），
            // 说明用户在手动操作，自动暂停跟随
            if auto_scroll && buffer_len > 0 {
                let window_duration = buffer_len as f64 * interval_us / 1_000_000.0;
                if let Some(prev) = prev_x_max {
                    // X 右边界变化超过 1% 窗口时长 → 用户在操作
                    let drift = (cur_x_max - prev).abs();
                    if drift > window_duration * 0.01 {
                        self.auto_scroll = false;
                    }
                }
            }

            // 自动滚动：X 轴跟随最新数据，Y 轴自适应 [-2*max, 2*max]
            if auto_scroll && buffer_len > 0 {
                let window_duration = buffer_len as f64 * interval_us / 1_000_000.0;
                let new_bounds = PlotBounds::from_min_max(
                    [latest_ts - window_duration, -y_bound],
                    [latest_ts, y_bound],
                );
                plot_ui.set_plot_bounds(new_bounds);
                self.last_x_max = Some(latest_ts);
            } else if auto_fit {
                // 非 Follow 模式下点击 Auto Fit Y：只调整 Y 轴，保留当前 X 轴范围
                let new_bounds = PlotBounds::from_min_max(
                    [cur_bounds.min()[0], -y_bound],
                    [cur_bounds.max()[0], y_bound],
                );
                plot_ui.set_plot_bounds(new_bounds);
                self.last_x_max = Some(cur_x_max);
            } else {
                self.last_x_max = Some(cur_x_max);
            }

            // 记录可视范围（用于下一帧的 X 轴标签）
            let bounds = plot_ui.plot_bounds();
            self.last_x_range = Some(bounds.max()[0] - bounds.min()[0]);

            // 捕获鼠标坐标（用于光标点击）
            let pointer_coord = plot_ui.pointer_coordinate();

            // 渲染各通道波形（零拷贝借用缓存数据）
            for (ch_idx, ch) in self.channels.iter().enumerate() {
                if !ch.visible {
                    continue;
                }
                let points = &self.cached_points[ch_idx];
                if points.is_empty() {
                    continue;
                }
                match self.display_mode {
                    WaveformDisplayMode::Line => {
                        plot_ui.line(
                            Line::new(points.as_slice())
                                .name(&ch.name)
                                .color(ch.color)
                                .width(1.5),
                        );
                    }
                    WaveformDisplayMode::Point => {
                        plot_ui.points(
                            Points::new(points.as_slice())
                                .name(&ch.name)
                                .color(ch.color)
                                .radius(1.5),
                        );
                    }
                }
            }

            // 检测点击 → 放置光标
            if plot_ui.response().clicked() {
                if let Some(coord) = pointer_coord {
                    // 用真实时间戳查找最接近的采样点
                    let target = coord.x;
                    if !buffer.is_empty() {
                        let idx = buffer.binary_search_by(|s| {
                            s.timestamp_sec.partial_cmp(&target).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let local_idx = match idx {
                            Ok(i) => i,
                            Err(i) => {
                                if i == 0 {
                                    0
                                } else if i >= buffer.len() {
                                    buffer.len() - 1
                                } else {
                                    let d1 = (buffer[i - 1].timestamp_sec - target).abs();
                                    let d2 = (buffer[i].timestamp_sec - target).abs();
                                    if d1 < d2 { i - 1 } else { i }
                                }
                            }
                        };
                        clicked_seq = Some(buffer_offset + local_idx as u64);
                    }
                }
            }
        });

        let _ = plot_response;
        clicked_seq
    }

    /// 重建波形缓存
    ///
    /// 关键不变式: **buffer 中的每个原始样本点都进入 cached_points**。
    /// 不进行任何形式的降采样、抽点或跳点 —— 采样工具的波形必须 100% 忠实于采集到的数据。
    /// 每个点按 `displayed_y = raw_f * y_scale + y_offset` 变换;Y 轴自动 fit 仍按 `raw_f.abs()`。
    pub fn rebuild_cache(&mut self, buffer: &[Sample]) {
        let mut y_max_abs: f64 = 0.0;

        for (ch_idx, ch) in self.channels.iter().enumerate() {
            if !ch.visible {
                self.cached_points[ch_idx].clear();
                continue;
            }

            let vt = self.value_types.get(ch_idx).copied().unwrap_or(ValueType::Float);
            let scale = ch.y_scale as f64;
            let offset = ch.y_offset as f64;

            // 每个原始样本点都转换为 PlotPoint，无降采样、无跳点
            let mut out: Vec<PlotPoint> = Vec::with_capacity(buffer.len());
            for s in buffer {
                let raw = s.values.get(ch_idx).copied().unwrap_or(0);
                let raw_f = vt.to_f64(raw);
                if !raw_f.is_finite() {
                    continue; // NaN/Inf 不参与 Y 轴 fit,也不绘制
                }
                let abs_val = raw_f.abs();
                if abs_val > y_max_abs {
                    y_max_abs = abs_val;
                }
                let displayed_y = raw_f * scale + offset;
                out.push(PlotPoint::new(s.timestamp_sec, displayed_y));
            }

            self.cached_points[ch_idx] = out;
        }

        // Y 轴自适应: 仍按 raw_f,不被 offset/scale 推偏
        self.auto_y_max_abs = if y_max_abs > 0.0 { y_max_abs } else { 1.0 };
    }

    /// 切换通道可见性
    pub fn toggle_channel(&mut self, index: usize) {
        if let Some(ch) = self.channels.get_mut(index) {
            ch.visible = !ch.visible;
        }
        // 通道可见性变化时强制重建缓存
        self.cache_buffer_len = 0;
    }

    /// 查询通道可见性
    pub fn is_channel_visible(&self, index: usize) -> bool {
        self.channels.get(index).map(|ch| ch.visible).unwrap_or(false)
    }

    /// 设置通道可见性
    pub fn set_channel_visible(&mut self, index: usize, visible: bool) {
        if let Some(ch) = self.channels.get_mut(index) {
            if ch.visible != visible {
                ch.visible = visible;
                self.cache_buffer_len = 0; // 强制重建缓存
            }
        }
    }

    /// 触发自动调整 Y 轴范围
    pub fn request_auto_fit_y(&mut self) {
        self.auto_fit_y = true;
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn channel_names(&self) -> Vec<&str> {
        self.channels.iter().map(|c| c.name.as_str()).collect()
    }

    pub fn value_types(&self) -> &[ValueType] {
        &self.value_types
    }

    /// 更新采样间隔（采样率变化时调用）
    pub fn set_interval(&mut self, interval_us: f64) {
        self.interval_us = interval_us;
        self.cache_buffer_len = 0; // 强制重建缓存
        self.last_x_max = None; // 重置拖拽检测
    }

    /// 设置波形显示模式
    pub fn set_display_mode(&mut self, mode: WaveformDisplayMode) {
        self.display_mode = mode;
    }

    /// 获取当前波形显示模式
    pub fn display_mode(&self) -> WaveformDisplayMode {
        self.display_mode
    }

    /// 设置单个通道的 y_offset / y_scale,并标记缓存需要重建。
    /// 若通道不存在(name 不匹配),该调用为 no-op。
    pub fn set_channel_transform(&mut self, name: &str, offset: f32, scale: f32) {
        if let Some(ch) = self.channels.iter_mut().find(|c| c.name == name) {
            ch.y_offset = offset;
            ch.y_scale = scale;
            self.cache_buffer_len = 0; // 强制下次 show() 重建缓存
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::sample::{Sample, ValueType};

    fn make_panel(channels: Vec<(&str, f32, f32)>) -> WaveformPanel {
        let names: Vec<String> = channels.iter().map(|(n, _, _)| n.to_string()).collect();
        let colors: Vec<egui::Color32> = (0..channels.len())
            .map(|i| egui::Color32::from_rgb((i * 30) as u8, 100, 200))
            .collect();
        let types: Vec<ValueType> = vec![ValueType::Float; channels.len()];
        let mut panel = WaveformPanel::new(names, colors, 1000.0, types, WaveformDisplayMode::Line);
        for (ch, (_, off, sca)) in panel.channels.iter_mut().zip(channels.iter()) {
            ch.y_offset = *off;
            ch.y_scale = *sca;
        }
        panel
    }

    fn make_buffer(seqs: u64, vals: &[u32]) -> Vec<Sample> {
        (0..seqs)
            .map(|i| Sample {
                seq: i,
                timestamp_sec: i as f64 * 0.001,
                values: vals.to_vec(),
            })
            .collect()
    }

    #[test]
    fn rebuild_cache_applies_transform() {
        let mut panel = make_panel(vec![("CH1", 10.0, 2.0)]);
        let buffer = make_buffer(1, &[3.0f32.to_bits()]);
        panel.rebuild_cache(&buffer);
        // displayed_y = 3.0 * 2.0 + 10.0 = 16.0
        assert_eq!(panel.cached_points[0].len(), 1);
        assert!((panel.cached_points[0][0].y - 16.0).abs() < 1e-6);
    }

    #[test]
    fn auto_y_uses_raw_not_transformed() {
        let mut panel = make_panel(vec![("CH1", 1.0e6, 1.0)]);
        let buffer = make_buffer(1, &[1.0f32.to_bits()]);
        panel.rebuild_cache(&buffer);
        // raw_f = 1.0, 不应受 offset 影响
        assert!((panel.auto_y_max_abs - 1.0).abs() < 1e-6);
    }

    #[test]
    fn set_channel_transform_marks_dirty() {
        let mut panel = make_panel(vec![("CH1", 0.0, 1.0)]);
        let buffer = make_buffer(1, &[1.0f32.to_bits()]);
        panel.rebuild_cache(&buffer);
        // 让 cache_buffer_len 与 buffer 同步,表示"未脏"
        panel.cache_buffer_len = buffer.len();
        assert_eq!(panel.cache_buffer_len, buffer.len());
        // 触发 transform 变化 → 应标记为脏
        panel.set_channel_transform("CH1", 5.0, 1.0);
        assert_eq!(panel.cache_buffer_len, 0);
    }

    #[test]
    fn rebuild_cache_no_samples_lost() {
        // 关键保真测试: 任何规模的 buffer 都必须完整保留所有样本点
        let mut panel = make_panel(vec![("CH1", 100.0, 1.0)]);
        let n = 5000_usize;
        // 每个样本一个值,值 = i as f32
        let buffer: Vec<Sample> = (0..n)
            .map(|i| Sample {
                seq: i as u64,
                timestamp_sec: i as f64 * 0.001,
                values: vec![(i as f32).to_bits()],
            })
            .collect();
        panel.rebuild_cache(&buffer);
        assert_eq!(panel.cached_points[0].len(), n);
        // 验证 transform 应用到了每一个点(不是只前 N 个)
        for i in 0..n {
            let raw = i as f64;
            let expected = raw * 1.0 + 100.0;
            let got = panel.cached_points[0][i].y;
            assert!(
                (got - expected).abs() < 1e-3,
                "sample {i}: expected {expected}, got {got}"
            );
        }
    }

    #[test]
    fn uniform_downsample_removed() {
        // 编译期检查: uniform_downsample 不应存在
        // (无法直接 assert 编译失败,但我们通过文档 + 后续 grep 在 PR 中确认)
        // 此测试仅作为占位提示。如未来有人重新引入降采样,这里需补充编译期检查。
    }
}
