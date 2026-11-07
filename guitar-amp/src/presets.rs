//! Presets d'usine et structure d'un preset.
use crate::params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub values: BTreeMap<String, f32>,
}

/// Presets d'usine, du jazz à Gojira. Les valeurs non citées gardent leur défaut.
pub fn factory_presets() -> Vec<Preset> {
    let base = params::Params::new().snapshot();
    let mk = |name: &str, v: &[(&str, f32)]| {
        let mut values = base.clone();
        for (k, x) in v {
            values.insert(k.to_string(), *x);
        }
        Preset { name: name.to_string(), values }
    };
    // amp_model : 0 Clean, 1 Crunch, 2 Lead, 3 Metal
    vec![
        mk("Jazz (clean chaud)", &[("amp_model", 0.0), ("amp_gain", 3.0), ("bass", 7.0), ("mid", 4.0), ("treble", 1.5), ("presence", 1.5), ("rv_on", 1.0), ("rv_size", 4.0), ("rv_mix", 2.5), ("gate_on", 0.0)]),
        mk("Country (clean brillant)", &[("amp_model", 0.0), ("amp_gain", 4.0), ("bass", 4.0), ("mid", 6.0), ("treble", 7.5), ("presence", 6.0), ("dl_on", 1.0), ("dl_time", 1.0), ("dl_fb", 1.0), ("dl_mix", 2.5), ("gate_on", 0.0)]),
        mk("Dire Straits (clean/crunch)", &[("amp_model", 0.0), ("amp_gain", 6.5), ("bass", 4.5), ("mid", 6.0), ("treble", 7.0), ("presence", 6.5), ("ch_on", 1.0), ("ch_mix", 2.0), ("ch_rate", 2.0), ("rv_on", 1.0), ("rv_mix", 2.0)]),
        mk("Blues (crunch léger)", &[("amp_model", 1.0), ("amp_gain", 4.0), ("bass", 5.0), ("mid", 6.5), ("treble", 5.5), ("presence", 4.5), ("od_on", 1.0), ("od_drive", 3.0), ("od_tone", 5.0), ("od_level", 6.0), ("rv_on", 1.0), ("rv_mix", 2.5)]),
        mk("Rock (crunch)", &[("amp_model", 1.0), ("amp_gain", 7.0), ("bass", 5.5), ("mid", 6.0), ("treble", 6.0), ("presence", 5.5), ("dl_on", 1.0), ("dl_time", 3.5), ("dl_mix", 2.0)]),
        mk("Solo (lead)", &[("amp_model", 2.0), ("amp_gain", 6.5), ("bass", 4.5), ("mid", 7.0), ("treble", 5.5), ("presence", 5.5), ("dl_on", 1.0), ("dl_time", 4.0), ("dl_fb", 3.5), ("dl_mix", 3.0), ("rv_on", 1.0), ("rv_mix", 3.0)]),
        mk("Folk acoustique", &[("inst", 2.0), ("body", 5.5), ("treble", 6.5), ("presence", 6.0), ("gate_on", 0.0), ("rv_on", 1.0), ("rv_size", 3.0), ("rv_mix", 2.0)]),
        mk("Classique (nylon)", &[("inst", 1.0), ("body", 5.5), ("treble", 4.5), ("gate_on", 0.0), ("rv_on", 1.0), ("rv_size", 4.0), ("rv_mix", 2.5)]),
        mk("Country jumbo", &[("inst", 3.0), ("body", 6.0), ("bass", 6.0), ("treble", 6.0), ("gate_on", 0.0), ("dl_on", 1.0), ("dl_time", 1.0), ("dl_fb", 1.0), ("dl_mix", 2.0)]),
        mk("Ukulélé", &[("inst", 4.0), ("body", 5.0), ("gate_on", 0.0), ("rv_on", 1.0), ("rv_size", 2.0), ("rv_mix", 1.5)]),
        mk("Banjo bluegrass", &[("inst", 5.0), ("body", 5.0), ("gate_on", 0.0), ("rv_on", 1.0), ("rv_size", 2.0), ("rv_mix", 1.5)]),
        mk("Gojira (metal)", &[("amp_model", 3.0), ("amp_gain", 6.5), ("bass", 6.0), ("mid", 3.5), ("treble", 6.5), ("presence", 6.0), ("od_on", 1.0), ("od_drive", 1.0), ("od_tone", 6.5), ("od_level", 8.0), ("gate_on", 1.0), ("gate_th", 5.0)]),
    ]
}

