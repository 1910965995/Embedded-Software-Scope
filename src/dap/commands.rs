// ============================================================
// DAP 命令码
// ============================================================
pub const DAP_INFO: u8 = 0x00;
pub const DAP_CONNECT: u8 = 0x02;
pub const DAP_SWJ_CLOCK: u8 = 0x11;
pub const DAP_TRANSFER: u8 = 0x05;

// ============================================================
// DAP_Info 请求 ID
// ============================================================
pub const INFO_ID_VENDOR: u8 = 0x01;
pub const INFO_ID_PRODUCT: u8 = 0x02;
pub const INFO_ID_SER_NUM: u8 = 0x03;
pub const INFO_ID_FW_VER: u8 = 0x04;
pub const INFO_ID_DEVICE_VENDOR: u8 = 0x05;
pub const INFO_ID_DEVICE_NAME: u8 = 0x06;
pub const INFO_ID_CAPABILITIES: u8 = 0xF0;
pub const INFO_ID_PACKET_COUNT: u8 = 0xFE;
pub const INFO_ID_PACKET_SIZE: u8 = 0xFF;

/// DAP_Connect 模式
pub const CONNECT_MODE_SWD: u8 = 0x00;
pub const CONNECT_MODE_JTAG: u8 = 0x01;

/// DAP_Transfer 状态码
pub const TRANSFER_OK: u8 = 0x01;
pub const TRANSFER_WAIT: u8 = 0x02;
pub const TRANSFER_FAULT: u8 = 0x04;
pub const TRANSFER_PROTOCOL_ERR: u8 = 0x08;

// ============================================================
// SWD 请求字节编码（CMSIS-DAP 标准）
//
// Bit 0: RnW     (0=写, 1=读)
// Bit 1: APnDP   (0=DP, 1=AP)
// Bit 2: A2      (寄存器地址 bit 2)
// Bit 3: A3      (寄存器地址 bit 3)
// ============================================================

/// 构建 SWD 请求字节（标准 CMSIS-DAP 编码）
pub fn make_request(rnw: bool, apndp: bool, a2: bool, a3: bool) -> u8 {
    let mut val = 0u8;
    if rnw { val |= 1 << 0; }
    if apndp { val |= 1 << 1; }
    if a2 { val |= 1 << 2; }
    if a3 { val |= 1 << 3; }
    val
}

/// 从寄存器地址提取位
fn reg_a2(addr: u8) -> bool { (addr >> 2) & 0x01 != 0 }
fn reg_a3(addr: u8) -> bool { (addr >> 3) & 0x01 != 0 }

// ============================================================
// DP 寄存器地址
// ============================================================
/// DP 寄存器地址: bits[3:2] = A3,A2
pub const DP_REG_DPIDR: u8 = 0x00;
pub const DP_REG_CTRL_STAT: u8 = 0x04;
pub const DP_REG_SELECT: u8 = 0x08;
pub const DP_REG_RDBUFF: u8 = 0x0C;

/// 读 DP 寄存器请求字节
pub fn req_read_dp(addr: u8) -> u8 {
    make_request(true, false, reg_a2(addr), reg_a3(addr))
}

/// 写 DP 寄存器请求字节
pub fn req_write_dp(addr: u8) -> u8 {
    make_request(false, false, reg_a2(addr), reg_a3(addr))
}

// ============================================================
// AP 寄存器地址
// ============================================================
pub const AP_REG_CSW: u8 = 0x00;
pub const AP_REG_TAR: u8 = 0x04;
pub const AP_REG_DRW: u8 = 0x0C;
pub const AP_REG_IDR: u8 = 0x0C; // 同 DRW (CSW=0 时返回 IDR)

/// 读 AP 寄存器请求字节
pub fn req_read_ap(addr: u8) -> u8 {
    make_request(true, true, reg_a2(addr), reg_a3(addr))
}

/// 写 AP 寄存器请求字节
pub fn req_write_ap(addr: u8) -> u8 {
    make_request(false, true, reg_a2(addr), reg_a3(addr))
}

// ============================================================
// DP CTRL/STAT 寄存器位定义
// ============================================================
pub const CSYSPWRUPREQ: u32 = 1 << 0;
pub const CDBGPWRUPREQ: u32 = 1 << 1;
pub const CSYSPWRUPACK: u32 = 1 << 2;
pub const CDBGPWRUPACK: u32 = 1 << 3;
pub const CDBGRSTREQ: u32 = 1 << 4;
pub const CDBGRSTACK: u32 = 1 << 5;

// ============================================================
// Transfer 请求/响应 结构体
// ============================================================

/// Transfer 请求描述
#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub rnw: bool,
    pub apndp: bool,
    pub reg_addr: u8,
    pub write_data: Option<u32>,
}

impl TransferRequest {
    pub fn read_dp(reg_addr: u8) -> Self {
        Self { rnw: true, apndp: false, reg_addr, write_data: None }
    }

    pub fn write_dp(reg_addr: u8, data: u32) -> Self {
        Self { rnw: false, apndp: false, reg_addr, write_data: Some(data) }
    }

    pub fn read_ap(reg_addr: u8) -> Self {
        Self { rnw: true, apndp: true, reg_addr, write_data: None }
    }

    pub fn write_ap(reg_addr: u8, data: u32) -> Self {
        Self { rnw: false, apndp: true, reg_addr, write_data: Some(data) }
    }

    /// 编码为 DAP_Transfer 请求字节
    pub fn request_byte(&self) -> u8 {
        make_request(self.rnw, self.apndp, reg_a2(self.reg_addr), reg_a3(self.reg_addr))
    }
}

/// Transfer 响应
#[derive(Debug)]
pub struct TransferResponse {
    pub status: u8,
    pub count: u8,
    pub data: Vec<u32>,
}

/// DAP 设备信息
#[derive(Debug)]
pub struct DapInfo {
    pub vendor: String,
    pub product: String,
    pub serial: String,
    pub fw_version: String,
    pub packet_size: u16,
    pub packet_count: u8,
}
