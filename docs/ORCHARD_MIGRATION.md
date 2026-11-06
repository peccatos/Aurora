# Orchard Studio migration inventory

> Source snapshot: `coderroleggg/orchard-studio` default branch at commit `6ec0e0130ad19492fe7b2a90b27ad63a1c8640a6` (2026-11-05).
> This is an inventory and migration boundary, not a claim that the complete FL-style DAW can be dropped into Aurora unchanged.

## Migration contract

Aurora keeps `aurora-dsp` as the backend authority. Portable processors may be reimplemented in Rust and exposed through the shared desktop/WASM pipeline only when their state, tails, channel assumptions, reset behavior, and transformation reporting are explicit. Orchard AudioWorklet, React editors, project model, and host routing are not runtime dependencies of Aurora.

Every migrated processor must provide:

- typed serde-defaulted parameters with finite/clamped normalization;
- stateful interleaved `f32` processing with deterministic reset/seek behavior;
- explicit tail/flush semantics for delay, reverb, convolution, and lookahead;
- honest `AudioTransformation` reporting and output-format accounting;
- exact bypass behavior where the neutral configuration is mathematically an identity;
- desktop and WASM behavior parity, or an explicit desktop-only gate;
- unit, property, finiteness, and throughput tests before UI exposure.

## Orchard scope

- **123 effect plugin modules** under `src/engine/effects/plugins/`, plus ParametricEQ2.
- **44 generator/instrument research modules** under `docs/research/gen/`.
- **88 effect research specifications** under `docs/research/fx/`.
- Core DAW systems: AudioWorklet host, transport, sequencer, playlist/audio clips, sampler, mixer/routing/PDC, automation, recording, MIDI Out, Patcher, project I/O, and visual/editor tooling.

## Status classes

| Class | Aurora treatment | Examples |
|---|---|---|
| A — portable stream DSP | Rust `aurora-dsp` stage; desktop/WASM | balance, phase inversion, stereo width, soft clipper, filters, EQ, compressor, limiter, delay, chorus |
| B — portable with architecture | Rust DSP plus state/tail/latency/analysis contracts | convolution, multiband dynamics, pitch/time processing, spectral processors, vocoder |
| C — host/routing dependent | Requires mixer graph, sidechain, event or MIDI model before honest migration | send, transmitter, transporter, frequency splitter, EQUO, Patcher, VFX modules |
| D — generator/asset dependent | Requires note scheduler, voice management, sample/import policy, or instrument state | 3x Osc, Sytrus, Sampler, Slicex, SoundFont Player, drum instruments |
| E — UI/utility/visual | Frontend or desktop capability, not an audio playback transformation | meters, notebooks, Edison, ZGame visualizer, dashboards, control surfaces |
| F — DAW subsystem | Separate Aurora feature, not an effect module | playlist, piano roll, automation clips, recording, project model, mixer routing |

## Implemented Aurora batch

- `ToolsParams` / `Tools`: balance, left phase inversion, mid/side stereo width, bounded soft clipper.
- Global DSP fallback and per-track profiles in the Effects page.
- `EffectTools` transformation reporting; exact bypass; finite parameter normalization.
- Orchard source is not copied into Aurora; implementations are independent Rust code based on the public behavior specifications.

## Effect inventory

All effect names below are tracked. Names marked **A** are the first candidates for direct stream-DSP migration; **B–E** require the contracts described above before implementation.

- `balance` — **A**
- `bassboost` — **A**
- `bigclock` — **B/C/E**
- `bloodoverdrive` — **A**
- `center` — **A**
- `chorus` — **A**
- `compressor` — **A**
- `controlsurface` — **B/C/E**
- `convolver` — **A**
- `dbmeter` — **B/C/E**
- `delay` — **A**
- `delay2` — **A**
- `delay3` — **A**
- `delaybank` — **A**
- `distructor` — **A**
- `edison` — **B/C/E**
- `effector` — **A**
- `emphasis` — **A**
- `emphasizer` — **A**
- `eq7` — **A**
- `equo` — **A**
- `fastdist` — **A**
- `fastlp` — **A**
- `filter` — **A**
- `flanger` — **A**
- `flangus` — **A**
- `formulactrl` — **B/C/E**
- `freefilter` — **A**
- `freqshifter` — **A**
- `freqsplitter` — **B/C/E**
- `grossbeat` — **A**
- `hardcore` — **A**
- `htmlnotebook` — **B/C/E**
- `hyperchorus` — **A**
- `limiter` — **A**
- `lovephilter` — **A**
- `lowlifter` — **A**
- `luxeverb` — **A**
- `maximus` — **A**
- `mbcompressor` — **A**
- `mbdelay` — **A**
- `mobilerack` — **B/C/E**
- `mute2` — **A**
- `newtime` — **B/C/E**
- `newtone` — **B/C/E**
- `notebook` — **B/C/E**
- `notebook2` — **B/C/E**
- `panomatic` — **A**
- `patcher` — **B/C/E**
- `peakctrl` — **B/C/E**
- `peq` — **A**
- `phaseinverter` — **A**
- `phaser` — **A**
- `pitcher` — **B/C/E**
- `pitchshifter` — **A**
- `reeverb` — **A**
- `reeverb2` — **A**
- `scratcher` — **B/C/E**
- `send` — **B/C/E**
- `softclipper` — **A**
- `soundgoodizer` — **A**
- `spectroman` — **B/C/E**
- `spreader` — **A**
- `squeeze` — **A**
- `stereoenhancer` — **A**
- `stereoshaper` — **A**
- `transient` — **A**
- `transmitter` — **B/C/E**
- `transporter` — **B/C/E**
- `tuner` — **B/C/E**
- `vchorus` — **A**
- `vfxcolormapper` — **B/C/E**
- `vfxenvelope` — **B/C/E**
- `vfxkeymapper` — **B/C/E**
- `vfxkeysplitter` — **B/C/E**
- `vfxlevelscaler` — **B/C/E**
- `vfxscript` — **B/C/E**
- `vfxsequencer` — **B/C/E**
- `vocoder` — **B/C/E**
- `vocodex` — **B/C/E**
- `vphaser` — **A**
- `wavecandy` — **B/C/E**
- `waveshaper` — **A**
- `xyctrl` — **B/C/E**
- `xyzctrl` — **B/C/E**
- `zgameviz` — **B/C/E**

## Generator and instrument inventory

Generator research modules are not stream effects: they need a note/event scheduler, voice lifecycle, polyphony, sample/resource policy, and often a mixer channel. They are therefore tracked as Class D until Aurora has that architecture.

- `3xosc` — **D**
- `autogun` — **D**
- `bassdrum` — **D**
- `beepmap` — **D**
- `boobass` — **D**
- `dance` — **D**
- `dashboard` — **D**
- `drumaxx` — **D**
- `drumpad` — **D**
- `drumsynth` — **D**
- `dx10` — **D**
- `envctrl` — **D**
- `flkeys` — **D**
- `flslayer` — **D**
- `fpc` — **D**
- `fruitkick` — **D**
- `gms` — **D**
- `granulizer` — **D**
- `harmless` — **D**
- `harmor` — **D**
- `kepler` — **D**
- `keplerexo` — **D**
- `keyboardctrl` — **D**
- `minisynth` — **D**
- `morphine` — **D**
- `ogun` — **D**
- `patcher` — **D**
- `plucked` — **D**
- `poizone` — **D**
- `sakura` — **D**
- `sawer` — **D**
- `simsynth` — **D**
- `slicer` — **D**
- `slicer2` — **D**
- `slicex` — **D**
- `soundfont` — **D**
- `sytrus` — **D**
- `toxicbiohazard` — **D**
- `transistorbass` — **D**
- `videoplayer` — **D**
- `voltagectrl` — **D**
- `wasp` — **D**
- `waspxt` — **D**
- `wavetraveller` — **D**

## DAW and host systems

| Orchard area | Aurora migration boundary |
|---|---|
| `src/engine/Engine.ts`, `Sequencer.ts`, `Automation.ts` | Class F: requires a transport, pattern/song timeline, event scheduler, and automation model. |
| `Mixer.ts`, `GenChannel.ts`, `InstrumentChannel.ts`, `LayerChannel.ts` | Class C/F: requires a graph with sends, sidechain inputs, PDC, and voice channels. |
| `Sampler.ts`, `AudioClips.ts`, `stretch.ts` | Class D/F: requires clip/voice state and explicit asset ownership. |
| `Recording.ts`, `AudioInput.ts`, `Recorder.ts` | Class E/F: desktop/browser device and permission boundary; not `aurora-dsp`. |
| `MidiOut.ts`, `midi.ts`, `midiLinks.ts` | Class C/F: requires MIDI ports, event timing, and link routing. |
| `Patcher.ts` concepts, VFX/controllers | Class C/E: requires a graph/event/control protocol. |
| project/store/UI/editor files | Class F/E: Aurora frontend/domain redesign; no source transplant. |

## Delivery order

1. Complete Class A processors in Rust batches, each with config, UI, transformation, and tests.
2. Add shared state/tail/latency interfaces needed by Class B processors.
3. Introduce an explicit mixer graph and sidechain/event protocol for Class C.
4. Add note scheduling, voices, sampler assets, and project persistence for Class D/F.
5. Port visual/editing/recording utilities behind platform capabilities (Class E).

This ordering prevents a UI that advertises Orchard features whose audio path does not exist and keeps Aurora integrity diagnostics truthful.
