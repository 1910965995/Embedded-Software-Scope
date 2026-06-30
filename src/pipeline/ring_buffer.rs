use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use super::sample::Sample;

/// Lock-Free SPSC 环形缓冲区
///
/// 单生产者（接收线程写入）、单消费者（主线程读出）。
/// 用 UnsafeCell 实现内部可变性，配合 AtomicUsize 保证无锁并发安全。
///
/// # Safety
/// - push() 仅由接收线程调用，通过 head 原子变量追踪写位置
/// - pop_batch() 仅由主线程调用，通过 tail 原子变量追踪读位置
/// - head 和 tail 的 Release/Acquire 内存序保证数据可见性
pub struct RingBuffer {
    /// UnsafeCell 提供内部可变性：push 需要写入，pop_batch 需要读取
    data: UnsafeCell<Vec<Option<Sample>>>,
    capacity: usize,
    /// 写指针 —— 仅接收线程写入（Release）
    head: AtomicUsize,
    /// 读指针 —— 仅主线程写入（Release）
    tail: AtomicUsize,
}

impl RingBuffer {
    /// 创建容量为 `capacity` 的环形缓冲区
    pub fn new(capacity: usize) -> Self {
        let mut data = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            data.push(None);
        }
        Self {
            data: UnsafeCell::new(data),
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// 写入一个采样点（接收线程调用）
    ///
    /// 永远不会阻塞。如果缓冲区满（消费者跟不上），**丢弃新数据**。
    ///
    /// # Safety
    /// 保持 SPSC 模型不变量 `head - tail <= capacity`：
    /// - 满时（`head - tail == capacity`）不写入，避免与消费者读取的
    ///   `tail % capacity` 槽位发生重叠（否则会触发 `Option<Sample>` 的
    ///   torn read / use-after-free，因为 `Option<Vec<u32>>` 赋值非原子）。
    pub fn push(&self, sample: Sample) {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        // 已写入但未读取的数量 = head - tail
        let in_use = h.wrapping_sub(t);
        if in_use >= self.capacity {
            // 缓冲区满：丢弃新数据，保持不变量 head - tail <= capacity
            // （消费者跟不上时主动丢点优于数据竞争导致崩溃）
            return;
        }
        let idx = h % self.capacity;
        // SAFETY: 只有接收线程调用 push，写入位置 [tail, head) 与
        // pop_batch 的读取范围 [tail, head) 由 head/tail 原子变量协调
        unsafe {
            let data = &mut *self.data.get();
            data[idx] = Some(sample);
        }
        self.head.store(h.wrapping_add(1), Ordering::Release);
    }

    /// 批量读取采样点（主线程调用）
    ///
    /// 返回实际读取的数量（≤ buf.len()）。
    /// 读取后 tail 指针前进，已读数据不可再读。
    pub fn pop_batch(&self, buf: &mut [Sample]) -> usize {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        let available = h.wrapping_sub(t).min(self.capacity);
        let count = available.min(buf.len());

        // SAFETY: 只有主线程调用 pop_batch，读取范围 tail..head 与 push 不重叠
        let data = unsafe { &*self.data.get() };
        for i in 0..count {
            let idx = (t.wrapping_add(i)) % self.capacity;
            if let Some(ref sample) = data[idx] {
                buf[i] = sample.clone();
            }
        }

        self.tail.store(t.wrapping_add(count), Ordering::Release);
        count
    }

    /// 查询当前缓冲区中的可用采样点数
    pub fn available(&self) -> usize {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        h.wrapping_sub(t).min(self.capacity)
    }

    /// 查询总写入数（即最新的 head 值，对应总采样序号）
    pub fn total_written(&self) -> usize {
        self.head.load(Ordering::Relaxed)
    }
}

// 环形缓冲区需要在线程间共享
// SAFETY: SPSC 模型 + 原子变量协调 = 线程安全
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}
