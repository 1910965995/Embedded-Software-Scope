use log::{info, warn};
use crate::error::*;
use crate::usb::transfer::BulkTransfer;
use crate::dap::protocol::DapProtocol;
use crate::dap::commands::*;

/// SWD 链路层：封装 SWD 初始化和内存访问
pub struct SwdLink {
    usb: BulkTransfer,
    dap: DapProtocol,
    ap_index: u8,
}

/// 连接后获取的设备信息
#[derive(Debug)]
pub struct DeviceInfo {
    pub dpidr: u32,
    pub ap_idr: u32,
    pub target_info: String,
}

impl SwdLink {
    /// 创建 SWD 链路（自动连接 USB 设备）
    pub fn new() -> Result<Self> {
        let usb = BulkTransfer::open()?;
        let dap = DapProtocol::new();
        Ok(Self { usb, dap, ap_index: 0 })
    }

    /// 获取 USB 传输层引用
    pub fn usb(&self) -> &BulkTransfer {
        &self.usb
    }

    /// 获取协议层引用
    pub fn dap(&self) -> &DapProtocol {
        &self.dap
    }

    // --------------------------------------------------------
    // SWD 初始化流程
    // --------------------------------------------------------
    /// 完整的 SWD 初始化序列（精确匹配 Keil USB 抓包流程）
    pub fn init(&mut self) -> Result<DeviceInfo> {
        info!("=== SWD 初始化开始 ===");

        // 0. 先断开之前可能残留的连接（清理 DAP-Link 状态）
        let mut buf = [0u8; 64];
        self.usb.write(&[DAP_DISCONNECT])?;
        let _ = self.usb.read(&mut buf);
        std::thread::sleep(std::time::Duration::from_millis(100));

        // 1. 查询 DAP-Link 信息
        let dap_info = self.dap.query_info(self.usb())?;
        info!("DAP-Link: {} {} v{}",
            dap_info.vendor, dap_info.product, dap_info.fw_version);

        // 2. SWD 连接
        self.swd_connect()?;

        // 3. 设置 SWD 时钟（10MHz，与 Keil 抓包一致）
        let clock = 10_000_000u32;
        let cmd = self.dap.build_clock_request(clock);
        self.usb.write(&cmd)?;
        let n = self.usb.read(&mut buf)?;
        DapProtocol::parse_clock_response(&buf[..n])?;
        info!("SWD 时钟: {} MHz", clock as f32 / 1_000_000.0);

        // 4. DAP_TransferConfigure (idle=0, wait_retry=100, match_retry=0)
        let cfg_cmd = DapProtocol::build_transfer_configure_request(0, 100, 0);
        self.usb.write(&cfg_cmd)?;
        let n = self.usb.read(&mut buf)?;
        DapProtocol::parse_transfer_configure_response(&buf[..n])?;

        // 5. DAP_SWD_Configure (turnaround=1, 无 dataPhase)
        let swd_cfg = DapProtocol::build_swd_configure_request(0x00);
        self.usb.write(&swd_cfg)?;
        let n = self.usb.read(&mut buf)?;
        DapProtocol::parse_swd_configure_response(&buf[..n])?;

        // 6. DAP_HostStatus (Connect LED on)
        let host_cmd = DapProtocol::build_host_status_request(0, 1);
        self.usb.write(&host_cmd)?;
        let n = self.usb.read(&mut buf)?;
        DapProtocol::parse_host_status_response(&buf[..n])?;

        // 7. 读取 DPIDR（DAP_Connect 已做 JTAG-to-SWD 切换，直接读）
        let dpidr = self.read_dpidr()?;
        info!("DPIDR = 0x{:08X}", dpidr);

        // 8. 再次读取 DPIDR（确认连接稳定）
        let _ = self.read_dpidr()?;

        // 9. 调试电源上电（写 CTRL/STAT = CSYSPWRUPREQ | CDBGPWRUPREQ）
        self.power_up_debug()?;

        // 10. 写 DP SELECT = 0
        let select_req = TransferRequest::write_dp(DP_REG_SELECT, 0);
        let resp = self.dap.execute_transfer(self.usb(), &[select_req])?;
        if resp.status != TRANSFER_OK {
            return Err(Error::Swd("写 SELECT 失败".into()));
        }

        // 11. 扫描 AP
        let ap_idr = self.scan_ap()?;
        info!("AP{} IDR = 0x{:08X}", self.ap_index, ap_idr);

        // 12. 验证内存读取（读向量表地址 0x00000000）
        match self.read_memory(0x00000000) {
            Ok(v) => info!("验证读取 (0x00000000) = 0x{:08X} ✓", v),
            Err(e) => warn!("验证读取失败: {}", e),
        }

        info!("=== SWD 初始化完成 ===");
        Ok(DeviceInfo {
            dpidr,
            ap_idr,
            target_info: format!("Cortex-M DPIDR=0x{:08X}", dpidr),
        })
    }

    /// SWD 连接
    fn swd_connect(&self) -> Result<()> {
        let cmd = self.dap.build_connect_request(CONNECT_MODE_SWD);
        info!("DAP_Connect 请求: {:02X?}", cmd);
        self.usb.write(&cmd)?;
        let mut buf = [0u8; 64];
        let n = self.usb.read(&mut buf)?;
        info!("DAP_Connect 响应 ({} 字节): {:02X?}", n, &buf[..n]);
        let mode = DapProtocol::parse_connect_response(&buf[..n])?;
        info!("DAP_Connect 返回: {}", mode);
        Ok(())
    }

    /// 硬件复位目标 MCU (通过 DAP-Link 的 nRESET 引脚)
    fn hardware_reset(&self) -> Result<()> {
        info!("脉冲复位目标 MCU...");
        let mut buf = [0u8; 64];
        // nRESET = bit 2, 拉低 100ms 再释放
        let cmd = self.dap.build_pins_request(0x04, 0x00, 100_000); // nRESET低 100ms
        self.usb.write(&cmd)?;
        self.usb.read(&mut buf)?;
        let cmd = self.dap.build_pins_request(0x04, 0x04, 100_000); // nRESET高 100ms
        self.usb.write(&cmd)?;
        self.usb.read(&mut buf)?;
        info!("复位完成");
        Ok(())
    }

    /// SWD 线复位 + JTAG-to-SWD 切换
    /// SWD 线复位（精确匹配 Keil 抓包序列：Line Reset + JTAG-to-SWD + Line Reset）
    fn swd_line_reset(&self) -> Result<()> {
        info!("发送 SWD 初始化序列...");
        let mut buf = [0u8; 64];

        // 1. Line Reset: 51 bits TMS=1 (Keil: 12 33 FF FF FF FF FF FF FF)
        let cmd1 = vec![DAP_SWJ_SEQUENCE, 51, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        self.usb.write(&cmd1)?;
        let n = self.usb.read(&mut buf)?;
        info!("SWJ Line Reset 响应 ({} 字节): {:02X?}", n, &buf[..n]);

        // 2. JTAG-to-SWD 切换: 16 bits (Keil: 12 10 9E E7)
        let cmd2 = vec![DAP_SWJ_SEQUENCE, 16, 0x9E, 0xE7];
        self.usb.write(&cmd2)?;
        let n = self.usb.read(&mut buf)?;
        info!("SWJ JTAG-to-SWD 响应 ({} 字节): {:02X?}", n, &buf[..n]);

        // 3. Line Reset: 51 bits TMS=1 (Keil: 12 33 FF FF FF FF FF FF FF)
        let cmd3 = vec![DAP_SWJ_SEQUENCE, 51, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        self.usb.write(&cmd3)?;
        let n = self.usb.read(&mut buf)?;
        info!("SWJ Line Reset 2 响应 ({} 字节): {:02X?}", n, &buf[..n]);

        info!("SWD 序列完成");
        Ok(())
    }

    /// 自动检测 SWD 时钟（从低频到高频试探）
    fn auto_detect_clock(&self) -> Result<u32> {
        let clock_vals = [1_000_000, 2_000_000, 4_000_000, 8_000_000, 10_000_000];
        let mut best_clock = 1_000_000;

        for &clock in &clock_vals {
            let cmd = self.dap.build_clock_request(clock);
            self.usb.write(&cmd)?;
            let mut buf = [0u8; 64];
            let n = self.usb.read(&mut buf)?;
            let status = DapProtocol::parse_clock_response(&buf[..n])?;
            if status == 0 {
                best_clock = clock;
            } else {
                break;
            }
        }
        info!("最佳 SWD 时钟: {} Hz", best_clock);
        Ok(best_clock)
    }

    /// 读取 DPIDR 寄存器
    fn read_dpidr(&self) -> Result<u32> {
        let req = TransferRequest::read_dp(DP_REG_DPIDR);
        let resp = self.dap.execute_transfer(self.usb(), &[req])?;
        if resp.status != TRANSFER_OK {
            return Err(Error::TransferFailed(resp.status, resp.count));
        }
        Ok(resp.data.first().copied().unwrap_or(0))
    }

    /// 调试电源上电
    fn power_up_debug(&self) -> Result<()> {
        // SWD 读操作有流水线延迟：前一个读的数据在 RDBUFF 中。
        // 两次 DPIDR 读后，需要先读 RDBUFF 清空流水线，再做写操作。
        let rdbuff_req = TransferRequest::read_dp(DP_REG_RDBUFF);
        let _ = self.dap.execute_transfer(self.usb(), &[rdbuff_req])?;

        // 请求系统调试电源（写 CTRL/STAT）
        let req1 = TransferRequest::write_dp(DP_REG_CTRL_STAT, CSYSPWRUPREQ | CDBGPWRUPREQ);
        let resp = self.dap.execute_transfer(self.usb(), &[req1])?;
        if resp.status != TRANSFER_OK {
            return Err(Error::Swd(format!("电源上电请求失败: status={}, count={}", resp.status, resp.count)));
        }

        // 读 CTRL/STAT 确认电源就绪（此 DAP-Link 无流水线延迟，直接返回）
        let req2 = TransferRequest::read_dp(DP_REG_CTRL_STAT);
        let resp = self.dap.execute_transfer(self.usb(), &[req2])?;
        let ctrl_stat = resp.data.first().copied().unwrap_or(0);

        // 读 RDBUFF 消耗流水线中的残留数据
        let rdbuff_req = TransferRequest::read_dp(DP_REG_RDBUFF);
        let _ = self.dap.execute_transfer(self.usb(), &[rdbuff_req])?;

        if (ctrl_stat & (CSYSPWRUPACK | CDBGPWRUPACK)) != (CSYSPWRUPACK | CDBGPWRUPACK) {
            return Err(Error::Swd(format!("调试电源未就绪: CTRL/STAT=0x{:08X}", ctrl_stat)));
        }
        info!("调试电源已就绪: CTRL/STAT=0x{:08X}", ctrl_stat);
        Ok(())
    }

    /// 选择 AP 端口
    fn select_ap(&self, ap: u8) -> Result<()> {
        // DP SELECT 寄存器: APSEL=[31:24], APBANKSEL=[7:4]
        let req = TransferRequest::write_dp(DP_REG_SELECT, (ap as u32) << 24);
        let resp = self.dap.execute_transfer(self.usb(), &[req])?;
        if resp.status != TRANSFER_OK {
            return Err(Error::Swd(format!("选择 AP{} 失败", ap)));
        }
        Ok(())
    }

    /// 扫描 AP 端口
    fn scan_ap(&mut self) -> Result<u32> {
        // 选择 AP0 并读取 IDR
        self.select_ap(0)?;

        // 通过读 AP IDR 来验证 AP 是否存在
        // CSW 默认配置下，读 DRW 寄存器返回的是 IDR
        // 需要先写 CSW 设置 32-bit 访问，简化处理使用默认值
        let req = TransferRequest::read_ap(AP_REG_IDR);
        let resp = self.dap.execute_transfer(self.usb(), &[req])?;

        if resp.status != TRANSFER_OK || resp.data.is_empty() {
            return Err(Error::Swd("AP 扫描失败".into()));
        }

        self.ap_index = 0;
        Ok(resp.data[0])
    }

    // --------------------------------------------------------
    // 内存读取
    // --------------------------------------------------------
    /// 读取 32 位内存值（单地址）
    pub fn read_memory(&self, address: u32) -> Result<u32> {
        // 确保 AP 已选中
        // 两条请求打包到一次 DAP_Transfer:
        // 1. 写 AP TAR = address
        // 2. 读 AP DRW → 获取数据
        let requests = [
            TransferRequest::write_ap(AP_REG_TAR, address),
            TransferRequest::read_ap(AP_REG_DRW),
        ];

        let resp = self.dap.execute_transfer(self.usb(), &requests)?;

        if resp.status != TRANSFER_OK {
            return Err(Error::TransferFailed(resp.status, resp.count));
        }

        // response.data 只包含读操作的结果
        Ok(resp.data.first().copied().unwrap_or(0))
    }

    /// 批量读取多个地址
    pub fn read_memory_batch(&self, addresses: &[u32]) -> Result<Vec<u32>> {
        let max_vars = addresses.len().min(8);
        let mut requests = Vec::with_capacity(max_vars * 2);

        for &addr in &addresses[..max_vars] {
            requests.push(TransferRequest::write_ap(AP_REG_TAR, addr));
            requests.push(TransferRequest::read_ap(AP_REG_DRW));
        }

        let resp = self.dap.execute_transfer(self.usb(), &requests)?;

        if resp.status != TRANSFER_OK {
            return Err(Error::TransferFailed(resp.status, resp.count));
        }

        Ok(resp.data)
    }

    /// 读取内存并解析为 float
    pub fn read_float(&self, address: u32) -> Result<f32> {
        let raw = self.read_memory(address)?;
        Ok(f32::from_bits(raw))
    }
}
