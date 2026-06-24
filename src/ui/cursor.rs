use crate::pipeline::sample::Sample;

/// 光标测量状态
///
/// 支持单光标和双光标差值测量。
#[derive(Debug, Clone, Default)]
pub struct CursorState {
    /// 第一个光标位置（采样点序号，None = 未放置）
    pub cursor1: Option<u64>,
    /// 第二个光标位置（None = 未放置）
    pub cursor2: Option<u64>,
}

/// 光标测量结果
#[derive(Debug, Clone)]
pub struct CursorResult {
    /// 光标位置（采样点序号）
    pub seq: u64,
    /// 推算时间（秒）
    pub time_sec: f64,
    /// 该点的各通道值
    pub values: Vec<f64>,
}

impl CursorState {
    pub fn new() -> Self {
        Self { cursor1: None, cursor2: None }
    }

    /// 点击放置/移动光标
    ///
    /// 第一次点击 → cursor1；第二次点击 → cursor2；第三次点击 → 重置 cursor1。
    pub fn click(&mut self, seq: u64) {
        if self.cursor1.is_none() {
            self.cursor1 = Some(seq);
            self.cursor2 = None;
        } else if self.cursor2.is_none() {
            self.cursor2 = Some(seq);
        } else {
            self.cursor1 = Some(seq);
            self.cursor2 = None;
        }
    }

    /// 清除所有光标
    pub fn clear(&mut self) {
        self.cursor1 = None;
        self.cursor2 = None;
    }

    /// 获取光标处的采样数据
    ///
    /// `buffer` 是 DisplayBuffer 的切片，`buffer_offset` 是最旧可用序号，`interval_us` 是采样间隔（微秒）。
    pub fn get_result(
        &self,
        buffer: &[Sample],
        buffer_offset: u64,
        interval_us: f64,
    ) -> Option<CursorResult> {
        let seq = self.cursor1?;
        let local_idx = seq.checked_sub(buffer_offset)? as usize;
        let sample = buffer.get(local_idx)?;
        Some(CursorResult {
            seq,
            time_sec: sample.timestamp_sec(interval_us),
            values: sample.as_f64s(),
        })
    }

    /// 获取双光标的差值信息
    ///
    /// 返回 (时间差(秒), 各通道值差)。
    pub fn delta(
        &self,
        buffer: &[Sample],
        buffer_offset: u64,
        interval_us: f64,
    ) -> Option<(f64, Vec<f64>)> {
        let r1 = self.get_result(buffer, buffer_offset, interval_us)?;
        let seq2 = self.cursor2?;
        let local_idx2 = seq2.checked_sub(buffer_offset)? as usize;
        let sample2 = buffer.get(local_idx2)?;
        let r2 = CursorResult {
            seq: seq2,
            time_sec: sample2.timestamp_sec(interval_us),
            values: sample2.as_f64s(),
        };

        let dt = r2.time_sec - r1.time_sec;
        let dv: Vec<f64> = r1.values.iter()
            .zip(r2.values.iter())
            .map(|(a, b)| b - a)
            .collect();

        Some((dt, dv))
    }
}
