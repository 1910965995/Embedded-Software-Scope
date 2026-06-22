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
        Ok(Self {
            handle,
            ep_out,
            ep_in,
            timeout: Duration::from_millis(1000),
        })
    }

    /// 设置超时
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Bulk 写（发送命令）
    pub fn write(&self, data: &[u8]) -> Result<usize> {
        log::debug!("USB OUT: {} 字节", data.len());
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
}

// rusb::DeviceHandle 是 Send + Sync 的，所以 BulkTransfer 也可以
unsafe impl Send for BulkTransfer {}
unsafe impl Sync for BulkTransfer {}
