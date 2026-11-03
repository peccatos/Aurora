//! VST3 no-op stand-in for builds without native plugin hosting.
//!
//! Aurora Web (wasm) has no VST3: native DLLs cannot load in a browser.
//! The engine's VST integration points compile against these types so
//! the rest of the pipeline is unchanged. Every operation is honest
//! identity: an empty chain, no transformations, passthrough audio —
//! the same behavior as a native build with no plugins configured.

use aurora_core::AudioTransformation;

/// Serialized chain configuration (same wire shape as the real one).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct VstPluginList {
    pub slots: Vec<(String, String)>,
}

impl VstPluginList {
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The transformations this chain reports when active (ADR 0002).
    /// A stub chain can never be non-empty, so this is always empty —
    /// honest: no plugin runs, nothing is reported.
    pub fn transformations(&self) -> Vec<AudioTransformation> {
        Vec::new()
    }
}

/// A "loaded" chain: passthrough only. Loading a non-empty list in a
/// no-host build fails honestly — the caller logs and plays without
/// plugins, exactly like a native load failure.
pub struct VstPluginChain;

impl VstPluginChain {
    pub fn load(list: &VstPluginList, _format: &aurora_core::PcmFormat) -> Result<Self, String> {
        if list.slots.is_empty() {
            Ok(Self)
        } else {
            Err("VST3 hosting is not available in this build (no native plugins on wasm)".into())
        }
    }

    pub fn active_plugin_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// A stub chain is always a passthrough.
    pub fn is_passthrough(&self) -> bool {
        true
    }

    /// No plugins: nothing to apply parameters to.
    pub fn apply_params(&mut self, _params: &std::collections::HashMap<u32, f64>) {}

    /// No plugins: nothing to apply parameters to.
    pub fn apply_params_to(&mut self, _slot: usize, _params: &std::collections::HashMap<u32, f64>) {
    }

    /// Identity processing: samples pass through unmodified.
    pub fn process_with_bypass(
        &mut self,
        interleaved: &[f32],
        _bypassed: &std::collections::HashSet<usize>,
    ) -> Vec<f32> {
        interleaved.to_vec()
    }

    pub fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        interleaved.to_vec()
    }

    /// No tail: nothing was ever processed.
    pub fn flush(&mut self) -> Vec<f32> {
        Vec::new()
    }
}
