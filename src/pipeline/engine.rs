use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::*;
use crate::usb::transfer::BulkTransfer;
use crate::dap::protocol::DapProtocol;
use crate::dap::commands::*;
use super::sample::Sample;
use super::ring_buffer::RingBuffer;

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
}

impl PipelineEngine {
    /// 从已初始化的 SWD 链路创建流水线引擎
    ///
    /// `usb` 和 `dap` 来自 `SwdLink::into_parts()`。
    /// `addresses` 是一组目标内存地址（1-8 个）。
    /// `rate_hz` 是目标采样率。
    pub fn new(
        usb: BulkTransfer,
        dap: DapProtocol,
        addresses: Vec<u32>,
        rate_hz: u32,
    ) -> Self {
        let interval_us = (1_000_000.0 / rate_hz as f64) as u64;
        Self {
            usb: Arc::new(usb),
            dap,
            addresses,
            interval_us,
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    /// 启动流水线，返回控制句柄
    ///
    /// 此方法消费 PipelineEngine，启动两个后台线程，
    /// 返回 PipelineHandle 供主线程消费环形缓冲区数据。
    pub fn start(mut self) -> Result<PipelineHandle> {
        let ring = Arc::new(RingBuffer::new(200_000)); // 10秒 @ 20kHz

        let submit = self.spawn_submit_thread()?;
        let collect = self.spawn_collect_thread(Arc::clone(&ring))?;

        Ok(PipelineHandle {
            submit_handle: submit,
            collect_handle: collect,
            ring_buffer: ring,
            running: self.running,
        })
    }

    /// 启动提交线程
    fn spawn_submit_thread(&mut self) -> Result<JoinHandle<()>> {
        let usb = Arc::clone(&self.usb);
        let interval_us = self.interval_us;
        let addresses = self.addresses.clone();
        let running = Arc::clone(&self.running);

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

        let request_count = requests.len() as u8;
        let dap_index = self.dap.dap_index;

        let handle = thread::Builder::new()
            .name("dap-submit".into())
            .spawn(move || {
                let mut seq: u64 = 0;
                let start = Instant::now();

                while running.load(Ordering::Relaxed) {
                    // --- 构造 DAP_Transfer 命令 ---
                    let mut cmd = vec![DAP_TRANSFER, dap_index, request_count];
                    for req in &requests {
                        cmd.push(req.request_byte());
                        if !req.rnw {
                            let data = req.write_data.unwrap_or(0);
                            cmd.extend_from_slice(&data.to_le_bytes());
                        }
                    }

                    // --- 非阻塞发送 ---
                    if let Err(e) = usb.write_nonblock(&cmd) {
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
    fn spawn_collect_thread(&self, ring: Arc<RingBuffer>) -> Result<JoinHandle<()>> {
        let usb = Arc::clone(&self.usb);
        let running = Arc::clone(&self.running);
        let addresses = self.addresses.clone();
        let num_vars = addresses.len();

        let handle = thread::Builder::new()
            .name("dap-collect".into())
            .spawn(move || {
                let mut buf = vec![0u8; 1024];
                let mut seq: u64 = 0;

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

                    // 解析 DAP_Transfer 响应
                    let resp = match DapProtocol::parse_transfer_response(&buf[..n]) {
                        Ok(r) => r,
                        Err(e) => {
                            log::error!("接收线程解析失败 (seq={}): {}", seq, e);
                            continue;
                        }
                    };

                    if resp.status != TRANSFER_OK {
                        log::warn!(
                            "接收线程收到非 OK 状态 (seq={}): status={}, count={}",
                            seq, resp.status, resp.count
                        );
                        continue;
                    }

                    // resp.data 中每变量一个 u32（写 TAR 不返回数据，只有读 DRW 返回）
                    if resp.data.len() != num_vars {
                        log::warn!(
                            "接收线程数据数量不匹配 (seq={}): 期望 {} 个, 实际 {} 个",
                            seq, num_vars, resp.data.len()
                        );
                        continue;
                    }

                    let sample = Sample {
                        seq,
                        values: resp.data.clone(),
                    };

                    ring.push(sample);
                    seq += 1;
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
}

impl PipelineHandle {
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
        // 等待线程退出（忽略 join 错误）
        let _ = self.submit_handle.join();
        let _ = self.collect_handle.join();
        log::info!("流水线已停止");
    }
}
