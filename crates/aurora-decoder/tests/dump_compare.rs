// Сравнение: copy_bytes_to_vec_interleaved (наш путь) против
// copy_to_slice_interleaved в f32 (эталон из примера symphonia).
// Если эталон звучит как музыка, а наш путь — мусор, баг в byte-пути.
#[test]
fn byte_copy_matches_typed_copy() {
    // Генерируем WAV с чистой синусоидой, декодируем, сравниваем оба пути.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sine.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for frame in 0..1000 {
        for ch in 0..2u32 {
            let phase =
                frame as f64 * 440.0 / 44_100.0 + f64::from(ch) * std::f64::consts::FRAC_PI_2;
            let s = (phase.sin() * 20_000.0).round() as i16;
            writer.write_sample(s).unwrap();
        }
    }
    writer.finalize().unwrap();

    // Открываем напрямую через symphonia, как это делает наш AudioSource
    let file = std::fs::File::open(&path).unwrap();
    let mss = symphonia::core::io::MediaSourceStream::new(Box::new(file), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &Default::default(),
            mss,
            Default::default(),
            Default::default(),
        )
        .unwrap();
    let track = format
        .default_track(symphonia::core::formats::TrackType::Audio)
        .unwrap();
    let params = track
        .codec_params
        .as_ref()
        .unwrap()
        .audio()
        .unwrap()
        .clone();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &Default::default())
        .unwrap();
    let track_id = track.id;

    let packet = format.next_packet().unwrap().unwrap();
    assert_eq!(packet.track_id, track_id);
    let decoded = decoder.decode(&packet).unwrap();

    // Путь А (наш): copy_bytes_to_vec_interleaved
    let mut bytes_path = Vec::new();
    decoded.copy_bytes_to_vec_interleaved(&mut bytes_path);

    // Путь Б (эталон): copy_to_slice_interleaved в i16
    let mut typed = vec![0i16; decoded.samples_interleaved()];
    decoded.copy_to_slice_interleaved(&mut typed);
    let mut typed_bytes = Vec::new();
    for s in &typed {
        typed_bytes.extend_from_slice(&s.to_le_bytes());
    }

    println!(
        "bytes_path len: {}, typed len: {}",
        bytes_path.len(),
        typed_bytes.len()
    );
    println!(
        "bytes_path first 16: {:02X?}",
        &bytes_path[..16.min(bytes_path.len())]
    );
    println!(
        "typed  first 16: {:02X?}",
        &typed_bytes[..16.min(typed_bytes.len())]
    );

    assert_eq!(bytes_path, typed_bytes, "byte path must match typed path");
}
