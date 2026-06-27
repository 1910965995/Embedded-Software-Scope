use clap::{Parser, Subcommand};

use dap_sampler::dap::swd::SwdLink;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "dap-sampler", about = "CMSIS-DAP v2 High-Speed Variable Sampler")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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
        #[arg(long, default_value = "10000")]
        rate: u32,
        /// 采样次数（默认无限次）
        #[arg(long)]
        count: Option<u32>,
        /// 解析为 float 类型
        #[arg(long)]
        float: bool,
    },
    /// 高速流水线采样（P2），支持多变量
    Sample {
        /// 内存地址列表（十六进制，逗号分隔，如 0x20000100,0x20000104）
        #[arg(long, value_delimiter = ',')]
        addresses: Vec<String>,

        /// 采样率 (Hz)
        #[arg(long, default_value = "10000")]
        rate: u32,

        /// 采样次数（默认无限）
        #[arg(long)]
        count: Option<u64>,

        /// 解析为 float 类型（所有变量）
        #[arg(long)]
        float: bool,

        /// 输出 CSV 文件路径（默认输出到 stdout）
        #[arg(long)]
        output: Option<String>,
    },
    /// 启动 GUI 波形显示（P3/P4）
    Gui {
        /// 内存地址列表（十六进制，逗号分隔）
        #[arg(long, value_delimiter = ',')]
        addresses: Vec<String>,

        /// 采样率 (Hz)
        #[arg(long, default_value = "10000")]
        rate: u32,

        /// 采样次数（默认无限）
        #[arg(long)]
        count: Option<u64>,

        /// 变量类型列表（逗号分隔，如 float,int32,uint32）
        ///
        /// 每个地址对应一个类型，默认全部 float。
        /// 支持: float, int32, uint32, int16, uint16, int8, uint8
        #[arg(long, value_delimiter = ',')]
        r#type: Option<Vec<String>>,

        /// ELF 固件文件路径（P4: 启用变量浏览器）
        #[arg(long)]
        elf: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Gui {
        addresses: vec![],
        rate: 20000,
        count: None,
        r#type: None,
        elf: None,
    }) {
        Command::List => cmd_list(),
        Command::Info => cmd_info(),
        Command::Read { address, float } => cmd_read(&address, float),
        Command::Monitor { address, rate, count, float } => cmd_monitor(&address, rate, count, float),
        Command::Sample { addresses, rate, count, float, output } => {
            cmd_sample(&addresses, rate, count, float, output.as_deref())
        }
        Command::Gui { addresses, rate, count, r#type, elf } => {
            cmd_gui(&addresses, rate, count, r#type.as_deref(), elf.as_deref())
        }
    }
}

fn cmd_list() -> anyhow::Result<()> {
    let devices = dap_sampler::usb::device::list_devices()?;
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
    let s = s.trim();
    // 如果以 0x/0X 开头，按十六进制解析
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16)
            .map_err(|e| anyhow::anyhow!("无效的地址格式 '{}': {}", s, e));
    }
    // 否则先尝试十进制（PowerShell 会把 0x0000000c 转成十进制 "12"），
    // 再回退到十六进制
    if let Ok(v) = u32::from_str_radix(s, 10) {
        return Ok(v);
    }
    u32::from_str_radix(s, 16)
        .map_err(|e| anyhow::anyhow!("无效的地址格式 '{}': {}", s, e))
}

fn cmd_sample(
    address_strs: &[String],
    rate: u32,
    count: Option<u64>,
    as_float: bool,
    output_path: Option<&str>,
) -> anyhow::Result<()> {
    use std::io::Write;
    use dap_sampler::pipeline::engine::PipelineEngine;

    // 解析地址列表
    let addresses: Vec<u32> = address_strs
        .iter()
        .map(|s| parse_address(s))
        .collect::<anyhow::Result<Vec<_>>>()?;

    if addresses.is_empty() {
        anyhow::bail!("至少需要指定一个内存地址");
    }
    if addresses.len() > 8 {
        anyhow::bail!("最多支持 8 个变量，当前: {}", addresses.len());
    }

    println!("\u{1f50c} 正在连接 DAP-Link...");
    let mut swd = SwdLink::new()?;
    swd.init()?;

    let (usb, dap) = swd.into_parts();

    let interval_us = 1_000_000.0 / rate as f64;
    println!("\u{1f680} 启动流水线采集:");
    println!("   变量数: {}", addresses.len());
    for (i, &addr) in addresses.iter().enumerate() {
        println!("   [{}] 0x{:08X}", i, addr);
    }
    println!("   采样率: {} Hz (间隔 {:.0} us)", rate, interval_us);
    println!("   模式:   {}", if as_float { "float" } else { "u32 (hex)" });
    if let Some(path) = output_path {
        println!("   输出:   {}", path);
    }

    // 创建引擎并启动
    let usb = Arc::new(usb);
    let engine = PipelineEngine::new(usb, dap, addresses.clone(), rate);
    let handle = engine.start()?;

    // 准备输出
    let mut writer: Box<dyn Write> = match output_path {
        Some(path) => Box::new(std::fs::File::create(path)?),
        None => Box::new(std::io::stdout()),
    };

    // CSV 表头
    write!(writer, "seq,time_ms")?;
    for i in 0..addresses.len() {
        write!(writer, ",var{}", i)?;
    }
    writeln!(writer)?;

    let max_samples = count.unwrap_or(u64::MAX);
    let mut total_collected: u64 = 0;
    let mut sample_buf: Vec<dap_sampler::pipeline::sample::Sample> = (0..1024)
        .map(|_| dap_sampler::pipeline::sample::Sample { seq: 0, timestamp_sec: 0.0, values: vec![] })
        .collect();

    let mut last_progress = 0u64;

    while total_collected < max_samples {
        let n = handle.drain_samples(&mut sample_buf);
        if n == 0 {
            // 暂无数据，短暂等待
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }

        for sample in &sample_buf[..n] {
            let time_ms = sample.timestamp_sec * 1000.0;
            write!(writer, "{},{}", sample.seq, time_ms)?;
            if as_float {
                let floats = sample.as_floats();
                for &v in &floats {
                    write!(writer, ",{}", v)?;
                }
            } else {
                for &v in &sample.values {
                    write!(writer, ",0x{:08X}", v)?;
                }
            }
            writeln!(writer)?;

            total_collected += 1;
            if total_collected >= max_samples {
                break;
            }
        }

        // 进度提示（每秒打印一次）
        if total_collected / (rate as u64) > last_progress {
            last_progress = total_collected / (rate as u64);
            log::info!(
                "已采集 {} 个采样点 ({:.1}s), 缓冲区可用 {}",
                total_collected,
                total_collected as f64 / rate as f64,
                handle.available_samples()
            );
        }
    }

    // 停止流水线
    handle.stop();

    let actual_count = total_collected;
    println!("\n\u{2705} 采集完成: {} 个采样点", actual_count);
    if actual_count > 0 {
        let actual_duration = actual_count as f64 / rate as f64;
        println!("   实际时长: {:.3}s", actual_duration);
    }

    Ok(())
}

fn cmd_gui(
    address_strs: &[String],
    rate: u32,
    count: Option<u64>,
    type_strs: Option<&[String]>,
    elf_path: Option<&str>,
) -> anyhow::Result<()> {
    use dap_sampler::ui::app::DapSamplerApp;
    use dap_sampler::elf::ElfParser;

    // 加载 ELF（如果提供）
    let elf_ctx = if let Some(path) = elf_path {
        println!("📦 正在加载 ELF: {}", path);
        Some(ElfParser::load(path).map_err(|e| anyhow::anyhow!("ELF 加载失败: {}", e))?)
    } else {
        None
    };

    // 手工地址模式（--addresses）：仅解析地址，不在此处连接 USB
    // USB 连接推迟到用户在 GUI 中点击 Start 时再进行
    let manual_addresses: Vec<u32> = if !address_strs.is_empty() {
        let addresses: Vec<u32> = address_strs
            .iter()
            .map(|s| parse_address(s))
            .collect::<anyhow::Result<Vec<_>>>()?;

        if addresses.len() > 8 {
            anyhow::bail!("最多支持 8 个变量，当前: {}", addresses.len());
        }
        addresses
    } else {
        vec![]
    };

    // 解析变量类型列表
    // 未指定 --type 时，手工模式默认 Uint32（原始内存值）
    let manual_types: Vec<dap_sampler::pipeline::sample::ValueType> = if !manual_addresses.is_empty() {
        if let Some(types) = type_strs {
            types
                .iter()
                .map(|s| dap_sampler::pipeline::sample::ValueType::parse(s).unwrap_or(dap_sampler::pipeline::sample::ValueType::Uint32))
                .collect()
        } else {
            vec![dap_sampler::pipeline::sample::ValueType::Uint32; manual_addresses.len()]
        }
    } else {
        vec![]
    };

    // 启动 egui 窗口（此时不连接 USB 设备）
    let app = DapSamplerApp::new(
        manual_addresses,
        address_strs.to_vec(),
        rate,
        count,
        elf_ctx,
        manual_types,
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };

    eframe::run_native(
        "DAP Sampler",
        options,
        Box::new(move |cc| {
            setup_cjk_fonts(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    ).map_err(|e| anyhow::anyhow!("GUI failed: {}", e))?;

    Ok(())
}

/// Load a CJK-compatible font from the Windows system so Chinese text renders correctly.
fn setup_cjk_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Try common Windows CJK fonts in order of preference.
    let candidates: &[(&str, u32)] = &[
        ("C:\\Windows\\Fonts\\msyh.ttc", 0),   // Microsoft YaHei (collection)
        ("C:\\Windows\\Fonts\\simhei.ttf", 0), // SimHei
        ("C:\\Windows\\Fonts\\simsun.ttc", 0),  // SimSun (collection)
        ("C:\\Windows\\Fonts\\Deng.ttf", 0),    // DengXian
    ];

    for (path, index) in candidates {
        if let Ok(data) = std::fs::read(path) {
            fonts.font_data.insert(
                "cjk".to_owned(),
                std::sync::Arc::new(egui::FontData {
                    font: std::borrow::Cow::Owned(data),
                    index: *index,
                    tweak: Default::default(),
                }),
            );
            // Prepend CJK font so it takes priority for CJK glyphs,
            // while the default Latin font still handles ASCII.
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                if let Some(font_list) = fonts.families.get_mut(&family) {
                    font_list.push("cjk".to_owned());
                }
            }
            log::info!("Loaded CJK font from {}", path);
            break;
        }
    }

    ctx.set_fonts(fonts);
}
