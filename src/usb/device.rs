use rusb::{Context, Device, UsbContext};
use crate::error::*;

/// CMSIS-DAP v2 已知设备 VID/PID
pub const KNOWN_DEVICES: &[(u16, u16)] = &[
    (0x0d28, 0x0204),  // ARM mbed DAPLink
    (0x0d28, 0x0205),  // ARM mbed DAPLink (alternative)
    (0x1fc9, 0x0132),  // NXP LPC-Link2
    (0x03eb, 0x2111),  // Atmel EDBG
    (0x2e88, 0x0001),  // AT-Link (AT32)
    (0x0483, 0x374b),  // ST-Link v3 (CMSIS-DAP mode)
    (0x0483, 0x374c),  // ST-Link v3 (CMSIS-DAP mode alt)
    (0x0483, 0x374d),  // ST-Link v3 (CMSIS-DAP mode alt 2)
    (0x0483, 0x374e),  // ST-Link v3 (CMSIS-DAP mode alt 3)
    (0xc251, 0x1001),  // JLink (CMSIS-DAP mode)
    (0xc251, 0x1002),  // JLink (CMSIS-DAP mode alt)
];

/// USB 设备信息
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    pub bus_number: u8,
    pub address: u8,
}

/// 列出所有 CMSIS-DAP v2 设备
pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    let context = Context::new()?;
    let devices = context.devices()?;
    let mut result = Vec::new();

    for device in devices.iter() {
        let desc = device.device_descriptor()?;
        let vid = desc.vendor_id();
        let pid = desc.product_id();

        // 策略1: 已知 VID/PID 匹配
        let known = is_known_device(vid, pid);

        // 策略2: 检查产品字符串描述符是否包含 "CMSIS-DAP"
        let has_cmsis = if !known {
            match device.open() {
                Ok(handle) => {
                    if let Some(idx) = desc.product_string_index() {
                        if let Ok(s) = handle.read_string_descriptor_ascii(idx) {
                            s.to_uppercase().contains("CMSIS-DAP")
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
                Err(_) => false,
            }
        } else {
            true
        };

        if !known && !has_cmsis {
            continue;
        }

        // 检查是否为 CMSIS-DAP v2 接口 (Bulk 端点)
        if !has_cmsis_dap_interface(&device)? {
            continue;
        }

        let handle = match device.open() {
            Ok(h) => h,
            Err(_) => continue,
        };

        let manufacturer = read_string_descriptor(&handle, desc.manufacturer_string_index());
        let product = read_string_descriptor(&handle, desc.product_string_index());
        let serial = read_string_descriptor(&handle, desc.serial_number_string_index());

        result.push(DeviceInfo {
            vid, pid,
            manufacturer,
            product,
            serial,
            bus_number: device.bus_number(),
            address: device.address(),
        });
    }

    Ok(result)
}

/// 根据 VID/PID 判断是否为已知 DAP 设备
fn is_known_device(vid: u16, pid: u16) -> bool {
    KNOWN_DEVICES.contains(&(vid, pid))
}

/// 已知的非 CMSIS-DAP 接口类
const CDC_CLASS: u8 = 0x02;    // Communication Device Class
const CDC_DATA_CLASS: u8 = 0x0A; // CDC Data Class
const MSC_CLASS: u8 = 0x08;    // Mass Storage Class
const HUB_CLASS: u8 = 0x09;    // Hub Class
const AUDIO_CLASS: u8 = 0x01;  // Audio Class
const HID_CLASS: u8 = 0x03;    // Human Interface Device

/// 检查 interface 是否为 CMSIS-DAP v2 候选（Bulk 端点对 + 非已知类）
fn is_cmsis_dap_candidate(alt: &rusb::InterfaceDescriptor) -> bool {
    let class = alt.class_code();
    // 排除已知的非 CMSIS-DAP 接口类
    if class == CDC_CLASS || class == CDC_DATA_CLASS || class == MSC_CLASS
        || class == HUB_CLASS || class == AUDIO_CLASS || class == HID_CLASS {
        return false;
    }
    // 必须有 Bulk IN + Bulk OUT
    let mut has_bulk_in = false;
    let mut has_bulk_out = false;
    for ep in alt.endpoint_descriptors() {
        if ep.transfer_type() == rusb::TransferType::Bulk {
            match ep.direction() {
                rusb::Direction::In => has_bulk_in = true,
                rusb::Direction::Out => has_bulk_out = true,
            }
        }
    }
    has_bulk_in && has_bulk_out
}

/// 检查设备是否有 CMSIS-DAP v2 接口
fn has_cmsis_dap_interface(device: &Device<Context>) -> Result<bool> {
    let config = match device.config_descriptor(0) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };
    for interface in config.interfaces() {
        for alt in interface.descriptors() {
            if is_cmsis_dap_candidate(&alt) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// 打开并返回第一个 CMSIS-DAP v2 设备
pub fn open_first_device() -> Result<rusb::DeviceHandle<Context>> {
    let context = Context::new()?;
    let devices = context.devices()?;

    for device in devices.iter() {
        let desc = device.device_descriptor()?;
        let vid = desc.vendor_id();
        let pid = desc.product_id();

        if !is_known_device(vid, pid) {
            match device.open() {
                Ok(handle) => {
                    let is_cmsis = match desc.product_string_index() {
                        Some(idx) => handle.read_string_descriptor_ascii(idx)
                            .map(|s| s.to_uppercase().contains("CMSIS-DAP"))
                            .unwrap_or(false),
                        None => false,
                    };
                    if !is_cmsis {
                        continue;
                    }
                }
                Err(_) => continue,
            }
        }

        if !has_cmsis_dap_interface(&device)? {
            continue;
        }

        let handle = device.open()?;
        let interface_num = find_cmsis_dap_interface_number(&device)?;
        log::info!("claiming interface {}", interface_num);
        handle.claim_interface(interface_num)?;
        return Ok(handle);
    }

    Err(Error::DeviceNotFound)
}

/// 查找 CMSIS-DAP v2 接口号 (排除 CDC/MSC 等已知非 DAP 接口)
fn find_cmsis_dap_interface_number(device: &Device<Context>) -> Result<u8> {
    let config = device.config_descriptor(0)?;
    for interface in config.interfaces() {
        for alt in interface.descriptors() {
            if is_cmsis_dap_candidate(&alt) {
                return Ok(alt.interface_number());
            }
        }
    }
    Err(Error::DeviceNotFound)
}

/// 查找 CMSIS-DAP v2 的 Bulk 端点地址
pub fn find_bulk_endpoints(handle: &rusb::DeviceHandle<Context>) -> Result<(u8, u8)> {
    let device = handle.device();
    let config = device.config_descriptor(0)?;

    for interface in config.interfaces() {
        for alt in interface.descriptors() {
            if !is_cmsis_dap_candidate(&alt) {
                continue;
            }
            let mut ep_out = 0u8;
            let mut ep_in = 0u8;
            for ep in alt.endpoint_descriptors() {
                if ep.transfer_type() != rusb::TransferType::Bulk {
                    continue;
                }
                match ep.direction() {
                    rusb::Direction::Out => ep_out = ep.address(),
                    rusb::Direction::In => ep_in = ep.address(),
                }
            }
            if ep_out != 0 && ep_in != 0 {
                return Ok((ep_out, ep_in));
            }
        }
    }
    Err(Error::DeviceNotFound)
}

/// 读取 USB 字符串描述符（优先 ASCII，fallback 到 Unicode）
fn read_string_descriptor(
    handle: &rusb::DeviceHandle<Context>,
    idx: Option<u8>,
) -> String {
    match idx {
        Some(i) if i > 0 => {
            // 尝试读 ASCII（厂商/产品字符串通常为 ASCII）
            if let Ok(s) = handle.read_string_descriptor_ascii(i) {
                return s;
            }
            String::new()
        }
        _ => String::new(),
    }
}
