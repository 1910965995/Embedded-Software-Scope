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
}

impl BulkTransfer {
    /// 创建 BulkTransfer 实例（自动发现并连接设备）
    pub fn open() -> Result<Self> {
        let handle = device::open_first_device()?;
        let (ep_out, ep_in) = device::find_bulk_endpoints(&handle)?;
        log::info!("Bulk 端点: OUT=0x{:02X}, IN=0x{:02X}", ep_out, ep_in);
        let transfer = Self {
            handle,
            ep_out,
            ep_in,
            timeout: Duration::from_millis(1000),
        };
        // 清空 USB IN 端点缓冲区中可能残留的数据
        // （流水线非阻塞写入后可能残留未读响应）
        transfer.flush_input();
        Ok(transfer)
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

    /// Bulk 写（非阻塞，timeout=0 立即提交）
    pub fn write_nonblock(&self, data: &[u8]) -> Result<usize> {
        let n = self.handle.write_bulk(self.ep_out, data, Duration::from_secs(0))?;
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
