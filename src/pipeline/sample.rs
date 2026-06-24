/// 变量值类型
///
/// 决定如何将 u32 原始值解释为可显示的 f64 数值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Float,
    Int32,
    Uint32,
    Int16,
    Uint16,
    Int8,
    Uint8,
}

impl ValueType {
    /// 从字符串解析类型名称
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "float" | "f32" => Ok(ValueType::Float),
            "int32" | "i32" => Ok(ValueType::Int32),
            "uint32" | "u32" => Ok(ValueType::Uint32),
            "int16" | "i16" => Ok(ValueType::Int16),
            "uint16" | "u16" => Ok(ValueType::Uint16),
            "int8" | "i8" => Ok(ValueType::Int8),
            "uint8" | "u8" => Ok(ValueType::Uint8),
            _ => Err(format!("未知类型: {}（支持: float, int32, uint32, int16, uint16, int8, uint8）", s)),
        }
    }

    /// 返回类型的显示标签
    pub fn label(&self) -> &'static str {
        match self {
            ValueType::Float => "float",
            ValueType::Int32 => "int32",
            ValueType::Uint32 => "uint32",
            ValueType::Int16 => "int16",
            ValueType::Uint16 => "uint16",
            ValueType::Int8 => "int8",
            ValueType::Uint8 => "uint8",
        }
    }

    /// 将 u32 原始值按本类型转换为 f64
    pub fn to_f64(&self, raw: u32) -> f64 {
        match self {
            ValueType::Float => f64::from(f32::from_bits(raw)),
            ValueType::Int32 => raw as i32 as f64,
            ValueType::Uint32 => raw as f64,
            ValueType::Int16 => (raw as i16) as f64,
            ValueType::Uint16 => (raw as u16) as f64,
            ValueType::Int8 => (raw as i8) as f64,
            ValueType::Uint8 => (raw as u8) as f64,
        }
    }
}

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

    /// 根据类型列表将 u32 值转换为 f64 数组
    ///
    /// 支持不同变量使用不同类型解释（如 float、int32、uint16 等）。
    /// 若类型列表短于 values，缺失位置默认 float。
    pub fn as_f64s_typed(&self, types: &[ValueType]) -> Vec<f64> {
        self.values.iter().enumerate().map(|(i, &v)| {
            let t = types.get(i).copied().unwrap_or(ValueType::Float);
            t.to_f64(v)
        }).collect()
    }
}
