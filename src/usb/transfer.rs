use rusb;
use std::time::Duration;
use crate::error::*;
use crate::usb::device;

/// USB Bulk 传输封装
pub struct BulkTransfer {
    handle: rusb::DeviceHandle<rusb::Context>,
    ep_out: u8,
    ep_in: u8,
    timeout: Duration,
    /// 已 claim 的 USB 接口号，释放时需要 release_interface
    interface_num: u8,
    /// 是否已显式 release（避免 Drop 时重复释放）
    released: bool,
}

impl BulkTransfer {
    /// 创建 BulkTransfer 实例（自动发现并连接设备）
    pub fn open() -> Result<Self> {
        let (handle, interface_num) = device::open_first_device()?;
        let (ep_out, ep_in) = device::find_bulk_endpoints(&handle)?;
        log::info!("Bulk 端点: OUT=0x{:02X}, IN=0x{:02X}", ep_out, ep_in);
        let transfer = Self {
            handle,
            ep_out,
            ep_in,
            timeout: Duration::from_millis(1000),
            interface_num,
            released: false,
        };
        // 清空 USB IN 端点缓冲区中可能残留的数据
        // （流水线非阻塞写入后可能残留未读响应）
        transfer.flush_input();
        Ok(transfer)
    }

    /// 显式释放 USB 接口和设备
    ///
    /// 高采样率下停止时，DAP-Link IN 端点可能堆积大量未读响应。
    /// 必须先排空这些残留响应，否则 DAP_Disconnect 命令会被淹没，
    /// DAP-Link 固件仍处于忙状态，Keil 等工具无法重新连接。
    pub fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        // 1. 排空 USB IN 端点的所有残留响应
        //    高速采样时可能有上千个未读响应堆积在缓冲区中
        let mut drain_count = 0;
        let mut buf = [0u8; 1024];
        loop {
            match self.handle.read_bulk(self.ep_in, &mut buf, Duration::from_millis(50)) {
                Ok(n) if n > 0 => {
                    drain_count += 1;
                    // 继续读，直到超时无数据
                }
                _ => break,
            }
        }
        if drain_count > 0 {
            log::info!("排空 {} 个残留 USB 响应", drain_count);
        }

        // 2. 发送 DAP_Disconnect 命令，清理 CMSIS-DAP 连接状态
        log::info!("发送 DAP_Disconnect 命令");
        let disconnect_cmd = [crate::dap::commands::DAP_DISCONNECT];
        if let Err(e) = self.handle.write_bulk(self.ep_out, &disconnect_cmd, Duration::from_millis(200)) {
            log::warn!("DAP_Disconnect 发送失败: {}", e);
        } else {
            // 读取 DAP_Disconnect 响应
            let _ = self.handle.read_bulk(self.ep_in, &mut buf, Duration::from_millis(200));
        }

        // 3. 发送 DAP_HostStatus (Connect LED off)
        let host_status_cmd = [crate::dap::commands::DAP_LED, 0x00, 0x00];
        if let Ok(_) = self.handle.write_bulk(self.ep_out, &host_status_cmd, Duration::from_millis(200)) {
            let _ = self.handle.read_bulk(self.ep_in, &mut buf, Duration::from_millis(200));
        }

        // 4. 释放 USB 接口
        log::info!("显式释放 USB 接口 {}", self.interface_num);
        if let Err(e) = self.handle.release_interface(self.interface_num) {
            log::warn!("release_interface 失败: {}", e);
        }

        log::info!("DAP-Link 释放完成");
    }

    /// 设置超时
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Bulk 写（发送命令）
    pub fn write(&self, data: &[u8]) -> Result<usize> {
        log::debug!("USB OUT ({} 字节): {:02X?}", data.len(), data);
        let n = self.handle.write_bulk(self.ep_out, data, self.timeout)?;
        Ok(n)
    }

    /// Bulk 写（带 200ms 超时）
    ///
    /// 注意：rusb 中 timeout < 1ms 会无限阻塞。
    /// 此处使用 200ms 超时，确保提交线程在收到停止标志后能及时退出，
    /// 避免高采样率下 write 永久阻塞导致 join 卡死、USB 设备无法释放。
    pub fn write_nonblock(&self, data: &[u8]) -> Result<usize> {
        let n = self.handle.write_bulk(self.ep_out, data, Duration::from_millis(200))?;
        Ok(n)
    }

    /// Bulk 读（接收响应）
    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let n = self.handle.read_bulk(self.ep_in, buf, self.timeout)?;
        log::debug!("USB IN: {} 字节", n);
        Ok(n)
    }

    /// Bulk 读（可指定超时）
    pub fn read_timeout(&self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        let n = self.handle.read_bulk(self.ep_in, buf, timeout)?;
        Ok(n)
    }

    /// 清空 USB IN 端点缓冲区中的残留数据
    ///
    /// 流水线非阻塞写入后，IN 端点可能残留未读响应。
    /// 新连接时调用此方法可避免读到陈旧数据导致协议失步。
    pub fn flush_input(&self) {
        let mut buf = [0u8; 1024];
        let mut flushed = 0;
        loop {
            match self.handle.read_bulk(self.ep_in, &mut buf, Duration::from_millis(10)) {
                Ok(n) => {
                    flushed += n;
                }
                Err(_) => break,
            }
        }
        if flushed > 0 {
            log::info!("已清空 USB 缓冲区残留数据: {} 字节", flushed);
        }
    }
}

// rusb::DeviceHandle 是 Send + Sync 的，所以 BulkTransfer 也可以
unsafe impl Send for BulkTransfer {}
unsafe impl Sync for BulkTransfer {}

impl Drop for BulkTransfer {
    /// 兜底释放：如果未显式调用 release()，Drop 时自动执行。
    ///
    /// 这确保即使 Arc::try_unwrap 失败（如 PipelineEngine 仍持有引用），
    /// USB 接口也能在最后一个引用释放时被正确释放，避免 DAP-Link 卡死。
    fn drop(&mut self) {
        if !self.released {
            log::warn!("BulkTransfer 未显式 release，Drop 时自动释放");
            self.release();
        }
    }
}
