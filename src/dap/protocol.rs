use crate::error::*;
use crate::dap::commands::*;
use crate::usb::transfer::BulkTransfer;

/// DAP 协议处理层
pub struct DapProtocol {
    dap_index: u8,
}

impl DapProtocol {
    pub fn new() -> Self {
        Self { dap_index: 0 }
    }

    // --------------------------------------------------------
    // DAP_Info
    // --------------------------------------------------------
    pub fn build_info_request(&self, info_id: u8) -> Vec<u8> {
        vec![DAP_INFO, info_id]
    }

    pub fn parse_info_response(data: &[u8]) -> Result<(u8, Vec<u8>)> {
        if data.is_empty() {
            return Err(Error::InvalidResponse("DAP_Info 响应为空".into()));
        }
        let status = data[0];
        let content = if data.len() > 1 { data[1..].to_vec() } else { vec![] };
        Ok((status, content))
    }

    // --------------------------------------------------------
    // DAP_Connect
    // --------------------------------------------------------
    pub fn build_connect_request(&self, mode: u8) -> Vec<u8> {
        vec![DAP_CONNECT, mode]
    }

    pub fn parse_connect_response(data: &[u8]) -> Result<u8> {
        if data.is_empty() {
            return Err(Error::InvalidResponse("DAP_Connect 响应为空".into()));
        }
        Ok(data[0])
    }

    // --------------------------------------------------------
    // DAP_SWJ_Clock
    // --------------------------------------------------------
    pub fn build_clock_request(&self, freq_hz: u32) -> Vec<u8> {
        let mut cmd = vec![DAP_SWJ_CLOCK];
        cmd.extend_from_slice(&freq_hz.to_le_bytes());
        cmd
    }

    pub fn parse_clock_response(data: &[u8]) -> Result<u8> {
        if data.is_empty() {
            return Err(Error::InvalidResponse("DAP_SWJ_Clock 响应为空".into()));
        }
        Ok(data[0])
    }

    // --------------------------------------------------------
    // DAP_Transfer
    // --------------------------------------------------------
    /// 构建 DAP_Transfer 命令
    pub fn build_transfer_request(&self, requests: &[TransferRequest]) -> Vec<u8> {
        let mut cmd = vec![DAP_TRANSFER, self.dap_index, requests.len() as u8];

        for req in requests {
            cmd.push(req.request_byte());
            if !req.rnw {
                // 写操作：后跟 4 字节数据（小端序）
                let data = req.write_data.unwrap_or(0);
                cmd.extend_from_slice(&data.to_le_bytes());
            }
        }
        cmd
    }

    /// 解析 DAP_Transfer 响应
    pub fn parse_transfer_response(data: &[u8]) -> Result<TransferResponse> {
        if data.len() < 2 {
            return Err(Error::InvalidResponse(format!(
                "DAP_Transfer 响应过短: {} 字节", data.len()
            )));
        }

        let status = data[0];
        let count = data[1];

        // 读返回数据：每个读操作返回 4 字节
        let data_bytes = &data[2..];
        let num_values = data_bytes.len() / 4;
        let mut values = Vec::with_capacity(num_values);

        for i in 0..num_values {
            let offset = i * 4;
            let bytes: [u8; 4] = data_bytes[offset..offset + 4].try_into()
                .map_err(|_| Error::InvalidResponse("数据解析错误".into()))?;
            values.push(u32::from_le_bytes(bytes));
        }

        Ok(TransferResponse { status, count, data: values })
    }

    /// 便捷方法：执行一次 DAP_Transfer
    pub fn execute_transfer(
        &self,
        usb: &BulkTransfer,
        requests: &[TransferRequest],
    ) -> Result<TransferResponse> {
        let cmd = self.build_transfer_request(requests);
        usb.write(&cmd)?;
        let mut buf = vec![0u8; 1024];
        let n = usb.read(&mut buf)?;
        DapProtocol::parse_transfer_response(&buf[..n])
    }

    // --------------------------------------------------------
    // 复合操作：查询 DAP 信息
    // --------------------------------------------------------
    /// 查询 DAP-Link 设备信息
    pub fn query_info(&self, usb: &BulkTransfer) -> Result<DapInfo> {
        let vendor = self.query_info_string(usb, INFO_ID_VENDOR)?;
        let product = self.query_info_string(usb, INFO_ID_PRODUCT)?;
        let serial = self.query_info_string(usb, INFO_ID_SER_NUM)?;
        let fw_version = self.query_info_string(usb, INFO_ID_FW_VER)?;

        let packet_size = self.query_info_u16(usb, INFO_ID_PACKET_SIZE)?;
        let packet_count = self.query_info_u8(usb, INFO_ID_PACKET_COUNT)?;

        Ok(DapInfo { vendor, product, serial, fw_version, packet_size, packet_count })
    }

    fn query_info_raw(&self, usb: &BulkTransfer, id: u8) -> Result<(u8, Vec<u8>)> {
        let cmd = self.build_info_request(id);
        usb.write(&cmd)?;
        let mut buf = [0u8; 64];
        let n = usb.read(&mut buf)?;
        Self::parse_info_response(&buf[..n])
    }

    fn query_info_string(&self, usb: &BulkTransfer, id: u8) -> Result<String> {
        let (_, content) = self.query_info_raw(usb, id)?;
        let s = String::from_utf8_lossy(&content).trim_end_matches('\0').to_string();
        Ok(s)
    }

    fn query_info_u16(&self, usb: &BulkTransfer, id: u8) -> Result<u16> {
        let (_, content) = self.query_info_raw(usb, id)?;
        if content.len() >= 2 {
            Ok(u16::from_le_bytes([content[0], content[1]]))
        } else {
            Ok(0)
        }
    }

    fn query_info_u8(&self, usb: &BulkTransfer, id: u8) -> Result<u8> {
        let (_, content) = self.query_info_raw(usb, id)?;
        Ok(content.first().copied().unwrap_or(0))
    }
}
