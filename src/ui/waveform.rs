use egui_plot::{Line, Legend, Plot};
use crate::pipeline::sample::Sample;

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

    let bucket_size = (n - 2) as f64 / (threshold - 2) as f64;
    let mut prev_selected = 0usize;

    for i in 0..(threshold - 2) {
        // 当前桶范围 [bucket_start, bucket_end)
        let bucket_start = (i as f64 * bucket_size).floor() as usize + 1;
        let bucket_end = (((i + 1) as f64 * bucket_size).floor() as usize + 1).min(n - 1);
        let bucket_end = bucket_end.max(bucket_start + 1);

        // 下一个桶范围（用于计算平均点）
        let next_start = bucket_end;
        let next_end = (((i + 2) as f64 * bucket_size).floor() as usize + 1).min(n);

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
pub struct WaveformPanel {
    /// 通道配置
    channels: Vec<ChannelInfo>,
    /// 采样间隔（微秒），用于时间戳推算
    interval_us: f64,
}

impl WaveformPanel {
    pub fn new(channel_names: Vec<String>, channel_colors: Vec<egui::Color32>, interval_us: f64) -> Self {
        let channels = channel_names
            .into_iter()
            .zip(channel_colors)
            .map(|(name, color)| ChannelInfo { name, color, visible: true })
            .collect();
        Self { channels, interval_us }
    }

    /// 渲染波形
    ///
    /// `ui` 是 egui 的 Ui 对象，`buffer` 是显示缓冲区，`visible_width` 是 Plot 像素宽度。
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        buffer: &[Sample],
        visible_width: f32,
        _cursors: &super::cursor::CursorState,
    ) {
        if buffer.is_empty() {
            ui.label("Waiting for data...");
            return;
        }

        let target_points = (visible_width as usize * 3).min(100_000); // 3 倍像素宽度

        Plot::new("waveform_plot")
            .legend(Legend::default())
            .x_axis_label("Time (s)")
            .y_axis_label("Value")
            .allow_zoom(true)
            .allow_drag(true)
            .allow_scroll(true)
            .show(ui, |plot_ui| {
                for (ch_idx, ch) in self.channels.iter().enumerate() {
                    if !ch.visible {
                        continue;
                    }

                    // 构建 [x, y] 数据点序列
                    let points: Vec<[f64; 2]> = buffer
                        .iter()
                        .filter_map(|s| {
                            let val = s.as_f64s().get(ch_idx).copied().unwrap_or(0.0);
                            if val.is_finite() {
                                Some([s.timestamp_sec(self.interval_us), val])
                            } else {
                                None
                            }
                        })
                        .collect();

                    if points.is_empty() {
                        continue;
                    }

                    // 降采样
                    let display_points = if points.len() > target_points {
                        lttb_downsample(&points, target_points)
                    } else {
                        points
                    };

                    plot_ui.line(
                        Line::new(display_points)
                            .name(&ch.name)
                            .color(ch.color)
                            .width(1.5),
                    );
                }
            });
    }

    /// 切换通道可见性
    pub fn toggle_channel(&mut self, index: usize) {
        if let Some(ch) = self.channels.get_mut(index) {
            ch.visible = !ch.visible;
        }
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn channel_names(&self) -> Vec<&str> {
        self.channels.iter().map(|c| c.name.as_str()).collect()
    }
}
