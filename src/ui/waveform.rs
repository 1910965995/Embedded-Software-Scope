use egui_plot::{Line, Legend, Plot, PlotPoint};
use crate::pipeline::sample::{Sample, ValueType};

/// LTTB (Largest Triangle Three Buckets) 降采样
///
/// 将 N 个数据点降采样到 M 个点（M < N），同时保留波形的峰谷特征。
/// 适用于将 200K 采样点压缩到屏幕宽度（~1920px × 3 = 5760 点）。
///
/// 算法：
/// 1. 将数据分成 M-2 个桶（首尾点保留）
/// 2. 每个桶选一个点，使它与前一个选中点和下一个桶的平均点构成最大三角形面积
///
/// 参数:
/// - `data`: 原始数据切片 [x, y]
/// - `threshold`: 目标点数
pub fn lttb_downsample(data: &[[f64; 2]], threshold: usize) -> Vec<[f64; 2]> {
    let n = data.len();
    if n <= threshold || threshold < 3 {
        return data.to_vec();
    }

    let mut result = Vec::with_capacity(threshold);
    result.push(data[0]); // 首点保留

    // 使用整数运算计算桶边界，避免浮点精度问题
    let denom = threshold - 2;
    let range = n - 2;
    let mut prev_selected = 0usize;

    for i in 0..(threshold - 2) {
        // 当前桶范围 [bucket_start, bucket_end)
        let bucket_start = i * range / denom + 1;
        let bucket_end = (((i + 1) * range / denom) + 1).min(n - 1);
        let bucket_end = bucket_end.max(bucket_start + 1);

        // 下一个桶范围（用于计算平均点）
        let next_start = bucket_end;
        let next_end = (((i + 2) * range / denom) + 1).min(n);

        // 下一个桶的平均点
        let next_len = (next_end - next_start).max(1);
        let next_avg_x: f64 = data[next_start..next_end]
            .iter()
            .map(|p| p[0])
            .sum::<f64>()
            / next_len as f64;
        let next_avg_y: f64 = data[next_start..next_end]
            .iter()
            .map(|p| p[1])
            .sum::<f64>()
            / next_len as f64;

        // 在当前桶中选三角形面积最大的点
        let mut max_area = -1.0f64;
        let mut max_idx = bucket_start;

        for j in bucket_start..bucket_end {
            let area = triangle_area(
                data[prev_selected][0], data[prev_selected][1],
                data[j][0], data[j][1],
                next_avg_x, next_avg_y,
            );
            if area > max_area {
                max_area = area;
                max_idx = j;
            }
        }

        result.push(data[max_idx]);
        prev_selected = max_idx;
    }

    result.push(data[n - 1]); // 尾点保留
    result
}

/// 计算三点构成的三角形面积（绝对值）
fn triangle_area(x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) -> f64 {
    ((x1 - x3) * (y2 - y1) - (x1 - x2) * (y3 - y1)).abs()
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
    /// 是否需要自动调整 Y 轴范围
    auto_fit_y: bool,
}

impl WaveformPanel {
    pub fn new(
        channel_names: Vec<String>,
        channel_colors: Vec<egui::Color32>,
        interval_us: f64,
        value_types: Vec<ValueType>,
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
            auto_fit_y: false,
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
        visible_width: f32,
        has_new_data: bool,
    ) -> Option<u64> {
        if buffer.is_empty() {
            ui.label("Waiting for data...");
            return None;
        }

        let target_points = (visible_width as usize * 3).min(100_000); // 3 倍像素宽度

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

        let mut clicked_seq: Option<u64> = None;
        let auto_fit = self.auto_fit_y;
        self.auto_fit_y = false; // 一次性触发

        let mut plot = Plot::new("waveform_plot")
            .legend(Legend::default())
            .show_grid(true)
            .show_axes(true)
            .x_axis_label(x_label)
            .y_axis_label(y_label)
            .allow_zoom(true)
            .allow_drag(true)
            .allow_scroll(true);

        if auto_fit {
            plot = plot.auto_bounds(egui::Vec2b::new(false, true));
        }

        let plot_response = plot.show(ui, |plot_ui| {
            // 捕获鼠标坐标（用于光标点击）
            let pointer_coord = plot_ui.pointer_coordinate();

            // 记录可视范围（用于下一帧的 X 轴标签）
            let bounds = plot_ui.plot_bounds();
            self.last_x_range = Some(bounds.max()[0] - bounds.min()[0]);

            // 渲染各通道波形（零拷贝借用缓存数据）
            for (ch_idx, ch) in self.channels.iter().enumerate() {
                if !ch.visible {
                    continue;
                }
                let points = &self.cached_points[ch_idx];
                if points.is_empty() {
                    continue;
                }
                plot_ui.line(
                    Line::new(points.as_slice())
                        .name(&ch.name)
                        .color(ch.color)
                        .width(1.5),
                );
            }

            // 检测点击 → 放置光标
            if plot_ui.response().clicked() {
                if let Some(coord) = pointer_coord {
                    // 从 X 坐标（秒）反推采样序号
                    let seq = (coord.x * 1_000_000.0 / self.interval_us).round() as u64;
                    clicked_seq = Some(seq);
                }
            }
        });

        let _ = plot_response;
        clicked_seq
    }

    /// 重建降采样缓存
    fn rebuild_cache(&mut self, buffer: &[Sample], target_points: usize) {
        for (ch_idx, ch) in self.channels.iter().enumerate() {
            if !ch.visible {
                self.cached_points[ch_idx].clear();
                continue;
            }

            let vt = self.value_types.get(ch_idx).copied().unwrap_or(ValueType::Float);

            // 构建 [x, y] 数据点序列
            let raw_points: Vec<[f64; 2]> = buffer
                .iter()
                .filter_map(|s| {
                    let raw = s.values.get(ch_idx).copied().unwrap_or(0);
                    let val = vt.to_f64(raw);
                    if val.is_finite() {
                        Some([s.timestamp_sec(self.interval_us), val])
                    } else {
                        None
                    }
                })
                .collect();

            // 降采样
            let downsampled = if raw_points.len() > target_points {
                lttb_downsample(&raw_points, target_points)
            } else {
                raw_points
            };

            // 转换为 PlotPoint 以支持零拷贝借用
            self.cached_points[ch_idx] = downsampled
                .into_iter()
                .map(PlotPoint::from)
                .collect();
        }
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
}
