//! Track-scoped DSP selection.
//!
//! This is deliberately an in-memory resolver. Persistence belongs to the
//! library/settings layer and is not implied by this type.

use aurora_dsp::DspConfig;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct DspProfileResolver {
    profiles: HashMap<String, DspConfig>,
}

impl DspProfileResolver {
    pub fn set(&mut self, track_key: impl Into<String>, config: DspConfig) {
        self.profiles.insert(track_key.into(), config);
    }
    pub fn remove(&mut self, track_key: &str) -> Option<DspConfig> {
        self.profiles.remove(track_key)
    }
    pub fn resolve<'a>(&'a self, track_key: &str, fallback: &'a DspConfig) -> &'a DspConfig {
        self.profiles.get(track_key).unwrap_or(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_track_profile_and_falls_back() {
        let mut r = DspProfileResolver::default();
        let fallback = DspConfig::default();
        let profile = DspConfig {
            volume_db: -6.0,
            ..fallback.clone()
        };
        assert_eq!(r.resolve("a", &fallback), &fallback);
        r.set("a", profile.clone());
        assert_eq!(r.resolve("a", &fallback), &profile);
        assert_eq!(r.resolve("b", &fallback), &fallback);
        assert_eq!(r.remove("a"), Some(profile));
    }
}
