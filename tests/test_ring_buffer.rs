/// 测试 pipeline::ring_buffer::RingBuffer — SPSC 环形缓冲区
use dap_sampler::pipeline::ring_buffer::RingBuffer;
use dap_sampler::pipeline::sample::Sample;

fn make_sample(seq: u64, val: u32) -> Sample {
    Sample { seq, timestamp_sec: seq as f64 * 0.001, values: vec![val] }
}

// ================================================================
// 初始状态
// ================================================================

#[test]
fn new_buffer_is_empty() {
    let rb = RingBuffer::new(1024);
    assert_eq!(rb.available(), 0);
    assert_eq!(rb.total_written(), 0);
}

#[test]
fn new_buffer_large_capacity() {
    let rb = RingBuffer::new(200_000);
    assert_eq!(rb.available(), 0);
    assert_eq!(rb.total_written(), 0);
}

// ================================================================
// push + pop_batch 基本流程
// ================================================================

#[test]
fn push_then_pop_single() {
    let rb = RingBuffer::new(8);
    rb.push(make_sample(0, 0xAA));

    let mut buf = vec![make_sample(0, 0); 4];
    let n = rb.pop_batch(&mut buf);

    assert_eq!(n, 1);
    assert_eq!(buf[0].seq, 0);
    assert_eq!(buf[0].values[0], 0xAA);
}

#[test]
fn push_multiple_pop_all() {
    let rb = RingBuffer::new(8);
    for i in 0..5 {
        rb.push(make_sample(i, i as u32 * 10));
    }

    let mut buf = vec![make_sample(0, 0); 8];
    let n = rb.pop_batch(&mut buf);

    assert_eq!(n, 5);
    for i in 0..5 {
        assert_eq!(buf[i].seq, i as u64);
        assert_eq!(buf[i].values[0], i as u32 * 10);
    }
}

#[test]
fn pop_empty_returns_zero() {
    let rb = RingBuffer::new(8);
    let mut buf = vec![make_sample(0, 0); 4];
    let n = rb.pop_batch(&mut buf);
    assert_eq!(n, 0);
}

#[test]
fn pop_batch_with_small_buf() {
    let rb = RingBuffer::new(8);
    // 缓冲区容量 8，push 10 个：后 2 个被丢弃（满时丢弃新数据）
    for i in 0..10 {
        rb.push(make_sample(i, i as u32));
    }

    let mut buf = vec![make_sample(0, 0); 3];
    let n = rb.pop_batch(&mut buf);
    assert_eq!(n, 3); // only room for 3
    // Remaining available: 8 written - 3 popped = 5
    assert_eq!(rb.available(), 5);
}

// ================================================================
// 环形覆盖（wraparound）
// ================================================================

#[test]
fn wrap_around_discards_new_when_full() {
    let rb = RingBuffer::new(4); // tiny capacity

    // Fill buffer: seq 0,1,2,3
    for i in 0..4 {
        rb.push(make_sample(i, i as u32));
    }

    // Buffer is full. Push two more — 新数据被丢弃（保持 head-tail 不变量）
    rb.push(make_sample(4, 100));
    rb.push(make_sample(5, 200));

    // available() should be capped at capacity
    assert_eq!(rb.available(), 4);

    let mut buf = vec![make_sample(0, 0); 8];
    let n = rb.pop_batch(&mut buf);

    // 保留的是最早写入的 4 个 (seq 0,1,2,3)，新数据 seq 4,5 已被丢弃
    assert_eq!(n, 4);
    let mut seqs: Vec<u64> = buf[..n].iter().map(|s| s.seq).collect();
    seqs.sort();
    assert_eq!(seqs, vec![0, 1, 2, 3]);
}

#[test]
fn available_after_wrap() {
    let rb = RingBuffer::new(4);
    for i in 0..6 {
        rb.push(make_sample(i, i as u32));
    }
    // 满 4 后丢弃新数据：head=4, tail=0, available=min(4, capacity)=4
    assert_eq!(rb.available(), 4);
}

// ================================================================
// available / total_written
// ================================================================

#[test]
fn total_written_tracks_successful_pushes() {
    let rb = RingBuffer::new(16);
    // 容量 16，push 42 个：后 26 个被丢弃，total_written 只计成功写入
    for i in 0..42 {
        rb.push(make_sample(i, i as u32));
    }
    assert_eq!(rb.total_written(), 16);
}

#[test]
fn available_decreases_after_pop() {
    let rb = RingBuffer::new(16);
    for i in 0..8 {
        rb.push(make_sample(i, i as u32));
    }
    assert_eq!(rb.available(), 8);

    let mut buf = vec![make_sample(0, 0); 4];
    let n = rb.pop_batch(&mut buf);
    assert_eq!(n, 4);
    assert_eq!(rb.available(), 4);

    let n = rb.pop_batch(&mut buf);
    assert_eq!(n, 4);
    assert_eq!(rb.available(), 0);
}

// ================================================================
// 带间隙的 push/pop
// ================================================================

#[test]
fn alternating_push_pop() {
    let rb = RingBuffer::new(32);

    // Push 5, pop 3 using small buffer
    for i in 0..5 {
        rb.push(make_sample(i, i as u32));
    }
    let mut small_buf = vec![make_sample(0, 0); 3];
    let n = rb.pop_batch(&mut small_buf);
    assert_eq!(n, 3);
    assert_eq!(small_buf[0].seq, 0);
    assert_eq!(small_buf[1].seq, 1);
    assert_eq!(small_buf[2].seq, 2);
    assert_eq!(rb.available(), 2);

    // Push 5 more, pop all (2 + 5 = 7)
    for i in 5..10 {
        rb.push(make_sample(i, i as u32));
    }
    let mut buf = vec![make_sample(0, 0); 8];
    let n = rb.pop_batch(&mut buf);
    assert_eq!(n, 7); // 2 remaining + 5 new
    assert_eq!(rb.available(), 0);
    // Last popped should be seq 9
    let seqs: Vec<u64> = buf[..n].iter().map(|s| s.seq).collect();
    assert_eq!(seqs, vec![3, 4, 5, 6, 7, 8, 9]);
}

// ================================================================
// 大数据量
// ================================================================

#[test]
fn stress_large_volume() {
    let rb = RingBuffer::new(1024);
    for i in 0..10_000u64 {
        rb.push(make_sample(i, i as u32));
    }
    // 满时丢弃新数据：只有前 1024 个成功写入，后 8976 个被丢弃
    assert_eq!(rb.available(), 1024);
    assert_eq!(rb.total_written(), 1024);

    let mut buf = vec![make_sample(0, 0); 1024];
    let n = rb.pop_batch(&mut buf);
    assert_eq!(n, 1024);

    // 保留的是最早写入的 [0, 1024) 区间
    let mut found: Vec<u32> = buf[..n].iter().map(|s| s.values[0]).collect();
    found.sort();
    let expected: Vec<u32> = (0u32..1024).collect();
    assert_eq!(found, expected);
}

// ================================================================
// 多变量 Sample
// ================================================================

#[test]
fn multi_value_sample_roundtrip() {
    let rb = RingBuffer::new(8);
    let sample = Sample { seq: 42, timestamp_sec: 0.042, values: vec![0x11111111, 0x22222222, 0x33333333, 0x44444444] };
    rb.push(sample);

    let mut buf = vec![make_sample(0, 0); 4];
    let n = rb.pop_batch(&mut buf);
    assert_eq!(n, 1);
    assert_eq!(buf[0].seq, 42);
    assert_eq!(buf[0].values.len(), 4);
    assert_eq!(buf[0].values, vec![0x11111111, 0x22222222, 0x33333333, 0x44444444]);
}
