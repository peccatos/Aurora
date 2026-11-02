//! VST3 plugin hosting (ADR 0004, Phase 9).
//!
//! Third-party plugins are the least trusted code in this process. The
//! contract:
//!
//! - **Out-of-process**: plugins load into `vst3-host`'s isolated helper
//!   process. A crashing plugin kills the helper, not the player.
//! - **Never in the audio callback**: processing runs inline on the
//!   decode worker (already non-RT — it sleeps in back-pressure loops)
//!   between the DSP pipeline and the wire conversion. The callback still
//!   only pops bytes from the SPSC ring.
//! - **Honest**: an active plugin is `AudioTransformation::ExternalPlugin`;
//!   integrity drops to `Transformed` and exclusive mode refuses while a
//!   plugin is active (ADR 0002).

mod error;
mod gui;
mod host;

pub use error::VstError;
pub use gui::{PluginGui, open_gui};
pub use host::{
    PluginDescriptor, PluginParam, VstPluginChain, VstPluginHandle, VstPluginList,
    list_preset_files, load_preset_and_params, probe_params, save_preset_file,
    scan_available_plugins, test_plugin_load,
};
