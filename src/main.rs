use clap::{Parser, Subcommand};

mod error;
mod usb;
mod dap;

use dap::swd::SwdLink;

#[derive(Parser)]
#[command(name = "dap-sampler", about = "CMSIS-DAP v2 High-Speed Variable Sampler")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 列出所有连接的 CMSIS-DAP 设备
    List,
    /// 连接设备并显示调试信息
    Info,
    /// 读取指定内存地址的值
    Read {
        /// 内存地址（十六进制，如 0x20000100）
        address: String,
        /// 解析为 float 类型
        #[arg(long)]
        float: bool,
    },
    /// 连续监视内存地址
    Monitor {
        /// 内存地址（十六进制）
        address: String,
        /// 采样率 (Hz)
        #[arg(long, default_value = "1000")]
        rate: u32,
        /// 采样次数（默认无限次）
        #[arg(long)]
        count: Option<u32>,
        /// 解析为 float 类型
        #[arg(long)]
        float: bool,
    },
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::List => cmd_list(),
        Command::Info => cmd_info(),
        Command::Read { address, float } => cmd_read(&address, float),
        Command::Monitor { address, rate, count, float } => cmd_monitor(&address, rate, count, float),
    }
}

fn cmd_list() -> anyhow::Result<()> {
    let devices = usb::device::list_devices()?;
    if devices.is_empty() {
        println!("⚠️  未找到 CMSIS-DAP v2 设备");
        println!("  请确保 DAP-Link 已通过 USB 连接");
        println!();
        println!("  提示: 设备需支持 CMSIS-DAP v2 (Bulk 传输)");
        println!("  可通过设备管理器检查是否识别为 WinUSB 设备");
    } else {
        println!("🔍 找到 {} 个 CMSIS-DAP v2 设备:\n", devices.len());
        for (i, dev) in devices.iter().enumerate() {
            println!("  [{}] {:04X}:{:04X}  总线={} 地址={}", i, dev.vid, dev.pid, dev.bus_number, dev.address);
            println!("       厂商:   {}", if dev.manufacturer.is_empty() { "(无)" } else { &dev.manufacturer });
            println!("       产品:   {}", if dev.product.is_empty() { "(无)" } else { &dev.product });
            println!("       序列号: {}", if dev.serial.is_empty() { "(无)" } else { &dev.serial });
            println!();
        }
    }
    Ok(())
}

fn cmd_info() -> anyhow::Result<()> {
    println!("🔌 正在连接 DAP-Link...");
    let mut swd = SwdLink::new()?;
    println!("⚡ 正在初始化 SWD...");
    let device_info = swd.init()?;

    println!();
    println!("✅ 连接成功!");
    println!("  目标: {}", device_info.target_info);
    println!("  DPIDR = 0x{:08X}", device_info.dpidr);
    println!("  AP0 IDR = 0x{:08X}", device_info.ap_idr);
    println!();
    println!("  可以使用 read 命令读取内存:");
    println!("  dap-sampler read 0x20000000");

    Ok(())
}

fn cmd_read(address: &str, as_float: bool) -> anyhow::Result<()> {
    let addr = parse_address(address)?;

    println!("🔌 正在连接 DAP-Link...");
    let mut swd = SwdLink::new()?;
    swd.init()?;

    if as_float {
        let value = swd.read_float(addr)?;
        println!("📊 地址 0x{:08X} = {} (float)", addr, value);
    } else {
        let value = swd.read_memory(addr)?;
        println!("📊 地址 0x{:08X} = 0x{:08X} (u32)", addr, value);
        let as_f32 = f32::from_bits(value);
        println!("                   ≈ {} (float)", as_f32);
    }

    Ok(())
}

fn cmd_monitor(address: &str, rate: u32, count: Option<u32>, as_float: bool) -> anyhow::Result<()> {
    let addr = parse_address(address)?;
    let interval_us = (1_000_000.0 / rate as f64) as u64;

    println!("🔌 正在连接 DAP-Link...");
    let mut swd = SwdLink::new()?;
    swd.init()?;

    println!("📈 开始监视 0x{:08X} @ {} Hz (间隔 {} us)", addr, rate, interval_us);
    if as_float {
        println!("   格式: float");
    } else {
        println!("   格式: u32");
    }
    println!("   序号 | 时间 (ms) | 值");
    println!("   {}", "-".repeat(45));

    let max_samples = count.unwrap_or(u32::MAX);
    let start = std::time::Instant::now();

    for seq in 0..max_samples {
        let value_raw = swd.read_memory(addr)?;

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let display_value = if as_float {
            format!("{}", f32::from_bits(value_raw))
        } else {
            format!("0x{:08X} ({})", value_raw, value_raw)
        };

        println!("   {:>6} | {:>8.2} | {}", seq, elapsed_ms, display_value);

        // 控制采样间隔
        let elapsed = start.elapsed();
        let target = std::time::Duration::from_micros(interval_us * (seq + 1) as u64);
        if elapsed < target {
            std::thread::sleep(target - elapsed);
        }
    }

    Ok(())
}

fn parse_address(s: &str) -> anyhow::Result<u32> {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(s, 16)
        .map_err(|e| anyhow::anyhow!("无效的地址格式 '{}': {}", s, e))
}
