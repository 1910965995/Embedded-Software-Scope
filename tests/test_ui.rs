/// UI 逻辑单元测试: DisplayBuffer、光标计算、类型系统
use dap_sampler::ui::display_buffer::DisplayBuffer;
use dap_sampler::ui::cursor::CursorState;
use dap_sampler::pipeline::sample::{Sample, ValueType};

// ============================================================
// DisplayBuffer 测试
// ============================================================

fn make_sample(seq: u64, val: u32) -> Sample {
    Sample { seq, timestamp_sec: seq as f64 * 0.001, values: vec![val] }
}

#[test]
fn display_buffer_push_and_slice() {
    let mut buf = DisplayBuffer::new(1000);
    let samples: Vec<Sample> = (0..100).map(|i| make_sample(i, i as u32)).collect();
    buf.push_batch(&samples);

    assert_eq!(buf.len(), 100);
    assert!(!buf.is_empty());

    // 切片
    let slice = buf.slice(50, 60);
    assert_eq!(slice.len(), 10);
    assert_eq!(slice[0].seq, 50);
    assert_eq!(slice[9].seq, 59);
}

#[test]
fn display_buffer_overflow_drops_oldest() {
    let mut buf = DisplayBuffer::new(50);
    let samples: Vec<Sample> = (0..100).map(|i| make_sample(i, i as u32)).collect();
    buf.push_batch(&samples);

    // 只保留最近 50 个
    assert_eq!(buf.len(), 50);
    assert_eq!(buf.oldest_seq(), 50); // 前 50 个被丢弃
    assert_eq!(buf.next_seq(), 100);
}

#[test]
fn display_buffer_empty() {
    let buf = DisplayBuffer::new(100);
    assert!(buf.is_empty());
    assert_eq!(buf.len(), 0);
    assert_eq!(buf.slice(0, 10).len(), 0);
}

#[test]
fn display_buffer_slice_out_of_range() {
    let mut buf = DisplayBuffer::new(100);
    let samples: Vec<Sample> = (0..10).map(|i| make_sample(i, i as u32)).collect();
    buf.push_batch(&samples);

    // 请求超出范围的切片
    assert_eq!(buf.slice(100, 110).len(), 0);
    assert_eq!(buf.slice(5, 5).len(), 0); // start == end
}

#[test]
fn display_buffer_all() {
    let mut buf = DisplayBuffer::new(100);
    let samples: Vec<Sample> = (0..10).map(|i| make_sample(i, i as u32)).collect();
    buf.push_batch(&samples);

    let all = buf.all();
    assert_eq!(all.len(), 10);
    assert_eq!(all[0].seq, 0);
    assert_eq!(all[9].seq, 9);
}

#[test]
fn display_buffer_push_batch_exact_overflow() {
    // 测试 drain-before-extend 优化：恰好溢出
    let mut buf = DisplayBuffer::new(10);
    let s1: Vec<Sample> = (0..8).map(|i| make_sample(i, i as u32)).collect();
    buf.push_batch(&s1);
    assert_eq!(buf.len(), 8);

    // 再推 5 个，总共 13，超出 10，应丢弃前 3 个
    let s2: Vec<Sample> = (8..13).map(|i| make_sample(i, i as u32)).collect();
    buf.push_batch(&s2);
    assert_eq!(buf.len(), 10);
    assert_eq!(buf.oldest_seq(), 3);
    assert_eq!(buf.next_seq(), 13);
}

// ============================================================
// 光标计算测试
// ============================================================

/// 创建一个 Sample，值以 f32 bit pattern 存储
fn make_float_sample(seq: u64, val: f32) -> Sample {
    Sample { seq, timestamp_sec: seq as f64 * 0.001, values: vec![val.to_bits()] }
}

#[test]
fn cursor_click_cycle() {
    let mut c = CursorState::new();
    assert!(c.cursor1.is_none());
    assert!(c.cursor2.is_none());

    c.click(100);
    assert_eq!(c.cursor1, Some(100));
    assert!(c.cursor2.is_none());

    c.click(200);
    assert_eq!(c.cursor1, Some(100));
    assert_eq!(c.cursor2, Some(200));

    c.click(300);
    assert_eq!(c.cursor1, Some(300)); // 重置
    assert!(c.cursor2.is_none());

    c.clear();
    assert!(c.cursor1.is_none());
    assert!(c.cursor2.is_none());
}

#[test]
fn cursor_get_result() {
    // 使用 f32 bit pattern 存储值，使 as_f64s_typed(float) 正确还原
    let samples: Vec<Sample> = (0..10).map(|i| make_float_sample(i, i as f32 * 10.0)).collect();
    let types = vec![ValueType::Float];

    let c = CursorState { cursor1: Some(5), cursor2: None };
    let r = c.get_result(&samples, 0, &types).unwrap();
    assert_eq!(r.seq, 5);
    assert!((r.time_sec - 0.005).abs() < 1e-9); // timestamp_sec=0.005
    assert!((r.values[0] - 50.0).abs() < 0.1);   // 5 * 10.0 = 50.0

    // 超出范围
    let c2 = CursorState { cursor1: Some(100), cursor2: None };
    assert!(c2.get_result(&samples, 0, &types).is_none());
}

#[test]
fn cursor_delta() {
    let samples: Vec<Sample> = (0..10).map(|i| make_float_sample(i, i as f32 * 10.0)).collect();
    let types = vec![ValueType::Float];

    let c = CursorState { cursor1: Some(2), cursor2: Some(8) };
    let (dt, dv) = c.delta(&samples, 0, &types).unwrap();
    assert!((dt - 0.006).abs() < 1e-9); // 0.008 - 0.002 = 0.006
    assert!((dv[0] - 60.0).abs() < 0.1);  // 80-20=60
}

#[test]
fn cursor_get_result_with_offset() {
    // 测试带 offset 的情况（DisplayBuffer 丢弃了旧数据）
    let samples: Vec<Sample> = (0..10).map(|i| make_float_sample(i + 50, i as f32 * 10.0)).collect();
    let types = vec![ValueType::Float];

    // buffer_offset = 50
    let c = CursorState { cursor1: Some(55), cursor2: None };
    let r = c.get_result(&samples, 50, &types).unwrap();
    assert_eq!(r.seq, 55);
    assert!((r.time_sec - 0.055).abs() < 1e-9); // timestamp_sec=0.055
    assert!((r.values[0] - 50.0).abs() < 0.1);
}

// ============================================================
// 类型系统测试
// ============================================================

#[test]
fn value_type_parse() {
    assert_eq!(ValueType::parse("float").unwrap(), ValueType::Float);
    assert_eq!(ValueType::parse("FLOAT").unwrap(), ValueType::Float);
    assert_eq!(ValueType::parse("f32").unwrap(), ValueType::Float);
    assert_eq!(ValueType::parse("int32").unwrap(), ValueType::Int32);
    assert_eq!(ValueType::parse("uint32").unwrap(), ValueType::Uint32);
    assert_eq!(ValueType::parse("i16").unwrap(), ValueType::Int16);
    assert_eq!(ValueType::parse("u8").unwrap(), ValueType::Uint8);
    assert!(ValueType::parse("unknown").is_err());
}

#[test]
fn sample_as_f64s_typed_float() {
    let s = make_float_sample(0, 3.14);
    let types = vec![ValueType::Float];
    let result = s.as_f64s_typed(&types);
    assert!((result[0] - 3.14).abs() < 0.001);
}

#[test]
fn sample_as_f64s_typed_uint32() {
    // u32 值 42 应直接解释为 42.0
    let s = Sample { seq: 0, timestamp_sec: 0.0, values: vec![42u32] };
    let types = vec![ValueType::Uint32];
    let result = s.as_f64s_typed(&types);
    assert!((result[0] - 42.0).abs() < 0.001);
}

#[test]
fn sample_as_f64s_typed_int32() {
    // int32 负数: 0xFFFFFFFF 应解释为 -1.0
    let s = Sample { seq: 0, timestamp_sec: 0.0, values: vec![0xFFFFFFFF] };
    let types = vec![ValueType::Int32];
    let result = s.as_f64s_typed(&types);
    assert!((result[0] - (-1.0)).abs() < 0.001);
}

#[test]
fn sample_as_f64s_typed_int16() {
    // int16: 0xFFFF0000 的低 16 位 = 0x0000 = 0
    // int16: 0x0000FFFF 的低 16 位 = 0xFFFF = -1 (as i16)
    let s = Sample { seq: 0, timestamp_sec: 0.0, values: vec![0x0000FFFF] };
    let types = vec![ValueType::Int16];
    let result = s.as_f64s_typed(&types);
    assert!((result[0] - (-1.0)).abs() < 0.001);
}

#[test]
fn sample_as_f64s_typed_mixed() {
    // 混合类型：float + uint32
    let s = Sample { seq: 0, timestamp_sec: 0.0, values: vec![3.14f32.to_bits(), 100u32] };
    let types = vec![ValueType::Float, ValueType::Uint32];
    let result = s.as_f64s_typed(&types);
    assert!((result[0] - 3.14).abs() < 0.001);
    assert!((result[1] - 100.0).abs() < 0.001);
}

#[test]
fn sample_as_f64s_typed_default_float() {
    // 类型列表短于 values 时，缺失位置默认 float
    let s = Sample { seq: 0, timestamp_sec: 0.0, values: vec![3.14f32.to_bits(), 100u32] };
    let types = vec![ValueType::Float]; // 只有 1 个类型，但 values 有 2 个
    let result = s.as_f64s_typed(&types);
    assert!((result[0] - 3.14).abs() < 0.001);
    // 第二个值默认用 float 解释（100 as f32 bits = 1.4e-43，不是 100.0）
    assert!(result[1] < 1.0); // 不等于 100.0
}
