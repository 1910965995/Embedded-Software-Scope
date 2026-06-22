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
    /// 完整的 SWD 初始化序列
    pub fn init(&mut self) -> Result<DeviceInfo> {
        info!("=== SWD 初始化开始 ===");

        // 1. 查询 DAP-Link 信息
        let dap_info = self.dap.query_info(self.usb())?;
        info!("DAP-Link: {} {} v{}",
            dap_info.vendor, dap_info.product, dap_info.fw_version);

        // 2. SWD 连接
        self.swd_connect()?;

        // 3. 设置 SWD 时钟（AC7840X 先用保守的 1MHz）
        let clock = 1_000_000u32;
        let cmd = self.dap.build_clock_request(clock);
        self.usb.write(&cmd)?;
        let mut buf = [0u8; 64];
        let n = self.usb.read(&mut buf)?;
        DapProtocol::parse_clock_response(&buf[..n])?;
        info!("SWD 时钟: {} MHz", clock as f32 / 1_000_000.0);

        // 4. 硬件复位目标 MCU
        self.hardware_reset()?;

        // 5. SWD 线复位 + JTAG-to-SWD 切换
        self.swd_line_reset()?;

        // 6. 读取 DPIDR
        let dpidr = self.read_dpidr()?;
        info!("DPIDR = 0x{:08X}", dpidr);

        // 7. 调试电源上电
        self.power_up_debug()?;

        // 8. 扫描 AP
        let ap_idr = self.scan_ap()?;
        info!("AP{} IDR = 0x{:08X}", self.ap_index, ap_idr);

        // 9. 验证内存读取（读向量表地址 0x00000000）
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
        // nRESET 引脚 = bit 2
        // 1. 拉低 nRESET (mask=bit2, value=0) + 等待 10ms
        let cmd = self.dap.build_pins_request(0x04, 0x00, 10000);
        self.usb.write(&cmd)?;
        let mut buf = [0u8; 64];
        self.usb.read(&mut buf)?;

        // 2. 拉高 nRESET (mask=bit2, value=bit2) + 等待 10ms
        let cmd = self.dap.build_pins_request(0x04, 0x04, 10000);
        self.usb.write(&cmd)?;
        self.usb.read(&mut buf)?;

        info!("复位完成");
        Ok(())
    }

    /// SWD 线复位 (对 SW-DP 目标只需 Line Reset, 无需 JTAG-to-SWD 切换)
    fn swd_line_reset(&self) -> Result<()> {
        info!("发送 SWD Line Reset...");

        // 对原生 SW-DP 目标，发送 50+ 个 TCK 时钟的 TMS=1
        // 使 SW-DP 进入 Line Reset 状态
        let seq = &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // 56 bits
                     0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]; // 56 bits (总共 112 bits)
        let cmd = self.dap.build_swj_sequence_request(seq);
        self.usb.write(&cmd)?;
        let mut buf = [0u8; 64];
        self.usb.read(&mut buf)?;

        info!("SWD 线复位完成");
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
        // 请求系统调试电源
        let req1 = TransferRequest::write_dp(DP_REG_CTRL_STAT, CSYSPWRUPREQ | CDBGPWRUPREQ);
        let resp = self.dap.execute_transfer(self.usb(), &[req1])?;
        if resp.status != TRANSFER_OK {
            return Err(Error::Swd("电源上电请求失败".into()));
        }

        // 读取 CTRL/STAT 确认电源就绪
        let req2 = TransferRequest::read_dp(DP_REG_CTRL_STAT);
        let resp = self.dap.execute_transfer(self.usb(), &[req2])?;
        let ctrl_stat = resp.data.first().copied().unwrap_or(0);

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
