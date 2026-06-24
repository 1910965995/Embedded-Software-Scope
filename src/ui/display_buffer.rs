use crate::pipeline::sample::Sample;

/// 显示专用线性缓冲区
///
/// 与 RingBuffer（环形、覆盖旧数据）不同，DisplayBuffer 是有限增长的线性缓冲。
/// UI 线程每帧从 RingBuffer 消费数据追加到此缓冲区，支持回看历史波形。
/// max_samples 限制内存使用（默认 10 秒 @ 20kHz = 200,000）。
pub struct DisplayBuffer {
    samples: Vec<Sample>,
    max_samples: usize,
    /// 总写入数（包括已丢弃的）
    total_pushed: u64,
    /// 已丢弃的序号偏移（最旧可用的 seq）
    offset: u64,
}

impl DisplayBuffer {
    /// 创建容量为 max_samples 的显示缓冲区
    pub fn new(max_samples: usize) -> Self {
        Self {
            samples: Vec::with_capacity(max_samples),
            max_samples,
            total_pushed: 0,
            offset: 0,
        }
    }

    /// 追加一批采样点（从 RingBuffer 消费后调用）
    ///
    /// 如果超过 max_samples，自动丢弃最旧数据，更新 offset。
    pub fn push_batch(&mut self, batch: &[Sample]) {
        self.samples.extend(batch.iter().cloned());
        self.total_pushed += batch.len() as u64;

        // 一次性丢弃超出的旧数据（比逐个 remove(0) 高效得多）
        let excess = self.samples.len().saturating_sub(self.max_samples);
        if excess > 0 {
            self.samples.drain(..excess);
            self.offset += excess as u64;
        }
    }

    /// 获取指定范围的采样点切片
    ///
    /// 返回切片。start/end 是全局序号（从 0 开始计数的 total_pushed 索引）。
    pub fn slice(&self, start: u64, end: u64) -> &[Sample] {
        if start >= self.offset + self.samples.len() as u64 || end <= self.offset {
            return &[];
        }
        let local_start = if start > self.offset {
            (start - self.offset) as usize
        } else {
            0
        };
        let local_end = if end > self.offset {
            ((end - self.offset) as usize).min(self.samples.len())
        } else {
            return &[];
        };
        if local_start >= local_end {
            return &[];
        }
        &self.samples[local_start..local_end]
    }

    /// 获取全部采样点切片
    pub fn all(&self) -> &[Sample] {
        &self.samples
    }

    /// 当前缓冲区中的采样点数
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// 缓冲区是否为空
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 最新采样点的全局序号（下一个 push 将使用的序号）
    pub fn next_seq(&self) -> u64 {
        self.offset + self.samples.len() as u64
    }

    /// 最旧可用采样点的全局序号
    pub fn oldest_seq(&self) -> u64 {
        self.offset
    }
}
