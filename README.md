# Aurora

Cross-platform, local-first music player built with Rust, Tauri 2 and TypeScript.

## Current status

This repository is an **architecture bootstrap**, not a finished audio player.
The workspace boundaries, desktop shell, frontend scaffold, CI and initial domain
types are present. Decoder, DSP, playback, library scanning and persistence
implementations are intentionally staged for later milestones.

## Architecture

- `aurora-core`: domain types and shared contracts.
- `aurora-decoder`: source decoding into PCM.
- `aurora-dsp`: optional signal processing and resampling.
- `aurora-audio`: device output and playback orchestration.
- `aurora-metadata`: tags and embedded artwork extraction.
- `aurora-storage`: SQLite persistence and migrations.
- `aurora-library`: filesystem indexing and library use cases.
- `aurora-tauri`: Tauri commands/events adapter.
- `aurora-app`: application composition root.
- `apps/desktop`: executable Tauri desktop application.

## Requirements

- Stable Rust toolchain (Edition 2024).
- Node.js LTS and npm.
- Tauri platform prerequisites: <https://v2.tauri.app/start/prerequisites/>.

Linux requires the system packages listed in the Tauri prerequisites.
Windows requires the Microsoft C++ Build Tools and WebView2.
macOS requires Xcode Command Line Tools.

## Run

```bash
npm --prefix frontend install
npm --prefix frontend run tauri:dev
```

If the Tauri CLI is not installed locally, use:

```bash
cd frontend
npm install
npm run tauri:dev
```

## Validate Rust workspace

```bash
cargo metadata --no-deps
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Cargo resolves compatible dependency releases and writes the exact resolution
to `Cargo.lock` on the first successful build. Commit `Cargo.lock` for this
desktop application. Do not treat a successful `cargo check` as proof of
real-time safety or bit-perfect output.

## Audio integrity policy

Aurora must never claim bit-perfect output based only on the source file format
or the selected output device. The complete path (decoder representation,
sample conversion, DSP state, backend mode, OS mixer and hardware path) must
be verified. When any transformation is active or cannot be ruled out, report
the output as transformed or unverified.

The real-time audio callback must not perform filesystem or database I/O,
blocking synchronization, dynamic allocation, decoding, or logging.
