# Aurora — сводка состояния проекта

_Дата: 1 ноября 2026. Полный отчёт для продолжения в новом окне._

## Что такое Aurora

Локальный аудиоплеер уровня «аудиофильский движок с честным трактом».
Rust + Tauri 2 + TypeScript. Windows/Linux/macOS.

**Ключевая идеология (ADR 0002):** плеер, которому верят. Bit-perfect — это
*измеренное* свойство тракта, а не маркетинговое заявление. Любая активная
трансформация честно репортится; непроверяемый тракт = `unverified`, не
«bit-perfect». VST3-хостинг запланирован как Phase 9 (post-1.0), VST2
отвергнут навсегда (закрытый SDK).

## Архитектура (Cargo workspace, 9 крейтов)

```
aurora-core     — домен: PcmFormat (SampleFormat+ChannelLayout!), TrackId
                  (контент-хеш), AudioTransformation, OutputIntegrity,
                  AudioFormat (lossless/lossy), EQ/Reverb/DSP конфиги
aurora-decoder  — Symphonia. ВАЖНО: формат берётся из РЕАЛЬНОГО
                  декодированного буфера (F32 для MP3/FLAC/Vorbis!),
                  не из параметров кодека — это фиксило белый шум.
                  Seek (accurate mode), bounded skip corrupt packets
aurora-dsp      — biquad EQ (f64, Audio EQ Cookbook), 10-band parametric,
                  Freeverb reverb (8 comb + 4 allpass), Gain/ReplayGain,
                  TPDF dither, rubato FFT resampler, speed (0.5–2.0x),
                  DspPipeline: EQ -> Reverb -> ReplayGain -> volume ->
                  resample. Bypass identity бит-точный, всё серилизуемо
aurora-audio    — движок: decoder worker -> SPSC ring (1MB) -> CPAL
                  callback (ноль аллокаций/блокировок). Live-DSP:
                  version-счётчик, worker перекомпилирует pipeline на
                  лету + DRAIN старого буфера (мгновенное применение
                  пресетов). Пауза стопит worker (иначе back-pressure
                  пинит его навсегда). WASAPI exclusive probe
                  (exclusive.rs), loopback-verification математика
aurora-tauri    — команды: устройства, play/pause/resume/stop/seek/
                  next/prev/queue/shuffle, get_dsp/set_dsp (live!),
                  playback_status (integrity+duration+transforms),
                  probe_exclusive
aurora-storage  — заглушка (Phase 6)
aurora-library  — заглушка (Phase 6)
aurora-app      — composition root
apps/desktop    — Tauri бинарник, capabilities (core+dialog:allow-open)
frontend/       — TypeScript+Vite. UI: плеер с очередью, seek-бар,
                  10-band EQ с 8 пресетами, reverb+speed панель
```

## Фазы (по ROADMAP.md)

| Фаза | Статус |
|---|---|
| Sprint 0 Foundation | ✅ CI три ОС + audit + MSRV 1.90 |
| 1 Domain core | ✅ property-тесты |
| 2 Decoder | ✅ bit-exact roundtrip |
| 3 Playback | ✅ реальный звук, 0 underruns |
| 4 DSP | ✅ live EQ/реверб/пресеты/speed |
| 5 Output integrity | ✅ честная классификация + WASAPI
  exclusive probe (Realtek: 16/24-bit YES) + loopback-математика.
  Полный exclusive-стрим и физ. loopback — следующий слой |
| 6 Library+SQLite | ⬜ СЛЕДУЮЩАЯ: база треков, сканирование,
  метаданные (lofty), плейлисты, корректные имена |
| 7 Desktop product | ⬜ |
| 8 Hardening | ⬜ |
| 9 VST3 host (post-1.0) | задокументирован в ROADMAP |

## Ключевые баги, которые мы поймали и починили (уроки!)

1. **Белый шум #1**: negotiate брал sample format первый из enum (U8!),
   worker пушал S16-байты → переинтерпретация wire-формата. Фикс:
   `wire_compatible()` — совпадение семейства И битовой ширины.
2. **Белый шум #2 (главный)**: symphonia декодит MP3/FLAC в **F32**-буферы,
   а формат брался из параметров кодека (None → дефолт S16). PcmChunk нёс
   F32-байты с меткой S16. Фикс: формат из фактического варианта буфера,
   probe-чанк буферизуется. **Урок: формат бери из реального буфера.**
3. **Задержка пресетов**: ring 8MB = ~46 сек старой обработки в буфере.
   Фикс: ring 1MB + drain-флаг при рекомпиляции.
4. **Worker-пиннинг на паузе**: back-pressure цикл крутился вечно (ring
   полон, колбэк стоит), DSP/seek мертвы. Фикс: paused-флаг + bounded waits.
5. **Play вместо Resume**: кнопка Play рестартила трек после паузы.

## Фронтенд-структура (frontend/src/main.ts)

- `PlaybackStatus {state, sample_rate, channels, bits, transformations,
  integrity, duration_s, frames_played, underruns}`
- `DspConfig {eq:{bands[10],preamp_db}, volume_db, replaygain, reverb,
  speed}` — всё через `get_dsp`/`set_dsp` (применяется live)
- Очередь: `set_queue`/`next_track`/`prev_track`/`play_queue_index`,
  shuffle = Fisher-Yates на фронте
- Пресеты EQ: EQ_PRESETS (flat/bass/vocal/rock/jazz/electronic/classical/
  night); Speed кнопки: Slowed (0.8x) / Speed Up (1.25x)
- Poll статуса 500ms, позиция 400ms, авто-advance в конце трека

## Команды запуска/валидации

```bash
# Валидация
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace

# Запуск (vite должен слушать 127.0.0.1:1420)
cd frontend && npm run dev      # отдельный терминал
cd apps/desktop && TAURI_DEV="true" cargo run

# Диагностика движка на реальном файле
cargo run --example diag_engine -p aurora-audio -- "C:\...\трек.mp3"
```

## Правила для агента (CLAUDE.md в корне!)

После инцидента со сжиганием ~100М токенов на повторах падающих команд —
в корне проекта лежит **CLAUDE.md** с железными правилами:
- максимум 2 попытки одной команды, потом смена подхода
- сложные команды → Write в файл-скрипт → запуск одним вызовом
- EOF/syntax error = сломан вызов, НЕ проект
- повторяешься 3-й раз → СТОП и скажи пользователю

## Что делать дальше (приоритеты пользователя)

Пользователь просит «идеальную фазу 4+»: реверб, EQ, перемотки, shuffle,
база треков, плейлисты, корректные имена, VST.

1. **Phase 6: Library + SQLite** (следующая по плану): sqlx 0.9 (уже в
   deps, features: runtime-tokio, tls-rustls, sqlite...), сканер папки
   (walkdir), метаданные через lofty (уже в deps), TrackId контент-хеш,
   плейлисты, поиск. Это даст «базу треков» и «корректные имена».
2. Полный WASAPI exclusive-стрим (поверх probe из Phase 5)
3. Физический loopback-захват (верификация Verified)
4. VST3-хост (Phase 9, out-of-process изоляция)

## Технический долг / известные границы

- Seek: старый буфер доигрывает ~1MB (секунды) перед прыжком —
  приемлемо, но можно дропать ring при seek
- ReplayGain: режимы есть, чтение тегов — с Phase 6 (lofty)
- Реверб только стерео (моно проходит мимо)
- Speed + устройство-не-совпало-по-rate: комбинации покрыты тестами,
  но живых прогонов мало
- Библиотека/хранение — пустые заглушки
