# Aurora — архитектурный baseline (исходное ревью)

Architecture baseline v0.1 · проект переименован: Sonora → Aurora

> Примечание: Sonora в тексте ниже = Aurora. Версии зависимостей актуализированы в `Cargo.toml`;
> актуальное имя workspace — Aurora.

Решения зафиксированы:

| Параметр         | Решение                   |
| ---------------- | ------------------------- |
| Платформы        | Windows, Linux, macOS     |
| UI               | Tauri 2 + TypeScript      |
| Audio            | Hybrid: Bit-perfect + DSP |
| Storage          | SQLite                    |
| Backend          | Rust                      |
| Edition          | Rust 2024                 |
| Архитектура      | Cargo Workspace           |
| Основной принцип | Audio-first               |

Я предлагаю строить Sonora как модульный аудиосервис с независимым аудиодвижком, а не как Tauri-приложение, внутри которого случайно оказался плеер.

Это принципиальное различие: аудиодвижок должен работать и тестироваться без запуска графического интерфейса.

## 1. Структура Cargo Workspace

Предлагаю девять отдельных crates, включая приложение и интеграционный слой.

```
sonora/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── .gitignore
├── crates/
│   ├── sonora-core/
│   ├── sonora-audio/
│   ├── sonora-decoder/
│   ├── sonora-dsp/
│   ├── sonora-library/
│   ├── sonora-metadata/
│   ├── sonora-storage/
│   ├── sonora-tauri/
│   └── sonora-app/
│
├── apps/
│   └── desktop/
│       ├── Cargo.toml
│       ├── tauri.conf.json
│       └── src/
│
├── frontend/
│   ├── package.json
│   ├── tsconfig.json
│   └── src/
│
├── migrations/
├── tests/
│   ├── fixtures/
│   ├── integration/
│   └── audio/
│
└── docs/
    ├── ARCHITECTURE.md
    ├── AUDIO_PIPELINE.md
    ├── DSP.md
    └── ADR/
```

### Ответственность crates

| Crate             | Ответственность                     |
| ----------------- | ----------------------------------- |
| `sonora-core`     | Доменные типы, ошибки, контракты    |
| `sonora-audio`    | Playback engine, CPAL, устройства   |
| `sonora-decoder`  | Декодирование аудио                 |
| `sonora-dsp`      | EQ, gain, resampling, DSP           |
| `sonora-library`  | Индексация музыкальной библиотеки   |
| `sonora-metadata` | Теги, обложки, информация о треках  |
| `sonora-storage`  | SQLite, миграции, репозитории       |
| `sonora-tauri`    | Команды и события Tauri             |
| `sonora-app`      | Composition root, запуск приложения |

`sonora-core` не должен зависеть от остальных внутренних crates. Это фундамент архитектуры.

## 2. Версии и политика зависимостей

На 31 октября 2026 года выбираем следующие версии:

| Зависимость | Версия | Назначение       |
| ----------- | ------ | ---------------- |
| CPAL        | 0.18.2 | Audio I/O        |
| Symphonia   | 0.6.1  | Decoder          |
| Rubato      | 5.0.1  | Resampling       |
| Ringbuf     | 0.5.2  | Lock-free buffer |
| Lofty       | 0.25.5 | Metadata         |
| Tauri       | 2.12.5 | Desktop runtime  |
| SQLx        | 0.9.0  | SQLite           |
| Tokio       | 1.x    | Async runtime    |
| Serde       | 1.x    | Serialization    |
| Tracing     | 0.1.x  | Observability    |

Источники версий:


Политика: фиксируем совместимые диапазоны версий в `Cargo.toml`, а конкретное разрешённое дерево зависимостей — в `Cargo.lock`. Не обновляем зависимости автоматически без прохождения CI.

Минимальная версия Rust — 1.90. Это консервативная базовая планка с учётом актуальной экосистемы Tauri.


## 3. Root Cargo.toml

Путь: `Cargo.toml`

Это единая точка управления зависимостями. Версии внутренних библиотек и внешних crates наследуются через `workspace = true`.

```

[workspace]
resolver = "3"
members = [
    "crates/sonora-core",
    "crates/sonora-audio",
    "crates/sonora-decoder",
    "crates/sonora-dsp",
    "crates/sonora-library",
    "crates/sonora-metadata",
    "crates/sonora-storage",
    "crates/sonora-tauri",
    "apps/desktop",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.90"
license = "MIT OR Apache-2.0"

[workspace.dependencies]

# Internal crates
sonora-core = { path = "crates/sonora-core" }
sonora-audio = { path = "crates/sonora-audio" }
sonora-decoder = { path = "crates/sonora-decoder" }
sonora-dsp = { path = "crates/sonora-dsp" }
sonora-library = { path = "crates/sonora-library" }
sonora-metadata = { path = "crates/sonora-metadata" }
sonora-storage = { path = "crates/sonora-storage" }
sonora-tauri = { path = "crates/sonora-tauri" }

# Audio
cpal = "0.18.2"
symphonia = { version = "0.6.1", features = ["all"] }
rubato = "5.0.1"
ringbuf = "0.5.2"
lofty = "0.25.5"

# Async
tokio = { version = "1", features = [
    "rt-multi-thread",
    "macros",
    "sync",
    "time",
    "fs"
] }

# Database
sqlx = { version = "0.9.0", default-features = false, features = [
    "runtime-tokio-rustls",
    "sqlite",
    "migrate",
    "macros",
    "chrono",
    "uuid"
] }

# Tauri
tauri = "2.12.5"
tauri-build = "2.7.4"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Types
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }

# Filesystem
walkdir = "2"

# Errors
thiserror = "2"
anyhow = "1"

# Observability
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = [
    "env-filter",
    "fmt"
] }

# Testing
tempfile = "3"
proptest = "1"
hound = "3.5"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[workspace.lints.clippy]
correctness = "deny"
suspicious = "warn"
perf = "warn"

[profile.dev]
opt-level = 1

[profile.dev.package.symphonia]
opt-level = 2

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = "symbols"

```

Примечание: `symphonia` с `all` выбран для первого этапа ради широкой поддержки форматов. После проверки реального набора музыкальных файлов можно сократить feature set.

## 4. Cargo.toml каждого crate

Все файлы ниже являются самостоятельными manifest-файлами. Их нужно положить по указанным путям.

### 4.1. sonora-core

Путь: `crates/sonora-core/Cargo.toml`

Центральная доменная библиотека. Здесь будут `Track`, `Album`, `Artist`, `PlaybackState`, `AudioFormat`, `PlaybackMode`, общие ошибки и интерфейсы.

```

[package]
name = "sonora-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
serde.workspace = true
uuid.workspace = true
chrono.workspace = true
thiserror.workspace = true

[dev-dependencies]
proptest.workspace = true

[lints]
workspace = true

```

### 4.2. sonora-decoder

Путь: `crates/sonora-decoder/Cargo.toml`

Отвечает исключительно за декодирование источников в PCM. Не знает о Tauri, SQLite и аудиоустройствах.

```

[package]
name = "sonora-decoder"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
sonora-core.workspace = true

symphonia.workspace = true

thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
hound.workspace = true
proptest.workspace = true

[lints]
workspace = true

```

### 4.3. sonora-dsp

Путь: `crates/sonora-dsp/Cargo.toml`

DSP должен быть независимым от аудиоустройств.

```

[package]
name = "sonora-dsp"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
sonora-core.workspace = true
rubato.workspace = true
thiserror.workspace = true

[dev-dependencies]
proptest.workspace = true

[lints]
workspace = true

```

### 4.4. sonora-audio

Путь: `crates/sonora-audio/Cargo.toml`

Главный аудиодвижок.

```

[package]
name = "sonora-audio"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
sonora-core.workspace = true
sonora-decoder.workspace = true
sonora-dsp.workspace = true

cpal.workspace = true
ringbuf.workspace = true

thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
proptest.workspace = true

[lints]
workspace = true

```

### 4.5. sonora-metadata

Путь: `crates/sonora-metadata/Cargo.toml`

Чтение тегов, обложек и технических характеристик файлов.

```

[package]
name = "sonora-metadata"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
sonora-core.workspace = true
lofty.workspace = true

thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true

```

### 4.6. sonora-storage

Путь: `crates/sonora-storage/Cargo.toml`

Единственный crate, который непосредственно работает с SQLite.

```

[package]
name = "sonora-storage"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
sonora-core.workspace = true

sqlx.workspace = true

tokio.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true

```

### 4.7. sonora-library

Путь: `crates/sonora-library/Cargo.toml`

Управление музыкальной библиотекой: сканирование каталогов, индексирование, поиск, обнаружение изменений.

```

[package]
name = "sonora-library"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
sonora-core.workspace = true
sonora-metadata.workspace = true
sonora-storage.workspace = true

walkdir.workspace = true
tokio.workspace = true

thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true

```

### 4.8. sonora-tauri

Путь: `crates/sonora-tauri/Cargo.toml`

Адаптер между Rust-приложением и frontend. Здесь будут Tauri commands, события и управление состоянием приложения.

```

[package]
name = "sonora-tauri"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
sonora-core.workspace = true
sonora-audio.workspace = true
sonora-library.workspace = true
sonora-storage.workspace = true

tauri.workspace = true
tokio.workspace = true

serde.workspace = true
serde_json.workspace = true

thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true

[lints]
workspace = true

```

### 4.9. Desktop application

Путь: `apps/desktop/Cargo.toml`

Фактический исполняемый файл приложения.

```

[package]
name = "sonora-desktop"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[build-dependencies]
tauri-build.workspace = true

[dependencies]
sonora-tauri.workspace = true

tauri.workspace = true

tracing.workspace = true
tracing-subscriber.workspace = true

[lints]
workspace = true

```

## 5. Граф зависимостей

Это важнее, чем количество crates. Нельзя допустить, чтобы аудиодвижок превратился в зависимый от всего монолит.

```
sonora-core -> sonora-decoder, sonora-dsp, sonora-metadata, sonora-storage
sonora-audio -> sonora-core, sonora-decoder, sonora-dsp
sonora-library -> sonora-core, sonora-metadata, sonora-storage
sonora-tauri -> sonora-core, sonora-audio, sonora-library, sonora-storage
sonora-desktop -> sonora-tauri
```

Важные ограничения:

- `sonora-core` ничего не знает о Tauri.
- `sonora-audio` ничего не знает о SQLite.
- `sonora-decoder` не управляет воспроизведением.
- `sonora-dsp` не управляет аудиоустройствами.
- `sonora-storage` не занимается бизнес-логикой.
- Frontend никогда не обращается непосредственно к SQLite.

## 6. Архитектура аудиодвижка

Здесь я хочу внести одно важное уточнение: CPAL сам по себе не гарантирует bit-perfect output. Обычный shared-mode вывод Windows, например, может подвергать сигнал системному микшированию и преобразованию частоты дискретизации.

Поэтому проектируем абстракцию `AudioOutputBackend`, а CPAL используем как первый кроссплатформенный backend. Позже добавим платформенные режимы вывода.

### Два независимых пути обработки

Path A

Bit-perfect capable

Lossless source

Decoder — native PCM representation

Format-compatible buffer

Compatible hardware output

Без software gain, EQ, resampling и mixing. Bit-perfect подтверждается только при совпадении всего signal path.

Path B

DSP mode

Audio Decoder

Float32 conversion

EQ / Gain / ReplayGain

Rubato Resampler

Output Backend

Критически важное техническое решение: не хранить все PCM-данные исключительно в `f32`.

Для DSP это подходящий формат, но для bit-perfect режима нужно сохранять исходную разрядность и представление сэмплов, насколько это поддерживает decoder и output backend.

Иначе мы можем незаметно потерять точность при преобразовании 32-bit integer PCM в `f32`.

### Требования к real-time audio thread

Аудиоколбэк должен:

- Не выделять память.
- Не обращаться к файловой системе.
- Не использовать блокирующие Mutex/RwLock.
- Не выполнять SQL-запросы.
- Не декодировать аудио.
- Не выполнять логирование.
- Не ожидать завершения других потоков.

Вся тяжёлая работа выполняется в worker threads. Для передачи аудиоданных используем заранее выделенный SPSC ring buffer.

# 7. Технический план реализации

Я предлагаю девять этапов. Каждый заканчивается проверяемым результатом, а не просто наличием написанного кода.

## Roadmap Sonora

1. Phase 0 — Workspace Foundation

   Создание workspace, manifests, Tauri bootstrap, CI.

   Exit criteria

   `cargo check --workspace` проходит на трёх ОС.
2. Phase 1 — Domain Core

   Реализация Track, Album, Artist, AudioFormat, PlaybackState, PlaybackMode.

   Exit criteria

   Unit-тесты доменных моделей и сериализации.
3. Phase 2 — Decoder

   Symphonia, чтение FLAC/WAV/MP3/AAC/ALAC/OGG, PCM abstraction.

   Exit criteria

   Декодирование тестовых файлов, корректные sample rate, channels и duration.
4. Phase 3 — Audio Engine

   CPAL, output devices, playback, pause, stop, seek, buffering.

   Exit criteria

   60 минут непрерывного воспроизведения без underrun в контролируемом тесте.
5. Phase 4 — DSP

   Gain, EQ, ReplayGain, resampling, clipping protection.

   Exit criteria

   DSP bypass даёт идентичный PCM; тесты на частотную характеристику и clipping.
6. Phase 5 — Bit-perfect

   Форматное согласование, аппаратные режимы вывода, диагностика преобразований.

   Exit criteria

   Измеряемое подтверждение неизменности PCM на поддерживаемом выходном тракте.
7. Phase 6 — Music Library

   Индексация, метаданные, обложки, SQLite, playlists, поиск.

   Exit criteria

   Повторное сканирование не создаёт дубликатов, изменения файлов обнаруживаются.
8. Phase 7 — Desktop UI

   Tauri 2, frontend, управление воспроизведением, очередь, устройства, настройки.

   Exit criteria

   UI полностью управляет backend через типизированный API.
9. Phase 8 — Production Hardening

   Тестирование, профилирование, CI/CD, installer, crash recovery, обновления.

   Exit criteria

   Воспроизводимые сборки для Windows, Linux и macOS.

## 8. Обязательные инженерные тесты

Я бы не принимал Sonora как профессиональный аудиопродукт без следующих проверок.

| Тест                | Что проверяем                              |
| ------------------- | ------------------------------------------ |
| PCM identity        | Исходные и выходные PCM-сэмплы             |
| Resampler           | Частотная характеристика, aliasing         |
| Clipping            | Отсутствие неконтролируемого переполнения  |
| Underrun            | Потеря данных аудиопотока                  |
| Long playback       | Длительное воспроизведение                 |
| Device switching    | Переключение устройств                     |
| Seek accuracy       | Точность позиционирования                  |
| Decoder robustness  | Повреждённые и неполные файлы              |
| Library consistency | Согласованность SQLite и файловой системы  |
| Recovery            | Восстановление после аварийного завершения |

Для bit-perfect режима нужен не просто тест на отсутствие ошибок, а проверка цифрового выхода с помощью loopback или аппаратного цифрового захвата там, где это возможно.

## 9. Правила реализации

Я предлагаю закрепить следующие архитектурные инварианты в `docs/ARCHITECTURE.md`.

1. Никакой бизнес-логики во frontend.
2. Никаких SQL-запросов из `sonora-audio`.
3. Никакого DSP внутри аудиоколбэка, если он требует динамических аллокаций или блокировок.
4. Любой аудиорежим должен явно сообщать о применённых преобразованиях.
5. Bit-perfect нельзя объявлять активным, если тракт не удовлетворяет его условиям.
6. Каждый crate должен иметь независимые unit-тесты.
7. Все изменения схемы SQLite выполняются через миграции.
8. Playback state должен быть согласованным между backend и frontend.
9. Ошибки аудиоустройств не должны приводить к аварийному завершению приложения.

Особенно рекомендую ввести `AudioPipelineDiagnostics`: объект, который сообщает о текущем формате источника, формате устройства, частоте дискретизации, разрядности и активных DSP-операциях.

Это станет одной из сильных особенностей Sonora.

## 10. Команды первоначальной сборки

Из корневой директории проекта:

```
# Проверить структуру workspace
cargo metadata --no-deps

# Проверить зависимости
cargo check --workspace

# Проверить форматирование
cargo fmt --all -- --check

# Статический анализ
cargo clippy --workspace --all-targets -- -D warnings

# Запустить тесты
cargo test --workspace

# Посмотреть граф зависимостей
cargo tree --workspace

# Проверить уязвимые зависимости
cargo audit
```

Для Tauri потребуются системные библиотеки WebKitGTK и GTK на Linux, а также соответствующие инструменты сборки на каждой целевой ОС.

Документация: [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/?utm_source=chatgpt.com).

## 11. Что я считаю MVP

Не нужно пытаться сделать Spotify, Roon и Audirvāna в первой версии.

# Sonora 0.1.0

First usable release

Release target

Обязательные функции:

- Локальная музыкальная библиотека.
- FLAC, WAV, MP3, AAC, ALAC, OGG.
- Gapless playback.
- Queue management.
- Play / Pause / Stop / Seek.
- Выбор аудиоустройства.
- Информация о sample rate и bit depth.
- Bit-perfect capable mode.
- DSP mode.
- 10-band parametric-capable EQ architecture.
- SQLite persistence.
- Обложки альбомов.
- Поиск и плейлисты.
- Восстановление последней сессии.

# 12. Первый рабочий спринт

Я рекомендую пока вообще не трогать EQ, обложки, плейлисты и красивый интерфейс.

Первая задача — получить архитектурный фундамент.

Sprint 0 — Foundation

Выполнение

# 0%

Создать структуру Sonora Workspace

Добавить root Cargo.toml

Добавить 9 package manifests

Создать build.rs для Tauri

Создать минимальный Tauri bootstrap

Добавить CI для Windows, Linux и macOS

Проверить cargo metadata

Проверить cargo check --workspace

Проверить cargo test --workspace

Создать ARCHITECTURE.md и AUDIO_PIPELINE.md

Скопировать чек-лист

Мой окончательный совет: сначала добиваемся полностью рабочего Rust workspace, затем реализуем decoder и playback engine. Tauri UI подключаем после того, как аудиодвижок сможет воспроизводить тестовые файлы самостоятельно.

И ещё: не называй Sonora bit-perfect-плеером до тех пор, пока не сможешь доказать это измерениями. Маркетинговые заявления без цифровой верификации нам не нужны.

Именно так получится проект, который можно будет предметно защищать перед системными инженерами, а не просто демонстрировать красивое окно с кнопкой Play.
