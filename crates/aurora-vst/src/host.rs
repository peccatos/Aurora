//! VST3 host facade: discovery, loading, and the processing chain.

use std::path::{Path, PathBuf};

use aurora_core::{AudioTransformation, PcmFormat};
use vst3_host::{AudioBuffers, Plugin, Vst3Host, Vst3HostBuilder};

use crate::error::VstError;

/// Fixed block size for plugin rendering (frames). VST3 plugins expect
/// steady, bounded blocks; 512 at 44.1k is ~11.6ms — small enough to keep
/// latency low, large enough to amortize the IPC round-trip to the
/// isolated helper process.
pub const BLOCK_SIZE: usize = 512;

/// Serializable descriptor of a discoverable VST3 effect.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginDescriptor {
    /// Absolute path of the VST3 bundle/file.
    pub path: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// VST3 category string (e.g. "Fx", "Instrument").
    pub category: String,
    /// Hex class id for `load_plugin_class`.
    pub class_id: String,
    /// Number of main audio input channels.
    pub input_channels: i32,
    /// Number of main audio output channels.
    pub output_channels: i32,
}

/// Scan the standard system VST3 directories (plus an optional extra
/// folder) using the crash-resistant out-of-process probe. Plugins that
/// crash or hang the probe are skipped, not fatal.
pub fn scan_available_plugins(extra_folder: Option<&Path>) -> Vec<PluginDescriptor> {
    let mut builder = Vst3HostBuilder::default().scan_default_paths();
    // Per-user plugin folder (VST3 spec location on Windows; the library's
    // default scan only covers Program Files).
    #[cfg(target_os = "windows")]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let user_vst = PathBuf::from(local).join("VST3");
        if user_vst.is_dir() {
            builder = builder.add_scan_path(user_vst);
        }
    }
    #[cfg(not(target_os = "windows"))]
    if let Some(home) = std::env::var_os("HOME") {
        let user_vst = PathBuf::from(home).join(".vst3");
        if user_vst.is_dir() {
            builder = builder.add_scan_path(user_vst);
        }
    }
    if let Some(dir) = extra_folder {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            builder = builder.add_scan_path(dir);
        }
    }
    let Ok(host) = builder.build() else {
        tracing::warn!("vst3 host construction failed");
        return Vec::new();
    };

    let report = host.discover_plugins_safe();
    if !report.scan_ran() {
        tracing::warn!(error = ?report.error, "vst3 discovery could not run");
        return Vec::new();
    }
    for skip in &report.skipped {
        tracing::warn!(path = %skip.path().display(), "vst3 plugin skipped (crash/timeout/failure)");
    }

    report
        .plugins
        .iter()
        .filter_map(|d| {
            // Only audio effects (no instruments without MIDI input
            // infrastructure yet, no non-audio classes).
            let audio_class = d
                .classes
                .iter()
                .find(|c| c.category.contains("Audio Module Class"))
                .or_else(|| d.classes.first())?;
            let is_fx =
                d.info.category.to_ascii_lowercase().contains("fx") || d.info.audio_inputs > 0;
            if !is_fx {
                return None;
            }
            let (in_ch, out_ch) = main_bus_channels(&d.buses);
            Some(PluginDescriptor {
                path: d.info.path.display().to_string(),
                name: d.info.name.clone(),
                vendor: d.info.vendor.clone(),
                version: d.info.version.clone(),
                category: d.info.category.clone(),
                class_id: audio_class.class_id.clone(),
                input_channels: in_ch,
                output_channels: out_ch,
            })
        })
        .collect()
}

/// Main-bus channel counts from the bus layout (first main input/output).
fn main_bus_channels(buses: &vst3_host::BusLayout) -> (i32, i32) {
    use vst3_host::BusInfo;
    fn main_ch(buses: &[BusInfo]) -> i32 {
        buses.first().map(|b| b.channel_count).unwrap_or_default()
    }
    (main_ch(&buses.audio_inputs), main_ch(&buses.audio_outputs))
}

/// A loaded, isolated VST3 plugin instance.
pub struct VstPluginHandle {
    plugin: Plugin,
    pub descriptor: PluginDescriptor,
    /// Plugin-reported latency in samples, if any (honest transformation
    /// data, surfaced in diagnostics).
    pub latency_samples: Option<u32>,
}

impl VstPluginHandle {
    /// All parameters (id, title, normalized value) for UI control.
    pub fn parameters(&self) -> Result<Vec<PluginParam>, VstError> {
        Ok(self
            .plugin
            .get_parameters()
            .map_err(|e| VstError::Load(e.to_string()))?
            .into_iter()
            .map(|p| PluginParam {
                id: p.id,
                title: p.name,
                value: p.value,
            })
            .collect())
    }

    /// Set one parameter by id (normalized 0..1 value).
    pub fn set_parameter(&mut self, id: u32, value: f64) -> Result<(), VstError> {
        self.plugin
            .set_parameter(id, value)
            .map_err(|e| VstError::Processing(e.to_string()))
    }
}

/// One automatable plugin parameter.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginParam {
    pub id: u32,
    pub title: String,
    pub value: f64,
}

/// The chain of active plugins on a stream. Serialized configuration
/// (paths + class ids) so a chain survives restarts.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct VstPluginList {
    /// (bundle path, class id) per slot, in processing order.
    pub slots: Vec<(String, String)>,
}

impl VstPluginList {
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The transformations this chain reports when active (ADR 0002).
    pub fn transformations(&self) -> Vec<AudioTransformation> {
        if self.slots.is_empty() {
            Vec::new()
        } else {
            vec![AudioTransformation::ExternalPlugin]
        }
    }
}

/// Load a single plugin out-of-process and verify it starts processing.
/// Returns the plugin's display name on success or the honest reason it
/// refused. This is the "does it actually work on this machine" check the
/// UI runs when the user attaches a plugin — before playback depends on
/// it and failures go silent.
pub fn test_plugin_load(path: &str, class_id: &str) -> Result<String, String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("plugin file not found: {path}"));
    }
    let mut host = match Vst3HostBuilder::default()
        .sample_rate(44_100.0)
        .block_size(BLOCK_SIZE)
        .build()
    {
        Ok(h) => h,
        Err(e) => return Err(format!("host build failed: {e}")),
    };
    let mut plugin = match host.load_plugin_class(p, class_id) {
        Ok(p) => p,
        Err(e) => return Err(format!("load failed: {e}")),
    };
    if let Err(e) = plugin.start_processing() {
        return Err(format!("start_processing failed: {e}"));
    }
    let name = plugin.info().name.clone();
    let latency = plugin.latency_samples();
    // One silent block through the plugin: exercises the real IPC path
    // (helper process round-trip) so "loads" is not confused with "works".
    let mut buffers = AudioBuffers::new(2, 2, BLOCK_SIZE, 44_100.0);
    match plugin.process_audio(&mut buffers) {
        Ok(()) => Ok(format!("{name} — OK (latency {latency} samples)")),
        Err(e) => Err(format!("{name} loaded, but processing failed: {e}")),
    }
}

/// A compiled, live VST3 processing chain for one playback stream.
///
/// Built for a specific `PcmFormat`; processes interleaved f32 samples
/// in-place. Runs on the decode worker (never the audio callback).
pub struct VstPluginChain {
    plugins: Vec<VstPluginHandle>,
    /// Per-plugin deinterleaved scratch buffers.
    buffers: Vec<AudioBuffers>,
    /// Carry-over samples: the chain consumes fixed BLOCK_SIZE frames,
    /// decoder chunks are arbitrary.
    fifo: Vec<f32>,
    /// Input channel count we map (stream channels -> plugin buses).
    channels: usize,
    /// Accumulated output produced but not yet consumed by the caller.
    out: Vec<f32>,
}

impl VstPluginChain {
    /// Load and activate every plugin in the list. Plugins whose channel
    /// layout cannot map to the stream are skipped with a warning —
    /// honest degradation, not a failed track.
    pub fn load(list: &VstPluginList, format: &PcmFormat) -> Result<Self, VstError> {
        let sample_rate = f64::from(format.sample_rate);
        let channels = usize::from(format.channels.max(1));

        let mut host_builder = Vst3HostBuilder::default();
        host_builder = host_builder
            .sample_rate(sample_rate)
            .block_size(BLOCK_SIZE)
            .input_channels(channels)
            .output_channels(channels);
        let mut host: Vst3Host = host_builder
            .build()
            .map_err(|e| VstError::Host(e.to_string()))?;

        let mut plugins = Vec::new();
        for (path, class_id) in &list.slots {
            if !Path::new(path).exists() {
                tracing::warn!(%path, "vst3 plugin file missing, skipping slot");
                continue;
            }
            let mut plugin = match host.load_plugin_class(path, class_id) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(%path, error = %e, "vst3 plugin refused to load, skipping");
                    continue;
                }
            };
            if let Err(e) = plugin.start_processing() {
                tracing::warn!(%path, error = %e, "vst3 plugin start_processing failed, skipping");
                continue;
            }
            let info = plugin.info().clone();
            let latency = plugin.latency_samples();
            plugins.push(VstPluginHandle {
                descriptor: PluginDescriptor {
                    path: info.path.display().to_string(),
                    name: info.name,
                    vendor: info.vendor,
                    version: info.version,
                    category: info.category,
                    class_id: class_id.clone(),
                    input_channels: i32::try_from(channels).unwrap_or(2),
                    output_channels: i32::try_from(channels).unwrap_or(2),
                },
                plugin,
                latency_samples: Some(latency),
            });
        }

        let n = plugins.len();
        let buffers = (0..n)
            .map(|_| AudioBuffers::new(channels, channels, BLOCK_SIZE, sample_rate))
            .collect();
        tracing::info!(count = n, "vst3 chain active (out-of-process)");
        Ok(Self {
            plugins,
            buffers,
            fifo: Vec::new(),
            channels,
            out: Vec::new(),
        })
    }

    /// True when no plugin actually loaded (all slots skipped/empty).
    pub fn is_passthrough(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Apply editor-GUI parameter values to the first plugin in the
    /// chain (id -> normalized value). Values going to the out-of-process
    /// plugin ride the next block's IPC round-trip.
    pub fn apply_params(&mut self, params: &std::collections::HashMap<u32, f64>) {
        let Some(handle) = self.plugins.first_mut() else {
            return;
        };
        for (id, value) in params {
            if let Err(e) = handle.plugin.set_parameter(*id, *value) {
                tracing::warn!(%e, id, "vst param apply failed");
            }
        }
    }

    /// The honest transformation list (ADR 0002).
    pub fn transformations(&self) -> Vec<AudioTransformation> {
        if self.plugins.is_empty() {
            Vec::new()
        } else {
            vec![AudioTransformation::ExternalPlugin]
        }
    }

    /// Names of active plugins, for diagnostics.
    pub fn active_plugin_names(&self) -> Vec<String> {
        self.plugins
            .iter()
            .map(|p| p.descriptor.name.clone())
            .collect()
    }

    /// Total plugin-reported latency in samples across the chain.
    pub fn latency_samples(&self) -> u32 {
        self.plugins.iter().filter_map(|p| p.latency_samples).sum()
    }

    /// Process interleaved f32 samples. Returns the processed interleaved
    /// samples; length may differ from the input (block quantization +
    /// plugin latency). Call `flush` at end of stream for the tail.
    pub fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        if self.plugins.is_empty() {
            return interleaved.to_vec();
        }
        let nch = self.channels;
        self.fifo.extend_from_slice(interleaved);
        let block_samples = BLOCK_SIZE * nch;
        while self.fifo.len() >= block_samples {
            let block: Vec<f32> = self.fifo.drain(..block_samples).collect();
            self.process_block(&block);
        }
        std::mem::take(&mut self.out)
    }

    /// Drain the FIFO's partial block with zero-padding and emit the tail.
    pub fn flush(&mut self) -> Vec<f32> {
        if self.plugins.is_empty() {
            return Vec::new();
        }
        let nch = self.channels;
        let block_samples = BLOCK_SIZE * nch;
        if !self.fifo.is_empty() {
            let mut block = std::mem::take(&mut self.fifo);
            block.resize(block_samples, 0.0);
            self.process_block(&block);
        }
        std::mem::take(&mut self.out)
    }

    /// Run one fixed block through every plugin in series.
    fn process_block(&mut self, block: &[f32]) {
        let nch = self.channels;
        if self.plugins.is_empty() || self.buffers.is_empty() {
            return;
        }
        // Deinterleave into the first plugin's input channels.
        {
            let buffers = &mut self.buffers[0];
            let in_ch = buffers.input_channels().min(nch);
            for ch in 0..in_ch {
                buffers.inputs[ch]
                    .iter_mut()
                    .zip(block[ch..].iter().step_by(nch))
                    .for_each(|(dst, src)| *dst = *src);
            }
            // Extra stream channels beyond the plugin's inputs: duplicate
            // the corresponding low channel (honest channel remap).
            let inputs_len = buffers.inputs.len();
            for ch in in_ch..inputs_len {
                let src: Vec<f32> = block[0..].iter().step_by(nch).copied().collect();
                let len = BLOCK_SIZE.min(src.len());
                buffers.inputs[ch][..len].copy_from_slice(&src[..len]);
            }
        }

        // Chain: plugin[i] outputs -> plugin[i+1] inputs.
        for i in 0..self.plugins.len() {
            let next_needs_input = i + 1 < self.plugins.len();
            let res = {
                let plugin = &mut self.plugins[i];
                let buffers = &mut self.buffers[i];
                plugin
                    .plugin
                    .process_audio(buffers)
                    .map_err(|e| e.to_string())
            };
            match res {
                Ok(()) => {}
                Err(e) => {
                    // Processing failure: pass the block through unprocessed
                    // (honest — the transformation list still reports the
                    // plugin; the UI notes the failure via tracing).
                    tracing::error!(%e, plugin = %self.plugins[i].descriptor.name, "vst3 process failed, passing block through");
                    let buffers = &mut self.buffers[i];
                    for ch in 0..buffers.output_channels().min(buffers.input_channels()) {
                        buffers.outputs[ch].copy_from_slice(&buffers.inputs[ch]);
                    }
                }
            }
            if next_needs_input {
                let (head, tail) = self.buffers.split_at_mut(i + 1);
                let src = &head[i];
                let dst = &mut tail[0];
                for (src_ch, dst_ch) in src.outputs.iter().zip(dst.inputs.iter_mut()) {
                    let len = src_ch.len().min(dst_ch.len());
                    dst_ch[..len].copy_from_slice(&src_ch[..len]);
                }
            }
        }

        // Re-interleave the final plugin's outputs.
        let last = self.buffers.len() - 1;
        let out_ch = self.buffers[last].output_channels().min(nch);
        for frame in 0..BLOCK_SIZE {
            for ch in 0..out_ch {
                self.out.push(self.buffers[last].outputs[ch][frame]);
            }
            // Stream channels beyond plugin outputs: duplicate the last
            // plugin channel (honest upmix, same as channel-remap).
            let src = self.buffers[last].outputs[out_ch.saturating_sub(1)][frame];
            for _ in out_ch..nch {
                self.out.push(src);
            }
        }
    }
}
