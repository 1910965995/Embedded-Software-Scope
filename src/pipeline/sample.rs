/// 单次采样点（包含多个变量的读取值）
#[derive(Debug, Clone)]
pub struct Sample {
    /// 全局采样序号（提交线程分配，单调递增）
    pub seq: u64,
    /// 各变量读取值（u32 原始值，由上层按需解释为 float/int）
    pub values: Vec<u32>,
}

impl Sample {
    /// 用序号和提交间隔（微秒）推算时间戳（秒）
    ///
    /// 因为提交线程以固定间隔发送，序号 × 间隔 = 精确时间偏移。
    /// 不依赖系统时钟，无 OS 调度抖动。
    pub fn timestamp_sec(&self, interval_us: f64) -> f64 {
        self.seq as f64 * interval_us / 1_000_000.0
    }

    /// 将原始 u32 值数组解释为 f32 数组
    pub fn as_floats(&self) -> Vec<f32> {
        self.values.iter().map(|&v| f32::from_bits(v)).collect()
    }

    /// 将原始 u32 值数组解释为 f64 数组（egui_plot 使用 f64 坐标）
    ///
    /// 每个 u32 被解释为 f32 的位模式，然后提升为 f64。
    pub fn as_f64s(&self) -> Vec<f64> {
        self.values.iter().map(|&v| f64::from(f32::from_bits(v))).collect()
    }
}
