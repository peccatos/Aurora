// Точный вызов продового negotiate для реального файла.
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let source = aurora_decoder::AudioSource::open(std::path::Path::new(&path)).unwrap();
    let fmt = source.pcm_format();
    println!("source: {fmt:?}");

    use cpal::traits::{DeviceTrait, HostTrait};
    let device = cpal::default_host().default_output_device().unwrap();

    // Тот же код, что в prod negotiate
    let supported = device.supported_output_configs().unwrap();
    let mut native: Option<String> = None;
    for cfg in supported {
        let min = cfg.min_sample_rate();
        let max = cfg.max_sample_rate();
        let wire_ok = matches!(
            (cfg.sample_format(), fmt.sample_format, fmt.bits_per_sample),
            (
                cpal::SampleFormat::I8,
                aurora_core::SampleFormat::SignedInt,
                8
            ) | (
                cpal::SampleFormat::I16,
                aurora_core::SampleFormat::SignedInt,
                16
            ) | (
                cpal::SampleFormat::I32,
                aurora_core::SampleFormat::SignedInt,
                32
            ) | (
                cpal::SampleFormat::U8,
                aurora_core::SampleFormat::UnsignedInt,
                8
            ) | (
                cpal::SampleFormat::F32,
                aurora_core::SampleFormat::Float,
                32
            ) | (
                cpal::SampleFormat::F64,
                aurora_core::SampleFormat::Float,
                64
            )
        );
        if (min..=max).contains(&fmt.sample_rate) && cfg.channels() == fmt.channels && wire_ok {
            native = Some(format!(
                "{}ch {} {:?}",
                cfg.channels(),
                fmt.sample_rate,
                cfg.sample_format()
            ));
            break;
        }
    }
    match native {
        Some(n) => println!("NEGOTIATED NATIVE: {n}"),
        None => {
            let d = device.default_output_config().unwrap();
            println!(
                "FALLBACK to default: {}ch {} {:?}",
                d.channels(),
                d.sample_rate(),
                d.sample_format()
            );
        }
    }
}
