# Aurora — сводка состояния проекта

_Дата: 2 ноября 2026 (обновление 5: VST multi-slot+пресеты, 5 плагинов установлены,
Nothing-тема, drag-n-drop очереди).
Полный отчёт для продолжения в новом окне. Кластерные логи: окно умерло — читать это._

## СРОЧНО ДЛЯ НОВОЙ СЕССИИ

1. **Всё закоммичено** (`ce035ca` HEAD, рабочее дерево чистое). Валидация зелёная
   (fmt/clippy/тесты/tsc/build).
2. **Запуск приложения**: ДВА окна PowerShell (не закрывать первое!):
   - `cd frontend && npm run dev` (vite, порт 1420) — должен слушать ПЕРВЫМ
   - `cd apps/desktop && TAURI_DEV="true" cargo run` — вторым
   Если окно приложения показывает «127.0.0.1 отказано в подключении» — умер vite:
   поднять его первым, потом перезапустить приложение.
3. **Файл-лог падений**: `%APPDATA%\dev.aurora.player\logs\aurora.log.<дата>` — при любом
   падении читать последние строки, там причина. Консоль умирает вместе с процессом,
   файл — нет.
4. **Токен-бюджет**: пользователь просил экономить. Меньше итераций, точные якоря,
   патчи через python-файл для массовых правок.

## Что такое Aurora

Локальный аудиоплеер, Rust + Tauri 2 + TypeScript, Windows/Linux/macOS.
Идеология (ADR 0002): плеер, которому верят — bit-perfect это ИЗМЕРЕННОЕ свойство,
каждая трансформация честно репортится.

## Крейты (10)

```
aurora-core     — домен; AudioTransformation::ExternalPlugin
aurora-decoder  — Symphonia; формат из реального буфера (F32 у MP3/FLAC!)
aurora-dsp      — EQ/реверб/gain/TPDF-dither/ресемплер/speed; live-рекомпиляция
aurora-vst      — VST3-хостинг (Phase 9): scan/load/chain + gui + vst-gui-helper
                  + probe_params/пресеты (probe-инстансы)
aurora-audio    — движок: worker→SPSC ring→CPAL; exclusive WASAPI; loopback;
                  VST-цепь на decode-worker'е; restart_for_dsp;
                  param_slot для multi-slot GUI-параметров
aurora-metadata — lofty; read_cover_art() — обложки из тегов
aurora-storage  — SQLite WAL, идемпотентный rescan (контент-хеш)
aurora-library  — сканер/поиск/плейлисты
aurora-tauri    — команды (движок, очередь, библиотека, VST, artwork, exclusive)
aurora-app      — composition root + файловое логирование (tracing-appender)
apps/desktop    — Tauri бинарник
frontend/       — весь UI в main.ts (~1700 строк) + style.css (Nothing-тема)
```

## Фазы

| Фаза | Статус |
|---|---|
| 0-8 | ✅ (foundation…hardening) |
| 9 VST3 | ✅ + полировка ce035ca (multi-slot, пресеты, generic-панель) |
| Редизайн | ✅ Nothing-тема (чёрный/белый/красный #D71921, dot-matrix) |

## Сделано этой сессией (обновление 5)

1. **VST multi-slot GUI**: параметры редактора теперь идут в СВОЙ слот
   (LiveVst.param_slot, apply_params_to) — любой слот цепи, не только первый.
2. **Generic-панель параметров**: list_vst_params/set_vst_param — слайдеры всех
   параметров плагина прямо в UI (без открытия редактора; probe-инстанс, живые
   значения через param_slot).
3. **Пресеты плагинов**: save_vst_preset/load_vst_preset/list_vst_presets —
   JSON-blob'ы в %APPDATA%\dev.aurora.player\vst-presets\<plugin>\, uid-проверка
   при загрузке; после загрузки значения пушатся в живую цепь.
4. **Плагины установлены (5)**: CHOWTapeModel (%LOCALAPPDATA%\VST3),
   NeuralAmpModeler v0.7.13 (Program Files\Common Files\VST3, InnoSetup
   /VERYSILENT), Dragonfly Hall/Plate/Room Reverb 3.2.10 (%LOCALAPPDATA%\VST3).
5. **Nothing-тема** (по запросу «стиль Nothing Phone/Player»): чёрный #000 фон,
   белые карточки, ЕДИНСТВЕННЫЙ красный акцент #D71921 (прогресс, play-кнопка,
   активный nav, current-трек), monospace-«dot-matrix» лейблы (letter-spacing
   0.22-0.34em), плоские поверхности без свечения, скругления 10-18px.
6. **Drag-n-drop очереди**: pointer-based reorder через drag-handle (⋮⋮),
   queueCurrent следует за ТРЕКОМ (не позицией), синк через set_queue.

## Ключевые баги этой сессии (уроки)

1. codeload.github.com может отдавать частичный zip без ошибок curl — проверять
   `unzip -t`; Python urllib с чтением до EOF надёжнее curl -C -.
2. Dragonfly: репо michaelwillis/dragonfly-reverb (не DragonflyReverb — 404).
3. InnoSetup-инсталляторы ставятся молча: `/VERYSILENT /SUPPRESSMSGBOXES
   /NORESTART /LOG=<file>`.
4. TAL-software недоступен с этой сети (CF-блок), зеркала не работают.

## НЕ сделано / открыто

1. **Verify (loopback) на живом устройстве** — пользователь не отчитался.
   Экран: Settings → «◎ Bit-perfect» (WASAPI exclusive) → «✓ Verify» (loopback
   2s, вердикт byte-identical %). Ожидается отчёт пользователя.
2. **Плейлисты**: drag-n-drop внутри плейлистов (drag-n-drop очереди готов),
   экспорт/импорт.
3. **Watch библиотеки** — нет.
4. Seek: старый буфер доигрывает ~1MB перед прыжком; реверб только стерео.
5. Внешний drag-n-drop файлов из Explorer в окно — не сделан.

## Критичные хвосты инфраструктуры

- Приложение+vite запускаются в ОТДЕЛЬНЫХ окнах PowerShell (переживают сессии
  агента). Пользователь знает: «не закрывать окно с npm run dev».
- Команда полного прогона: `cargo fmt --all && cargo clippy --workspace
  --all-targets -- -D warnings && cargo test --workspace` + `cd frontend &&
  npx tsc --noEmit && npm run build`.

## Что делать дальше (приоритеты)

1. Отчёт пользователя по Verify (loopback) на живом устройстве.
2. Внешний drag-n-drop (файлы из Explorer → очередь).
3. Плейлисты: reorder + экспорт/импорт.
4. Watch библиотеки.

## Правила для агента (CLAUDE.md в корне!)

- максимум 2 попытки одной команды, потом смена подхода
- сложные команды → Write в файл-скрипт → запуск одним вызовом
- EOF/syntax error = сломан вызов, НЕ проект
- повторяешься 3-й раз → СТОП и скажи пользователю
- НЕ коммитить без прогона fmt + clippy + test + tsc + build
