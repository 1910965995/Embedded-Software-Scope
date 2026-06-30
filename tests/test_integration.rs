/// 集成测试 — CLI 参数解析、边界条件、端到端数据流
///
/// 注意：这些测试不依赖 USB 硬件，仅验证逻辑层的正确性。
use dap_sampler::dap::commands::*;
use dap_sampler::dap::protocol::DapProtocol;
use dap_sampler::pipeline::sample::Sample;
use dap_sampler::pipeline::ring_buffer::RingBuffer;

// ================================================================
// 端到端模拟：从请求构建到响应解析的完整数据流
// ================================================================

#[test]
fn e2e_single_variable_read_flow() {
    // 模拟: 读地址 0x20000100
    // Step 1: 构建 DAP_Transfer 请求（write TAR + read DRW）
    let requests = vec![
        TransferRequest::write_ap(AP_REG_TAR, 0x20000100),
        TransferRequest::read_ap(AP_REG_DRW),
    ];

    let dap = DapProtocol { dap_index: 0 };
    let cmd = dap.build_transfer_request(&requests);

    // 验证命令
    assert_eq!(cmd[0], DAP_TRANSFER);
    assert_eq!(cmd[2], 2);

    // Step 2: 模拟 DAP-Link 响应（读取到值 0x40490FDB = 3.1416）
    let fake_response: [u8; 7] = [
        0x05, // cmd_echo
        0x02, // count=2
        0x01, // TRANSFER_OK
        0xDB, 0x0F, 0x49, 0x40, // 0x40490FDB in LE
    ];

    let resp = DapProtocol::parse_transfer_response(&fake_response).unwrap();
    assert_eq!(resp.status, TRANSFER_OK);
    assert_eq!(resp.data.len(), 1);
    assert_eq!(resp.data[0], 0x40490FDB);

    // Step 3: 包装为 Sample
    let sample = Sample { seq: 0, timestamp_sec: 0.0, values: resp.data };
    assert_eq!(sample.values[0], 0x40490FDB);
    let floats = sample.as_floats();
    assert!((floats[0] - 3.1415927).abs() < 1e-6);

    // Step 4: 时间戳验证
    assert!((sample.timestamp_sec - 0.0).abs() < 1e-9);
}

#[test]
fn e2e_multi_variable_read_flow() {
    // 模拟: 读 4 个地址
    let addrs = [0x20000100u32, 0x20000104, 0x20000108, 0x2000010c];
    let requests: Vec<TransferRequest> = addrs
        .iter()
        .flat_map(|&a| vec![
            TransferRequest::write_ap(AP_REG_TAR, a),
            TransferRequest::read_ap(AP_REG_DRW),
        ])
        .collect();

    let dap = DapProtocol { dap_index: 0 };
    let cmd = dap.build_transfer_request(&requests);
    assert_eq!(cmd[2], 8); // 8 requests (4 write + 4 read)

    // 模拟响应：返回 4 个 u32 值
    let mut fake_resp = vec![0x05u8, 0x08, 0x01]; // cmd_echo, count=8, OK
    let values: [u32; 4] = [0x11111111, 0x22222222, 0x33333333, 0x44444444];
    for v in &values {
        fake_resp.extend_from_slice(&v.to_le_bytes());
    }

    let resp = DapProtocol::parse_transfer_response(&fake_resp).unwrap();
    assert_eq!(resp.data.len(), 4);
    assert_eq!(resp.data, values.to_vec());

    let sample = Sample { seq: 5, timestamp_sec: 0.00025, values: resp.data };
    // 验证时间戳字段
    assert!((sample.timestamp_sec - 0.00025).abs() < 1e-9);
}

// ================================================================
// 环形缓冲区 + 多变量数据流
// ================================================================

#[test]
fn e2e_ring_buffer_pipeline_simulation() {
    let rb = RingBuffer::new(1024);

    // 模拟接收线程：推送 100 个采样点（每个 4 变量）
    for seq in 0..100u64 {
        let sample = Sample {
            seq,
            timestamp_sec: seq as f64 * 0.00005,
            values: vec![
                seq as u32 * 10,
                seq as u32 * 20,
                seq as u32 * 30,
                seq as u32 * 40,
            ],
        };
        rb.push(sample);
    }

    // 模拟主线程：批量消费
    let mut buf = vec![Sample { seq: 0, timestamp_sec: 0.0, values: vec![] }; 50];
    let n = rb.pop_batch(&mut buf);
    assert_eq!(n, 50);
    assert_eq!(buf[0].seq, 0);
    assert_eq!(buf[49].seq, 49);

    // 消费剩余
    let n2 = rb.pop_batch(&mut buf);
    assert_eq!(n2, 50);
    assert_eq!(buf[0].seq, 50);
    assert_eq!(buf[49].seq, 99);

    // 缓冲区应空
    assert_eq!(rb.available(), 0);
}

// ================================================================
// 边界条件：错误响应处理
// ================================================================

#[test]
fn e2e_handles_transfer_fault() {
    // 模拟 TRANSFER_FAULT 响应
    let fault_resp = [0x05, 0x02, TRANSFER_FAULT, 0x00, 0x00, 0x00, 0x00];
    let resp = DapProtocol::parse_transfer_response(&fault_resp).unwrap();
    assert_eq!(resp.status, TRANSFER_FAULT);
    // 消费者应跳过此响应，不创建 Sample
}

#[test]
fn e2e_data_length_mismatch() {
    // 响应中数据量与预期不符
    // 期望 2 个值，但响应只包含 1 个
    let short_resp = [0x05, 0x04, 0x01, 0xAA, 0xAA, 0xAA, 0xAA]; // only 1 value
    let resp = DapProtocol::parse_transfer_response(&short_resp).unwrap();
    assert_eq!(resp.data.len(), 1); // parse_transfer_response 不关心期望数量
}

// ================================================================
// 时间戳单调性
// ================================================================

#[test]
fn timestamp_monotonic() {
    let mut prev = -1.0;
    for seq in 0..10000u64 {
        let sample = Sample { seq, timestamp_sec: seq as f64 * 0.00005, values: vec![] };
        let ts = sample.timestamp_sec;
        assert!(ts > prev, "Timestamp not monotonic at seq={}", seq);
        prev = ts;
    }
}

// ================================================================
// SWD 请求字节不变性（regression test for Keil-verified encoding）
// ================================================================

#[test]
fn regression_keil_verified_encoding() {
    // 这些值经过 Keil USB 抓包验证（P1 阶段修正）
    // 读 DPIDR: RnW=1, APnDP=0, A2=0, A3=0 → 0x02
    assert_eq!(req_read_dp(DP_REG_DPIDR), 0x02);
    // 写 CTRL/STAT: RnW=0, APnDP=0, A2=1, A3=0 → 0x04
    assert_eq!(req_write_dp(DP_REG_CTRL_STAT), 0x04);
    // 写 SELECT: RnW=0, APnDP=0, A2=0, A3=1 → 0x08
    assert_eq!(req_write_dp(DP_REG_SELECT), 0x08);
    // 写 AP TAR: RnW=0, APnDP=1, A2=1, A3=0 → 0x05
    assert_eq!(req_write_ap(AP_REG_TAR), 0x05);
    // 读 AP DRW: RnW=1, APnDP=1, A2=1, A3=1 → 0x0F
    assert_eq!(req_read_ap(AP_REG_DRW), 0x0F);
}

// ================================================================
// Transfer 响应格式（CMSIS-DAP v2 标准）
// ================================================================

#[test]
fn regression_transfer_ok_is_0x01() {
    // P1 阶段修正：TRANSFER_OK = 0x01，不是 0x00
    assert_eq!(TRANSFER_OK, 0x01);
}

#[test]
fn regression_ctrl_stat_powerup_bits() {
    // P1 阶段修正：CSYSPWRUPREQ 在 bit 30，CDBGPWRUPREQ 在 bit 28
    assert_eq!(CSYSPWRUPREQ, 1 << 30);
    assert_eq!(CDBGPWRUPREQ, 1 << 28);
    assert_eq!(CSYSPWRUPACK, 1 << 31);
    assert_eq!(CDBGPWRUPACK, 1 << 29);
}

// ================================================================
// 压力测试：大容量环形缓冲区
// ================================================================

#[test]
fn stress_ring_buffer_200k() {
    let rb = RingBuffer::new(200_000); // P2 design spec: 10s @ 20kHz

    // Fill beyond capacity
    for i in 0..300_000u64 {
        rb.push(Sample { seq: i, timestamp_sec: i as f64 * 0.00005, values: vec![i as u32] });
    }

    // 当前行为（commit b0d76c0 后）：缓冲区满时丢弃新数据，保持 head - tail <= capacity
    // 前 200_000 个样本成功写入，后 100_000 个被丢弃
    assert_eq!(rb.available(), 200_000);
    assert_eq!(rb.total_written(), 200_000);

    // Pop all available
    let mut buf = vec![Sample { seq: 0, timestamp_sec: 0.0, values: vec![] }; 200_000];
    let n = rb.pop_batch(&mut buf);
    assert_eq!(n, 200_000);

    // 保留的是最早写入的 [0, 200_000) 区间（新数据被丢弃）
    for sample in &buf[..n] {
        assert!(sample.values[0] < 200_000, "value {} too new (should have been discarded)", sample.values[0]);
    }

    // Should contain exactly the expected set of values [0, 200_000)
    let mut found: Vec<u32> = buf[..n].iter().map(|s| s.values[0]).collect();
    found.sort();
    let expected: Vec<u32> = (0u32..200_000).collect();
    assert_eq!(found, expected);
}
