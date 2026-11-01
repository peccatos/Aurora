// Проверка фактического rate WAV-файла против того, что играет движок
fn main() {
    let path = "C:/Users/madei/Music/iam - sp.iii.wav";
    let source = aurora_decoder::AudioSource::open(std::path::Path::new(path)).unwrap();
    println!("source: {:?}", source.pcm_format());
    println!("info: {:?}", source.info());
}
