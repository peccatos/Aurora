# Audio pipeline

## Planned data flow

File source -> Symphonia decoder worker -> typed PCM frames -> bounded SPSC
buffer -> output adapter -> operating-system audio subsystem -> device.

The DSP path is optional:

PCM -> explicit sample conversion -> gain/EQ -> resampler (when required) ->
format conversion -> output adapter.

## Real-time callback constraints

The callback must not allocate, block, lock a contended mutex, access storage,
decode files, log, or wait for worker threads. It should copy available frames,
fill any underrun remainder with silence, and update lock-free counters.

## Diagnostics

Expose at least:

- source codec and container;
- source sample rate, channel count and sample representation;
- selected device and supported output configuration;
- negotiated output sample rate, channel count and sample format;
- active DSP processors;
- resampling and sample-format conversion status;
- underrun count;
- bit-perfect eligibility and verification status.

Do not label a stream bit-perfect unless the complete output chain has been
verified for the selected platform and device mode.
