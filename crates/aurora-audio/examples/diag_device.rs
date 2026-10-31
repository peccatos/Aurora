// Диагностический бинарник: какие форматы реально перечисляет устройство
fn main() {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let device = host.default_output_device().expect("no device");
    println!("Device: {}", device);
    println!(
        "Default config: {:?}",
        device
            .default_output_config()
            .map(|c| (c.channels(), c.sample_rate(), c.sample_format()))
    );
    println!("\nAll supported output configs:");
    for cfg in device.supported_output_configs().expect("sup") {
        println!(
            "  ch={} rate {}-{} fmt={:?}",
            cfg.channels(),
            cfg.min_sample_rate(),
            cfg.max_sample_rate(),
            cfg.sample_format()
        );
    }
}
