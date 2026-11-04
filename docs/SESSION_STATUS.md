# Aurora — сводка состояния проекта

_Дата: 4 ноября 2026 (обновление 7: kernel/shell split, три фичи, CI-фиксы,
старт веб-версии на wasm-ядре + поиск по внешним каталогам).
Полный отчёт для продолжения в новом окне. Кластерные логи: окно умерло — читать это._

## СРОЧНО ДЛЯ НОВОЙ СЕССИИ

1. **Всё закоммичено и запушено** (HEAD `315011a` + незакоммиченные правки
   текущей сессии — см. «незакоммичено» ниже). Ветка main.
2. **Запуск веба (проверяется прямо сейчас пользователем):**
   `cd frontend && npm run dev` → http://127.0.0.1:1420/#/home
3. **Запуск десктопа**: ДВА окна PowerShell (vite ПЕРВЫМ!):
   - `cd frontend && npm run dev` (порт 1420)
   - `cd apps/desktop && TAURI_DEV="true" cargo run`
   НЕ задавать TAURI_CONFIG.
4. **Валидация — одной командой**: `powershell -ExecutionPolicy Bypass -File check.ps1`
   (fmt + clippy + test + tsc + vite build). Это контракт CLAUDE.md.
5. **Логи падений**: `%APPDATA%\dev.aurora.player\logs\aurora.log.<дата>`.
6. **Токен-бюджет**: пользователь просил экономить. Патчи — python-скриптом из файла.
   Кавычки в commit-месседжах ломают here-string в PowerShell — писать месседж
   в файл и `git commit -F`.

## Что такое Aurora

Локальный аудиоплеер, Rust + Tauri 2 + TypeScript. Идеология (ADR 0002):
плеер, которому верят — bit-perfect это ИЗМЕРЕННОЕ свойство, каждая
трансформация честно репортится. Теперь ЯДРО + ОБОЛОЧКИ (ADR 0005):
`aurora-kernel` без UI-зависимостей; оболочки — Tauri (десктоп) и
веб (wasm, aurora-web). Сторонний плеер может строиться на ядре Aurora.

## Крейты (12)

```
aurora-core     домен; AudioTransformation; integrity контракты
aurora-decoder  Symphonia; open_from_bytes + web://-реестр (wasm);
                waveform_peaks; tempo (BPM: onset+автокорреляция, rustfft)
aurora-dsp      EQ/реверб/gain/dither/ресемплер/speed; live-рекомпиляция
aurora-vst      VST3 изолированно (нативно; НЕ для wasm)
aurora-audio    движок; CPAL-вывод (нативно) / web_pull-мост (wasm);
                WASAPI за feature win-wasapi; VST-заглушки на wasm
aurora-metadata lofty; обложки
aurora-storage  SQLite WAL (нативно; НЕ для wasm)
aurora-library  сканер/поиск/плейлисты (нативно)
aurora-kernel   ЯДРО: PlayerKernel, деки, очередь, DSP, BPM, события
                (EventSubscribers/KernelEvent); storage/vst/watch за cfg
aurora-web      wasm-bindgen WebKernel над ядром; register_file,
                web_pull (Float32Array)
aurora-tauri    тонкий адаптер: команда = один вызов ядра
aurora-app      композиция (нативная)
apps/desktop    Tauri бинарник; externalBin: vst3-host-helper,
                vst3-host-probe (не забыть!), vst-gui-helper
frontend/       модульный TS; invoke.ts — платформенный слой
```

## Фронтенд: двойная оболочка

- **invoke.ts** — единый `invoke(cmd, args)`: Tauri IPC / WebKernel (wasm).
  ВСЕ страницы импортируют только отсюда. Диалоги — lazy dynamic import.
- **audio-bridge.ts** (web) — вывод: kernel ring → web_pull → Web Audio
  (AudioBufferSourceNodes, ~200ms latency, честно). `unlockAudio()` —
  разблокировка AudioContext из user gesture (ОБЯЗАТЕЛЬНО вызывать
  из кликов, иначе браузер глушит звук!).
- **web-shell.ts + pages/home.ts** — веб-оболочка YTM-стиля (ПОКА:
  пользователь ОТМЕНИЛ копирование YTM — «плохо», дизайн вторичен,
  переделывать на собственный стиль ПОСЛЕ починки воспроизведения).
- Десктопный Рамс-UI не тронут (mainDesktop-ветка main.ts).
- **search-service.ts** — поиск по внешним каталогам (iTunes Search API +
  MusicBrainz, без ключей): метаданные + 30-сек превью через ядро.
- wasm-бинарь: `cd crates/aurora-web && wasm-pack build --target web
  --out-dir ..\..\frontend\src\wasm --out-name aurora_web`
  (wasm-opt отключён — крэш на графе Symphonia). ~2.3 МБ.
  ⚠️ ПЕРЕСБИРАТЬ ПОСЛЕ ЛЮБЫХ ПРАВОК aurora-web/aurora-kernel/aurora-audio/
  aurora-decoder! Устаревший бинарь = ошибки «не воспроизводится».

## КРИТИЧНО (текущее состояние, 4 ноября)

**Пользователь reports: ошибки воспроизведения + нет звука + «UI плохо».**
Сделаны три фикса (НЕ закоммичены, в рабочем дереве):
1. wasm-бинарь пересобран (был устаревшим)
2. web-реестр файлов теперь КЛОНИРУЕТ байты (не съедает) — реплеи/лупы ок
3. unlockAudio() при кликах (autoplay policy)
ЖДЁМ подтверждения от пользователя, работает ли звук после обновления
страницы. Если нет — цепочка диагностики: console errors в браузере →
play_track через invoke → kernel → registry (web://id) → web_pull →
AudioContext.state.

**Поиск и добавление треков** (сделано, web-only): на Home в поиск
интегрирован iTunes+MusicBrainz, превью играют через ядро. Полные треки
из чужих библиотек НЕ скачиваются (легально нельзя) — только метаданные
и превью. Пользователь принял.

**Дизайн**: YTM-копирование отменено. Дальше — собственный стиль
(какой — спросить; Braun-orange #E8500A — фирменный).

## Сделано в этой большой сессии (коммиты)

| Коммит | Что |
|---|---|
| 77d5626 | BPM (детектор+миграция+UI), watch библиотеки, плейлисты имп/эксп, ADR 0005 |
| a757ffe | CI: ALSA, windows-future-фикс (wasm-гейт win-wasapi), tauri-cwd, staging |
| ac23cb0 | CI: ctrlc-секция (cfg(unix)!), --exclude aurora-desktop в стейджинге |
| bf189d6 | CLAUDE.md из git (локальные правила) |
| d2fde7f | vst3-host-probe в externalBin (скан VST в установленной копии) |
| 3a56f23 | aurora-kernel собирается под wasm32 |
| b300ed2 | aurora-web (WebKernel) + YTM дизайн-система |
| 315011a | invoke-слой, audio-bridge, web://-файлы, YTM-шелл+Home, живая цепь |

Плюс незакоммичено сейчас: 3 фикса воспроизведения + search-service.ts
+ интеграция поиска в Home. Закоммитить после подтверждения звука.

## Ключевые уроки сессии

1. **wasm-бинарь устаревает молча** — после правок Rust-крейтов
   ПЕРЕСБИРАЙТЕ wasm-pack, иначе «ничего не работает» без ошибок.
2. **AudioContext.resume() только из user gesture** — unlockAudio().
3. **cargo не собирает бины зависимостей**: CI-стейджинг externalBin
   делает `cargo build --workspace --bins --exclude aurora-desktop`
   (исключение: tauri-build desktop-крейта требует эти же файлы).
4. **`[target.unix.dependencies]` — НЕвалидно**: `unix` это cfg, не triple.
   Правильно: `[target.'cfg(unix)'.dependencies]`. Cargo молча линкует
   ничего. Проверяйте целевые секции!
5. **tokio на wasm**: только sync,macros,io-util,rt,time. Workspace-фичи
   мержатся — объявлять lean-набор в workspace, rt/fs — у потребителей.
6. **wasm-opt крэшится на Symphonia-графе** — отключён в aurora-web
   (package.metadata.wasm-pack).
7. **uuid на wasm требует фичу js**.
8. **wasm-bindgen async fn требует wasm-bindgen-futures** (теперь
   безусловная зависимость aurora-web — собирается и нативно).
9. Плейрwright headless-диагностика: inspect.py в Temp конфликтует со
   stdlib — называйте скрипты уникально (aurora_check_page.py).
10. CI джобы падают по РАЗНЫМ причинам на платформах — читать логи
    каждого джоба отдельно (скрипты в Temp: ci_list.py, ci_jobs3.py,
    ci_logs.py — git credential fill для GitHub API).

## НЕ сделано / открыто

1. **Подтверждение звука в браузере** — ждём пользователя.
2. **UI переделка** — свой стиль вместо YTM (после звука).
3. IndexedDB-библиотека (сейчас треки живут до перезагрузки).
4. Деки в вебе (invoke-мост есть, YTM-страницы нет).
5. Деплой (GitHub Pages).
6. Сервисные фичи из плана (§6): lyrics+перевод (Genius), preview-панель
   (поиск сделан, панель нет), smart-queue/радио, share-плейлистов,
   совместное редактирование (CRDT-сервер — большой блок), импорт
   метаданных с VK/Яндекс/Spotify/Сбер/Apple.
7. Десктоп: Verify на живом устройстве; gapless; BPM beat-grid.
8. CI на последних коммитах — проверить статус (api-скрипты).

## План веб-версии

`абстракция(вебверсия)/ПЛАН-ВЕБ-ВЕРСИЯ.md` — полный план (§7 дизайн —
УСТАРЕЛ: YTM отменён; §6 сервисные фичи — актуален). Локально, не в git.

## Инфраструктура

- Валидация: check.ps1 (см. выше).
- Хелперы для externalBin стейджатся в apps/desktop/binaries/
  (gitignored): cargo build --workspace --bins --exclude aurora-desktop,
  потом копии с target-triple суффиксом.
- Установленные VST на машине: CHOWTapeModel, NeuralAmpModeler,
  Dragonfly (в %LOCALAPPDATA%\VST3 и Program Files).
- GitHub: origin = peccatos/Aurora, креды в Windows credential manager.

## Что делать дальше (приоритеты)

1. Подтвердить/починить звук в браузере (фиксы в дереве, wasm свежий).
2. Закоммитить фиксы + search-service (после подтверждения).
3. UI: собственный стиль (спросить пользователя: Braun-orange?).
4. IndexedDB-персистентность.
5. Деплой GitHub Pages.
6. Сервисы §6 по порядку: lyrics → preview-панель → smart-queue →
   share → совместное редактирование → импорт метаданных.

## Правила для агента (CLAUDE.md в корне, но он больше не в git!)

- максимум 2 попытки одной команды, потом смена подхода
- сложные команды → Write в файл-скрипт → запуск одним вызовом
- EOF/syntax error = сломан вызов, НЕ проект
- повторяешься 3-й раз → СТОП и скажи пользователю
- НЕ коммитить без check.ps1
