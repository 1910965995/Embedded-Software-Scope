use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::*;
use crate::usb::transfer::BulkTransfer;
use crate::dap::protocol::DapProtocol;
use crate::dap::commands::*;
use super::sample::Sample;
use super::ring_buffer::RingBuffer;

/// 单次 DAP_Transfer 命令的最大请求数（保守上限，避免超过 USB 包大小）
///
/// CMSIS-DAP v2 单包请求受 packet_size 限制。每个请求 5 字节
/// （1 字节 request + 4 字节 data），64 个请求 = 320 字节，远小于
/// 典型 packet_size (512/1024)。同时避免 total_requests as u8 截断。
const MAX_TRANSFER_REQUESTS: usize = 64;

/// DAP-Link 在途请求深度上限（流控阈值）
///
/// 限制"已提交但未收到响应"的请求数，防止 DAP-Link 内部队列堆积。
/// 堆积会导致 DAP-Link 快速连续处理多个请求（SWD 读取时刻接近），
/// 但 USB 传输仍按节奏返回响应，接收时刻无法反映真实采样时刻，
/// 在正弦波上产生"平直线伪影"。
///
/// 阈值 2 = 允许 1 个在 USB 传输中 + 1 个在 DAP-Link 处理中。
/// 正常情况下不会触发等待；仅在 DAP-Link 处理慢于提交节奏时生效，
/// 自动降低实际采样率以避免堆积。
const MAX_INFLIGHT: u64 = 2;

/// 流水线采集引擎
///
/// 双线程模型：
/// - 提交线程：以固定间隔构造 DAP_Transfer 命令并非阻塞写入 USB
/// - 接收线程：从 USB 读取响应、解析采样数据、推入环形缓冲区
pub struct PipelineEngine {
    usb: Arc<BulkTransfer>,
    dap: DapProtocol,
    addresses: Vec<u32>,
    /// 目标采样间隔（微秒）
    interval_us: u64,
    /// 停止标志（主线程设为 false 时，子线程退出）
    running: Arc<AtomicBool>,
    /// 写入请求队列（主线程推送，提交线程消费）
    /// 元素: (地址, 数据)
    write_queue: Arc<Mutex<VecDeque<(u32, u32)>>>,
    /// 接收线程已处理的 seq（流控用，提交线程读取以限制在途深度）
    collect_seq: Arc<AtomicU64>,
}

impl PipelineEngine {
    /// 从已初始化的 SWD 链路创建流水线引擎
    ///
    /// `usb` 和 `dap` 来自 `SwdLink::into_parts()`。
    /// `addresses` 是一组目标内存地址（1-8 个）。
    /// `rate_hz` 是目标采样率。
    pub fn new(
        usb: Arc<BulkTransfer>,
        dap: DapProtocol,
        addresses: Vec<u32>,
        rate_hz: u32,
    ) -> Self {
        let interval_us = (1_000_000.0 / rate_hz as f64) as u64;
        Self {
            usb,
            dap,
            addresses,
            interval_us,
            running: Arc::new(AtomicBool::new(true)),
            write_queue: Arc::new(Mutex::new(VecDeque::new())),
            collect_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 使用新采样率重建引擎（保持相同 USB/DAP/地址配置）
    ///
    /// 采样率在 UI 中修改后调用，返回新引擎替换旧的。
    pub fn with_rate(&self, rate_hz: u32) -> Self {
        Self {
            usb: Arc::clone(&self.usb),
            dap: self.dap,
            addresses: self.addresses.clone(),
            interval_us: (1_000_000.0 / rate_hz as f64) as u64,
            running: Arc::new(AtomicBool::new(true)),
            write_queue: Arc::clone(&self.write_queue),
            collect_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 启动流水线，返回控制句柄
    ///
    /// 可重复调用（Stop 后再次 Start），不消费 self。
    pub fn start(&self) -> Result<PipelineHandle> {
        // 重置停止标志（上一次 stop 可能已将其置为 false）
        self.running.store(true, Ordering::SeqCst);
        // 重置接收 seq（流控起点）
        self.collect_seq.store(0, Ordering::SeqCst);

        let ring = Arc::new(RingBuffer::new(200_000)); // 10秒 @ 20kHz
        let start_time = Instant::now();

        // 时间戳由接收线程记录每个响应的到达时刻（Instant::now()）：
        //   timestamp = (now - start_time).as_secs_f64()
        // 这反映真实接收时刻，避免 DAP-Link 批量处理堆积请求时，
        // 多个实际采样时刻接近的响应被赋予相隔 interval 的理想网格
        // 时间戳，从而在正弦波上产生平直线伪影。
        //
        // 配合流控（MAX_INFLIGHT）：提交线程限制在途请求深度，从源头
        // 避免 DAP-Link 队列堆积，使每个请求的 SWD 读取时刻更接近
        // 理想网格，接收时刻也更准确。

        let submit = self.spawn_submit_thread(start_time)?;
        let collect = self.spawn_collect_thread(Arc::clone(&ring), start_time)?;

        Ok(PipelineHandle {
            submit_handle: submit,
            collect_handle: collect,
            ring_buffer: ring,
            running: Arc::clone(&self.running),
            write_queue: Arc::clone(&self.write_queue),
        })
    }

    /// 启动提交线程
    fn spawn_submit_thread(
        &self,
        start_time: Instant,
    ) -> Result<JoinHandle<()>> {
        let usb = Arc::clone(&self.usb);
        let interval_us = self.interval_us;
        let addresses = self.addresses.clone();
        let running = Arc::clone(&self.running);
        let write_queue = Arc::clone(&self.write_queue);
        let collect_seq = Arc::clone(&self.collect_seq);

        // 预构建 TransferRequest 数组（每次采样都一样）
        // 每变量 = 写 TAR + 读 DRW，共 2 条请求
        let requests: Vec<TransferRequest> = addresses
            .iter()
            .flat_map(|&addr| {
                vec![
                    TransferRequest::write_ap(AP_REG_TAR, addr),
                    TransferRequest::read_ap(AP_REG_DRW),
                ]
            })
            .collect();

        let dap_index = self.dap.dap_index;

        let handle = thread::Builder::new()
            .name("dap-submit".into())
            .spawn(move || {
                let mut seq: u64 = 0;
                let start = start_time;

                while running.load(Ordering::Relaxed) {
                    // --- 流控：限制 DAP-Link 在途请求深度 ---
                    // 等待接收线程处理足够多的响应，避免 DAP-Link 内部队列堆积。
                    // 堆积会导致 DAP-Link 快速连续处理多个请求（SWD 读取时刻接近），
                    // 而 USB 传输仍按节奏返回，接收时刻无法反映真实采样时刻，
                    // 在正弦波上产生"平直线伪影"。
                    while running.load(Ordering::Relaxed) {
                        let in_flight = seq.saturating_sub(collect_seq.load(Ordering::Acquire));
                        if in_flight < MAX_INFLIGHT {
                            break;
                        }
                        // 在途深度超限，短暂等待接收线程消费
                        spin_sleep::sleep(Duration::from_micros(10));
                    }
                    if !running.load(Ordering::Relaxed) {
                        break;
                    }

                    // --- 构造 DAP_Transfer 命令 ---
                    // 检查是否有待写入的请求，有则附加到命令前部。
                    // 限制每轮消耗的写入数，确保 total_requests 不超过
                    // MAX_TRANSFER_REQUESTS（避免 u8 截断 + USB 包超限）。
                    let max_writes = (MAX_TRANSFER_REQUESTS.saturating_sub(requests.len())) / 2;
                    let mut pending_writes: Vec<(u32, u32)> = Vec::with_capacity(max_writes);
                    if let Ok(mut wq) = write_queue.lock() {
                        while pending_writes.len() < max_writes {
                            if let Some((addr, data)) = wq.pop_front() {
                                pending_writes.push((addr, data));
                            } else {
                                break;
                            }
                        }
                    }

                    // 构建 request 列表：[写入请求...] + [读取请求...]
                    // 写入: write TAR + write DRW (2 requests, 无返回数据)
                    // 读取: write TAR + read DRW (2 requests, 1 返回数据)
                    let total_requests = requests.len() + pending_writes.len() * 2;
                    // 安全断言：total_requests 必须在 u8 范围内
                    debug_assert!(total_requests <= MAX_TRANSFER_REQUESTS);
                    let mut cmd = vec![DAP_TRANSFER, dap_index, total_requests as u8];

                    // 先写入请求
                    for &(addr, data) in &pending_writes {
                        let w_tar = TransferRequest::write_ap(AP_REG_TAR, addr);
                        let w_drw = TransferRequest::write_ap(AP_REG_DRW, data);
                        cmd.push(w_tar.request_byte());
                        cmd.extend_from_slice(&addr.to_le_bytes());
                        cmd.push(w_drw.request_byte());
                        cmd.extend_from_slice(&data.to_le_bytes());
                    }

                    // 再读取请求
                    for req in &requests {
                        cmd.push(req.request_byte());
                        if !req.rnw {
                            let data = req.write_data.unwrap_or(0);
                            cmd.extend_from_slice(&data.to_le_bytes());
                        }
                    }

                    // --- 发送命令（200ms 超时）---
                    // write_nonblock 使用 200ms 超时，确保停止标志能被及时检查到
                    if !pending_writes.is_empty() {
                        log::info!(
                            "提交线程注入 {} 个写入请求 (seq={})",
                            pending_writes.len(), seq
                        );
                    }
                    if let Err(e) = usb.write_nonblock(&cmd) {
                        if !running.load(Ordering::Relaxed) {
                            // 停止中遇到的超时/错误，正常退出
                            log::info!("提交线程收到停止信号，退出");
                            break;
                        }
                        log::error!("提交线程 USB 写失败 (seq={}): {}", seq, e);
                        break;
                    }

                    seq += 1;

                    // --- 精确等间隔控制 ---
                    // 下一个目标时刻 = start + interval_us * seq
                    let target = start + Duration::from_micros(interval_us * seq);
                    let now = Instant::now();
                    if now < target {
                        // spin_sleep 提供微秒级精度的混合睡眠
                        spin_sleep::sleep(target - now);
                    }
                }

                log::info!("提交线程退出，共提交 {} 个请求", seq);
            })
            .map_err(|e| Error::PipelineThread(e.to_string()))?;

        Ok(handle)
    }

    /// 启动接收线程
    fn spawn_collect_thread(
        &self,
        ring: Arc<RingBuffer>,
        start_time: Instant,
    ) -> Result<JoinHandle<()>> {
        let usb = Arc::clone(&self.usb);
        let running = Arc::clone(&self.running);
        let addresses = self.addresses.clone();
        let num_vars = addresses.len();
        let collect_seq = Arc::clone(&self.collect_seq);

        let handle = thread::Builder::new()
            .name("dap-collect".into())
            .spawn(move || {
                let mut buf = vec![0u8; 4096];
                // carry-over buffer：保存上次读取的不完整响应尾部，
                // 下次读取时拼接到开头再解析，避免解析错位。
                let mut carry: Vec<u8> = Vec::new();
                let mut seq: u64 = 0;

                // 每个 DAP_Transfer 响应的固定长度：
                // [DAP_TRANSFER, count, status, data...] = 3 + 4*num_vars 字节
                // 每变量一个 u32 读数据（写 TAR 不返回数据）
                let resp_len = 3 + 4 * num_vars;

                while running.load(Ordering::Relaxed) {
                    // 读取 USB 响应（超时 200ms，允许优雅退出）
                    let n = match usb.read_timeout(&mut buf, Duration::from_millis(200)) {
                        Ok(n) => n,
                        Err(e) => {
                            // 超时可能只是暂无数据，检查运行标志后继续
                            if running.load(Ordering::Relaxed) {
                                log::warn!("接收线程读超时: {}", e);
                            }
                            continue;
                        }
                    };

                    // 拼接 carry + 本次读取的数据到一个连续缓冲区
                    // （carry 通常为空或仅几字节，分配开销可忽略）
                    let mut data: Vec<u8> = Vec::with_capacity(carry.len() + n);
                    data.extend_from_slice(&carry);
                    data.extend_from_slice(&buf[..n]);
                    carry.clear();

                    // 一次 read_bulk 可能读到多个响应（USB Bulk 合并短包），
                    // 按 resp_len 循环解析所有完整响应
                    let mut offset = 0;
                    while offset + resp_len <= data.len() {
                        let chunk = &data[offset..offset + resp_len];
                        offset += resp_len;

                        // 解析单个响应
                        let resp = match DapProtocol::parse_transfer_response(chunk) {
                            Ok(r) => r,
                            Err(e) => {
                                log::error!("接收线程解析失败 (seq={}): {}", seq, e);
                                seq += 1;
                                continue;
                            }
                        };

                        if resp.status != TRANSFER_OK {
                            log::warn!(
                                "接收线程收到非 OK 状态 (seq={}): status={}, count={}",
                                seq, resp.status, resp.count
                            );
                            seq += 1;
                            collect_seq.store(seq, Ordering::Release);
                            continue;
                        }

                        // 时间戳使用响应到达时刻，反映真实接收时间。
                        // 当 DAP-Link 批量处理堆积请求时，多个实际采样时刻
                        // 接近的响应会获得接近的时间戳，显示为重叠点而非
                        // 相隔 interval 的平直线，避免正弦波伪影。
                        let timestamp_sec = (Instant::now() - start_time).as_secs_f64();

                        let sample = Sample {
                            seq,
                            timestamp_sec,
                            values: resp.data.clone(),
                        };

                        ring.push(sample);
                        seq += 1;
                        collect_seq.store(seq, Ordering::Release);
                    }

                    // 保存不完整的尾部到 carry，下次读取时拼接
                    if offset < data.len() {
                        carry.extend_from_slice(&data[offset..]);
                        log::debug!(
                            "接收线程保留 {} 字节不完整响应到下次解析",
                            data.len() - offset
                        );
                    }
                }

                log::info!("接收线程退出，共接收 {} 个采样点", seq);
            })
            .map_err(|e| Error::PipelineThread(e.to_string()))?;

        Ok(handle)
    }
}

/// 流水线控制句柄
///
/// 主线程通过此句柄消费环形缓冲区中的采样数据。
pub struct PipelineHandle {
    submit_handle: JoinHandle<()>,
    collect_handle: JoinHandle<()>,
    ring_buffer: Arc<RingBuffer>,
    running: Arc<AtomicBool>,
    write_queue: Arc<Mutex<VecDeque<(u32, u32)>>>,
}

impl PipelineHandle {
    /// 推入一个写入请求（地址 + 数据）
    ///
    /// 提交线程会在下一个采样周期将此写入操作与读取操作合并为一条
    /// DAP_Transfer 命令，不影响正常采样。
    pub fn queue_write(&self, address: u32, data: u32) {
        if let Ok(mut wq) = self.write_queue.lock() {
            wq.push_back((address, data));
        }
    }

    /// 从环形缓冲区批量读取采样点
    ///
    /// 返回实际读取的数量（可能少于 buf.len()）。
    pub fn drain_samples(&self, buf: &mut [Sample]) -> usize {
        self.ring_buffer.pop_batch(buf)
    }

    /// 查询缓冲区中可用的采样点数
    pub fn available_samples(&self) -> usize {
        self.ring_buffer.available()
    }

    /// 查询已采集的总采样点数
    pub fn total_samples(&self) -> usize {
        self.ring_buffer.total_written()
    }

    /// 优雅停止采集
    ///
    /// 设置停止标志，等待两个子线程退出。
    pub fn stop(self) {
        self.running.store(false, Ordering::SeqCst);
        log::info!("等待提交线程退出...");
        let _ = self.submit_handle.join();
        log::info!("提交线程已退出");
        log::info!("等待接收线程退出...");
        let _ = self.collect_handle.join();
        log::info!("接收线程已退出");
        log::info!("流水线已停止");
    }
}
