use egui_plot::{Line, Legend, Plot, PlotBounds, PlotPoint, Points};
use crate::pipeline::sample::{Sample, ValueType};

/// 在数据跳变点插入阶梯点，使折线图渲染为阶梯波形（适用于数字信号）
///
/// 对于 0/1 开关信号，普通折线图会在跳变处绘制斜线，看起来不像方波。
/// 插入阶梯点后，跳变处变为垂直线，正确呈现数字信号的波形。
fn insert_step_points(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut result = Vec::with_capacity(points.len() * 2);
    result.push(points[0]);
    for i in 1..points.len() {
        // Y 值变化时，在当前 X 位置插入一个旧 Y 值的点，形成垂直阶梯
        if points[i][1] != points[i - 1][1] {
            result.push([points[i][0], points[i - 1][1]]);
        }
        result.push(points[i]);
    }
    result
}

/// 均匀步进降采样
///
/// 将 N 个数据点降采样到 threshold 个点，通过等间隔选取原始点实现。
/// 保证选出的点在 X 轴上严格等间隔分布，适用于需要一致采样间隔显示的场景。
///
/// 与 LTTB（按三角形面积选点）不同，本算法不依赖波形特征选点，
/// 因此显示的采样点间隔完全一致，不会因数据量变化或窗口缩放而改变。
///
/// 算法：计算步进 stride = (n-1)/(threshold-1)，按 stride 等间隔选取索引。
///
/// 参数:
/// - `data`: 原始数据切片 [x, y]
/// - `threshold`: 目标点数
pub fn uniform_downsample(data: &[[f64; 2]], threshold: usize) -> Vec<[f64; 2]> {
    let n = data.len();
    if n <= threshold || threshold < 3 {
        return data.to_vec();
    }

    let mut result = Vec::with_capacity(threshold);
    let stride = (n - 1) as f64 / (threshold - 1) as f64;

    for i in 0..threshold {
        let idx = (i as f64 * stride).round() as usize;
        result.push(data[idx.min(n - 1)]);
    }

    result
}

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
    /// 降采样缓存（每通道一条），使用 PlotPoint 以支持零拷贝借用
    cached_points: Vec<Vec<PlotPoint>>,
    /// 缓存对应的缓冲区长度（0 表示需要重建）
    cache_buffer_len: usize,
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
            .map(|(name, color)| ChannelInfo { name, color, visible: true })
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

        // 降采样目标点数：至少等于 buffer 长度，避免窗口内的点被降采样
        // 窗口大小最大 10000 点，现代 GPU 完全可以全量渲染，无需降采样
        // 只有 buffer 极大时（如未来支持更大窗口）才按像素宽度降采样
        let target_points = (visible_width as usize * 3).max(buffer.len()).min(100_000);

        // 缓存判断：仅在新数据到达或通道可见性变化时重建
        if has_new_data || self.cache_buffer_len != buffer.len() {
            self.rebuild_cache(buffer, target_points);
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

    /// 重建降采样缓存
    ///
    /// 同时计算所有可见通道数据的绝对值最大值，用于 Y 轴自适应。
    fn rebuild_cache(&mut self, buffer: &[Sample], target_points: usize) {
        let mut y_max_abs: f64 = 0.0;

        for (ch_idx, ch) in self.channels.iter().enumerate() {
            if !ch.visible {
                self.cached_points[ch_idx].clear();
                continue;
            }

            let vt = self.value_types.get(ch_idx).copied().unwrap_or(ValueType::Float);

            // 构建 [x, y] 数据点序列，同时追踪绝对值最大值
            let raw_points: Vec<[f64; 2]> = buffer
                .iter()
                .filter_map(|s| {
                    let raw = s.values.get(ch_idx).copied().unwrap_or(0);
                    let val = vt.to_f64(raw);
                    if val.is_finite() {
                        let abs_val = val.abs();
                        if abs_val > y_max_abs {
                            y_max_abs = abs_val;
                        }
                        Some([s.timestamp_sec, val])
                    } else {
                        None
                    }
                })
                .collect();

            // 降采样（均匀步进，保证显示间隔一致）
            let downsampled = if raw_points.len() > target_points {
                uniform_downsample(&raw_points, target_points)
            } else {
                raw_points
            };

            // 非浮点类型使用阶梯渲染（数字信号），
            // 在跳变点插入垂直阶梯，使方波正确呈现
            let final_points = match vt {
                ValueType::Float => downsampled,
                _ => insert_step_points(&downsampled),
            };

            // 转换为 PlotPoint 以支持零拷贝借用
            self.cached_points[ch_idx] = final_points
                .into_iter()
                .map(PlotPoint::from)
                .collect();
        }

        // 更新 Y 轴绝对值最大值（避免为 0 导致范围无效）
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
}
