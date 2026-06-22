# DAP Sampler - 高速变量采样工具

基于 CMSIS-DAP v2 协议，通过 DAP-Link 仿真器实时读取 ARM Cortex-M 内存变量。

## P1 阶段功能

```
dap-sampler list            列出所有 CMSIS-DAP v2 设备
dap-sampler info            连接设备并显示调试信息
dap-sampler read 0x20000100  读取 32 位内存值
dap-sampler read 0x20000100 --float  读取并解析为 float
dap-sampler monitor 0x20000100 --rate 10000  连续采样
```

## 硬件要求

- DAP-Link 仿真器（支持 CMSIS-DAP v2 / Bulk 传输）
- 目标 ARM Cortex-M MCU（如 STM32/GD32/AT32/AC7840X 等）

## 快速开始

```bash
cd dap_sampler

# 列出设备
cargo run -- list

# 连接并初始化 SWD
cargo run -- info

# 读取内存
cargo run -- read 0x20000000
cargo run -- read 0x20000000 --float

# 连续采样（10kHz）
cargo run -- monitor 0x20000000 --rate 10000 --count 100
```

## 项目结构

```
src/
├── main.rs              CLI 入口（list/info/read/monitor）
├── error.rs             错误类型定义
├── usb/
│   ├── device.rs        USB 设备发现（VID/PID + 字符串匹配）
│   └── transfer.rs      Bulk 传输封装
└── dap/
    ├── commands.rs       DAP 命令常量、寄存器定义、请求编码
    ├── protocol.rs       CMSIS-DAP 协议（4 条命令的打包/解析）
    └── swd.rs            SWD 操作（初始化/内存读取）
```

## 协议栈

```
┌──────────────────────┐
│    CLI (clap)         │
├──────────────────────┤
│    SwdLink            │  SWD 高级操作
├──────────────────────┤
│    DapProtocol        │  CMSIS-DAP 协议
├──────────────────────┤
│    BulkTransfer       │  USB Bulk 传输
├──────────────────────┤
│    rusb (libusb)      │  USB 底层
├──────────────────────┤
│    DAP-Link 仿真器     │
├──────────────────────┤
│    ARM Cortex-M 目标   │
└──────────────────────┘
```

## 注意事项

1. **CMSIS-DAP v2:** 需要 DAP-Link 支持 Bulk 传输（v2），不支持 v1 (HID)
2. **驱动:** Windows 下需要 WinUSB 驱动（可通过 Zadig 安装）
3. **目标电源:** 目标 MCU 需要上电且未进入低功耗模式
4. **读保护:** 如果目标开启了读保护（RDP），SWD 可能无法访问
