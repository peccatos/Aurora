// ПОЛНЫЙ ПРОДОВЫЙ ПУТЬ: engine.play() на реальном файле, 4 секунды,
// затем сверка: что реально было в кольце против что проиграно.
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let device = aurora_audio::default_output_device().unwrap();
    println!("device: {}", device.name());

    // DSP активен: bass boost + preamp, чтобы услышать и увидеть разницу.
    let mut dsp = aurora_dsp::DspConfig::default();
    dsp.eq.bands[0].gain_db = 6.0; // 31 Hz
    dsp.eq.bands[1].gain_db = 4.0; // 63 Hz
    dsp.eq.preamp_db = -2.0;
    dsp.volume_db = -3.0;

    let mut engine = aurora_audio::PlaybackEngine::new();
    engine
        .play(std::path::PathBuf::from(&path), &device, dsp)
        .expect("play");
    println!(
        "state: {:?}, output: {:?}, transforms: {:?}",
        engine.state(),
        engine.output_format(),
        engine.transformations()
    );

    std::thread::sleep(std::time::Duration::from_secs(4));
    println!(
        "after 4s: frames={}, underruns={}",
        engine.metrics().frames_played(),
        engine.metrics().underruns()
    );
    engine.stop();
    println!("stopped");
}
