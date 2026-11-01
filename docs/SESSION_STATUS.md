# Aurora — сводка состояния проекта

_Дата: 1 ноября 2026 (обновлено после Phase 8). Полный отчёт для продолжения в новом окне._

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
aurora-core     — домен: PcmFormat, TrackId (контент-хеш), Track (+genre/
                  year/track_no/ReplayGain, display_title/display_line),
                  Playlist, AudioTransformation, OutputIntegrity
aurora-decoder  — Symphonia; формат из РЕАЛЬНОГО буфера (F32 для MP3/FLAC!)
aurora-dsp      — EQ (10-band), Freeverb, Gain/ReplayGain, TPDF dither,
                  resampler, speed; DspPipeline, live-рекомпиляция
aurora-audio    — движок: worker -> SPSC ring (1MB) -> CPAL; live-DSP,
                  пауза/seek; play_with_replaygain(теги из библиотеки)
aurora-metadata — lofty: title/artist/album/genre/year/track, duration,
                  ReplayGain-парсер ("−6.5 dB" / "−6.5", мусор→None)
aurora-storage  — SQLite (sqlx 0.9, WAL, миграции), upsert по TrackId
                  (идемпотентность rescan!), search, prune, плейлисты,
                  blake3 content_digest
aurora-library  — LibraryService: scan_folder (walkdir→digest→метаданные→
                  upsert→prune, honest ScanSummary), search, async-мьютекс
                  на сканы, spawn_blocking на метаданные
aurora-tauri    — команды: движок + очередь (QueueTrack с RG-тегами и
                  display-именами) + 10 библиотечных команд
aurora-app      — composition root
apps/desktop    — Tauri бинарник
frontend/       — плеер + EQ + reverb/speed + ПАНЕЛЬ LIBRARY (скан, поиск,
                  плейлисты, корректные имена)
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
| 6 Library+SQLite | ✅ СВЕРШИЛОСЬ: сканер, метаданные (lofty),
  идемпотентный rescan (контент-хеш → TrackId), поиск, плейлисты,
  корректные имена (display_title/display_line), ReplayGain-теги до DSP.
  ADR 0003 написан |
| 7 Desktop product | ✅ автозагрузка библиотеки из app-data,
  персистентный выбор устройства (SQLite settings + fallback при
  отключённом устройстве), rescan всех корней, queue UI панель,
  add-to-queue, выбор из библиотеки |
| 8 Hardening | ✅ crash containment (worker panics ловятся
  catch_unwind, процесс жив), device-loss recovery (stream_error +
  recover_playback + кнопка Recover & Replay), graceful shutdown (WAL
  checkpoint при закрытии окна), перф-тесты DSP (10x realtime floor,
  44.1k/96k), release workflow (draft release, бандлы 3 ОС),
  PLATFORM_LIMITATIONS.md |
| 9 VST3 host (post-1.0) | задокументирован в ROADMAP |

## Ключевые баги, которые мы поймали и починили (уроки!)

1. **Белый шум #1**: negotiate брал sample format первый из enum (U8!).
   Фикс: `wire_compatible()` — совпадение семейства И битовой ширины.
2. **Белый шум #2 (главный)**: symphonia декодит MP3/FLAC в **F32**-буферы,
   а формат брался из параметров кодека (None → дефолт S16). **Урок:
   формат бери из реального буфера.**
3. **Задержка пресетов**: ring 8MB = ~46 сек старой обработки. Фикс:
   ring 1MB + drain-флаг.
4. **Worker-пиннинг на паузе**: back-pressure цикл крутился вечно. Фикс:
   paused-флаг + bounded waits.
5. **Play вместо Resume**.
6. **Два одинаковых WAV в тесте сканера = один трек**: контент-хеш
   действительно делает то, что обещает — тест-файлы должны отличаться.

## Фронтенд-структура (frontend/src/main.ts)

- Плеер: PlaybackStatus, DspConfig (get_dsp/set_dsp live), очередь
  `QueueTrack[]` (path + title/artist + RG-теги), seek, shuffle
- **Библиотека (новое)**: `open_library` (диалог .db) → `scan_folder`
  (диалог папки, honest summary) → `list_tracks`/`search_library`
  (debounce 250ms) → чекбоксы → Enter = играть выбранное; плейлисты
  (создать/открыть/удалить), `display` = "Artist — Title" со
  stem-fallback
- Пресеты EQ (8), Speed кнопки (Slowed 0.8x / Speed Up 1.25x)
- Poll статуса 500ms, позиция 400ms, авто-advance

## Команды запуска/валидации

```bash
# Валидация (всё зелёное на коммите Phase 6)
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cd frontend && npx tsc --noEmit && npm run build

# Запуск (vite должен слушать 127.0.0.1:1420)
cd frontend && npm run dev      # отдельный терминал
cd apps/desktop && TAURI_DEV="true" cargo run
```

## Правила для агента (CLAUDE.md в корне!)

- максимум 2 попытки одной команды, потом смена подхода
- сложные команды → Write в файл-скрипт → запуск одним вызовом
- EOF/syntax error = сломан вызов, НЕ проект
- повторяешься 3-й раз → СТОП и скажи пользователю

## Что делать дальше (приоритеты)

1. Полный WASAPI exclusive-стрим (поверх probe из Phase 5) — поднимет
   honest-статус с unverified до проверяемого
3. Физический loopback-захват (верификация Verified)
4. VST3-хост (Phase 9, out-of-process изоляция)

## Технический долг / известные границы

- Seek: старый буфер доигрывает ~1MB перед прыжком
- Реверб только стерео (моно проходит мимо)
- ReplayGain: теги читаются и применяются; preamp/clipping-guard —
  можно улучшить
- Библиотека: нет watch (изменения папки на лету), нет обложек
- Плейлисты: нет drag-n-drop переупорядочивания (только порядок
  сохранения), нет экспорта/импорта
- Устройство применяется к следующему треку (не hot-swap текущего стрима)
- Настройки хранятся в library.db (в settings), отдельного конфига нет
- Подпись релизов требует секретов (TAURI_SIGNING_PRIVATE_KEY, Apple
  notarization) — без них артефакты unsigned
- Frontend-валидация (tsc+build) не в CI-джобе rust (есть в release.yml)
