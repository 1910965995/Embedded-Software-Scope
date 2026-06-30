/// Watch 窗口 y_offset / y_scale 集成测试
///
/// 关注点:
/// - 解析合法性 (y_scale != 0,允许负数)
/// - 仿射变换在波形图上的端到端效果
/// - 所有原始样本点都进入缓存 (无降采样保真)
use dap_sampler::pipeline::sample::{Sample, ValueType};
use dap_sampler::ui::waveform::{WaveformDisplayMode, WaveformPanel};
use dap_sampler::ui::watch_panel::WatchPanel;

fn make_sample(seq: u64, val: f32) -> Sample {
    Sample {
        seq,
        timestamp_sec: seq as f64 * 0.001,
        values: vec![val.to_bits()],
    }
}

#[test]
fn watch_sync_default_transform() {
    // 新建 watch 面板并 sync → y_offset=0, y_scale=1
    let mut wp = WatchPanel::new();
    wp.sync_from_channels(&["CH1".to_string()], &[ValueType::Float], &[0x20000000]);
    assert!((wp.entries[0].y_offset - 0.0).abs() < 1e-6);
    assert!((wp.entries[0].y_scale - 1.0).abs() < 1e-6);
}

#[test]
fn transform_end_to_end_through_waveform() {
    // 1) 构造 watch + waveform,共用同一个通道名
    let mut wp = WatchPanel::new();
    wp.sync_from_channels(&["CH1".to_string()], &[ValueType::Float], &[0x20000000]);
    let mut wfp = WaveformPanel::new(
        vec!["CH1".to_string()],
        vec![egui::Color32::RED],
        1000.0,
        vec![ValueType::Float],
        WaveformDisplayMode::Line,
    );

    // 2) 在 watch 上编辑 y_scale → 推 dirty_transforms
    wp.entries[0].y_scale = -2.5;
    wp.dirty_transforms.push(("CH1".to_string(), 0.0, -2.5));

    // 3) App 把 dirty_transforms 转发到 waveform
    for (name, off, sca) in wp.drain_changed_transforms() {
        wfp.set_channel_transform(&name, off, sca);
    }

    // 4) 喂入样本并 rebuild,验证 transform 已生效
    let buffer: Vec<Sample> = (0..10).map(|i| make_sample(i, i as f32 + 1.0)).collect();
    wfp.rebuild_cache(&buffer);
    assert_eq!(wfp.cached_points[0].len(), 10);
    // sample 5 的 raw=6.0,displayed = 6.0 * -2.5 + 0.0 = -15.0
    assert!((wfp.cached_points[0][5].y - (-15.0)).abs() < 1e-6);
}

#[test]
fn no_samples_dropped_across_realistic_buffer_size() {
    // 与实际工具一致: 10k 样本(controls.rs 默认 window size),所有点都必须进 PlotPoint
    let mut wfp = WaveformPanel::new(
        vec!["CH1".to_string(), "CH2".to_string()],
        vec![egui::Color32::RED, egui::Color32::BLUE],
        1000.0,
        vec![ValueType::Float, ValueType::Float],
        WaveformDisplayMode::Line,
    );
    wfp.set_channel_transform("CH1", 50.0, 0.5);
    wfp.set_channel_transform("CH2", -10.0, 2.0);

    let n = 10_000_usize;
    let buffer: Vec<Sample> = (0..n)
        .map(|i| Sample {
            seq: i as u64,
            timestamp_sec: i as f64 * 0.001,
            values: vec![(i as f32).to_bits(), ((i * 2) as f32).to_bits()],
        })
        .collect();
    wfp.rebuild_cache(&buffer);
    assert_eq!(wfp.cached_points[0].len(), n);
    assert_eq!(wfp.cached_points[1].len(), n);

    // 抽查: CH1 末点 raw=9999, displayed=9999*0.5+50 ≈ 5054.5
    let last_ch1 = wfp.cached_points[0][n - 1].y;
    assert!((last_ch1 - (9999.0 * 0.5 + 50.0)).abs() < 1e-2);
    // CH2 末点 raw=19998, displayed=19998*2.0-10 ≈ 39986
    let last_ch2 = wfp.cached_points[1][n - 1].y;
    assert!((last_ch2 - (19998.0 * 2.0 - 10.0)).abs() < 1e-2);
}
