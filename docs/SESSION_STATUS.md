# Aurora — сводка состояния проекта

_Дата: 3 ноября 2026 (обновление 6: DJ-деки, big-feature волна, релиз v0.1.0 на GitHub).
Полный отчёт для продолжения в новом окне. Кластерные логи: окно умерло — читать это._

## СРОЧНО ДЛЯ НОВОЙ СЕССИИ

1. **Всё закоммичено и запушено** (`534252c` HEAD, ветка main, дерево чистое).
   **Релиз v0.1.0 опубликован**: https://github.com/peccatos/Aurora/releases/tag/v0.1.0
   (NSIS-инсталлятор Aurora_0.1.0_x64-setup.exe, 5.8 МБ, VST-хелперы внутри).
2. **Запуск в dev**: ДВА окна PowerShell (vite ПЕРВЫМ!):
   - `cd frontend && npm run dev` (порт 1420)
   - `cd apps/desktop && TAURI_DEV="true" cargo run`
   ВАЖНО: НЕ задавать переменную TAURI_CONFIG — ломает build-скрипт.
3. **Файл-лог падений**: `%APPDATA%\dev.aurora.player\logs\aurora.log.<дата>` — при
   любом падении читать последние строки.
4. **Release-сборка**: `cd apps/desktop && npx --prefix ../../frontend tauri build --bundles nsis`.
   LTO-линковка ~15-20 мин (норма). Хелперы VST стейджатся в `apps/desktop/binaries/`
   с target-triple суффиксом (gitignored) — externalBin в tauri.conf.json.
5. **Токен-бюджет**: пользователь просил экономить. Патчи больших файлов — python-скриптом.

## Что такое Aurora

Локальный аудиоплеер, Rust + Tauri 2 + TypeScript, Windows/Linux/macOS.
Идеология (ADR 0002): плеер, которому верят — bit-perfect это ИЗМЕРЕННОЕ свойство,
каждая трансформация честно репортится.

## Крейты (10)

```
aurora-core     — домен; AudioTransformation::ExternalPlugin; integrity контракты
aurora-decoder  — Symphonia; формат из реального буфера; waveform_peaks (пики файла)
aurora-dsp      — EQ/реверб/gain/TPDF-dither/ресемплер/speed; live-рекомпиляция
aurora-vst      — VST3: scan/load/chain + gui (vst-gui-helper) + probe-пресеты +
                  process_with_bypass (A/B на слот)
aurora-audio    — движок: worker→SPSC ring→CPAL; exclusive WASAPI; loopback-verify;
                  spectrum (rustfft, worker-side); A/B loop; LiveVst.bypassed
aurora-metadata — lofty; обложки
aurora-storage  — SQLite WAL; идемпотентный rescan; settings (vst_chain persistence)
aurora-library  — сканер/поиск/плейлисты
aurora-tauri    — команды: движок, очередь, DJ-деки, VST, artwork, media-keys
aurora-app      — composition root; файловое логирование
apps/desktop    — Tauri бинарник + externalBin (VST-хелперы) + global-shortcut
frontend/       — модульный TS: router.ts + pages/ + state.ts + styles/
```

## Фронтенд (модульная архитектура после полного ребилда)

```
main.ts      — каркас: rail-навигация, плеербар, поллинги, media-keys, hotkeys
router.ts    — hash-роуты: #/player #/queue #/library #/decks #/eq #/plugins #/settings
state.ts     — центральное состояние + invoke-обёртки + подписки
types.ts     — все DTO
artwork.ts   — обложки (кэш)
drop.ts      — drag-n-drop файлов из Explorer (Tauri drag-drop события)
pages/       — player (waveform+спектр+loop), queue (dnd), library, decks, eq,
               plugins (bypass, пресеты), settings (bit-perfect)
styles/      — base (токены), components, player, pages
```
Дизайн: швейцарский минимализм × Material × палитра Рамса (Braun-orange #E8500A,
тёплые серые, типографика делает работу).

## Фазы

| Фаза | Статус |
|---|---|
| 0-9 | ✅ (foundation…VST3) |
| 10 DJ-деки | ✅ (951c0aa) |
| Big-feature волна | ✅ (9177336: media keys, waveform, spectrum, loop, bypass, persist, drop) |
| UI-ребилд | ✅ (1a69f83: модульный, роуты, Рамс) |
| Релиз | ✅ v0.1.0 на GitHub |

## Сделано в последних сессиях (кратко)

1. **UI полностью пересобран** (модульно, hash-роуты, швейцарский минимализм,
   палитра Рамса) — по просьбе «не одностраничное, разделены плеер/очередь/плейлисты/эквалайзер/плагины».
2. **Media keys** (global-shortcut plugin): PlayPause/Stop/Next/Prev системно.
   Next/Prev → события во фронтенд (очередь — его состояние).
3. **Waveform seek** (пики файла, кэш 32 трека), **спектр** (16 лог-полос,
   2048-FFT на воркере по сигналу ПОСЛЕ DSP/VST), **A/B loop** (воркер крутит
   через seek), **VST bypass на слот** (плагин жив, выход=вход), **persist VST-цепи**
   (settings), **drag-n-drop из Explorer**, **hotkeys** (Space/стрелки/Ctrl+K).
4. **DJ-деки (DDJ-1000-style)**: AppState.decks: [Deck; 2] — каждый свой
   PlaybackEngine (два shared-WASAPI потока микшируются ОС — честно).
   Equal-power кроссфейдер (cos/sin), channel-фейдеры, 8 хот-кью (Shift+клик=set),
   темпо 0.5-1.5x live, jog-волны. Страница /decks.
5. **Релиз v0.1.0**: NSIS-инсталлятор собран и опубликован (с VST-хелперами,
   категория Music). Починены по пути: beforeBuildCommand (абс. путь),
   TAURI_CONFIG (не задавать!), category enum.

## Ключевые баги последних сессий (уроки)

1. **Vst3HostBuilder::default() = БЕЗ изоляции** — Cargo-фича только компилирует
   код. Иначе NAM ронял плеер через abort (0xc0000409). Всегда
   `.with_process_isolation(true)` на каждой загрузке.
2. **TAURI_CONFIG ломает tauri build-скрипт** («expected value») — не задавать.
3. **tauri CLI резолвит beforeBuildCommand от СВОЕГО cwd** — относительный
   `../../frontend` падал ENOENT; фиксирован абсолютным путём.
4. **NSIS category** — только PascalCase enum (Music), не «AudioVideo».
5. **externalBin** требует target-triple суффикс в имени файла
   (`binaries/vst3-host-helper-x86_64-pc-windows-msvc.exe`).
6. **codeload.github.com отдаёт битые partial-zip'ы** — проверять unzip -t;
   Python urllib с чтением до EOF надёжнее curl -C -.
7. **InnoSetup тихая установка**: `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=<file>`.

## НЕ сделано / открыто

1. **Verify (loopback) на живом устройстве** — пользователь так и не отчитался.
2. **Отчёт по установленному v0.1.0** — пути к хелперам в Program Files, VST-скан
   в установленной копии не проверен живьём.
3. **BPM/beat-sync** для деков — не начат (нужен beat-detection крейт).
4. **Slip-режим, beat-FX** на деках — нет.
5. **Watch библиотеки** (авто-скан папок) — нет.
6. **Плейлисты**: reorder, экспорт/импорт.
7. Gapless: между треками stop+play (щелчок) — не решено архитектурно.
8. Linux/macOS: экслюзив/loopback — только Windows (честно задокументировано).

## Инфраструктура

- Валидация перед коммитом: `cargo fmt --all && cargo clippy --workspace
  --all-targets -- -D warnings && cargo test --workspace` + `cd frontend &&
  npx tsc --noEmit && npm run build`.
- Установленные плагины на этой машине: CHOWTapeModel, NeuralAmpModeler v0.7.13,
  Dragonfly Hall/Plate/Room (в %LOCALAPPDATA%\VST3 и Program Files).
- GitHub: origin = peccatos/Aurora, креды в Windows credential manager
  (git credential fill работает для API-токена).

## Что делать дальше (приоритеты)

1. Отчёт пользователя по установленному релизу (VST-пути, Verify, деки).
2. Beat-sync/BPM для деков (фаза 11).
3. Watch библиотеки.
4. Плейлисты: reorder + экспорт/импорт.

## Правила для агента (CLAUDE.md в корне!)

- максимум 2 попытки одной команды, потом смена подхода
- сложные команды → Write в файл-скрипт → запуск одним вызовом
- EOF/syntax error = сломан вызов, НЕ проект
- повторяешься 3-й раз → СТОП и скажи пользователю
- НЕ коммитить без прогона fmt + clippy + test + tsc + build
