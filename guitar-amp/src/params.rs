//! Paramètres partagés entre l'interface (thread GUI) et le thread audio, sans verrou.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }
    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Relaxed)
    }
}

macro_rules! params {
    ($($name:ident = $def:expr),* $(,)?) => {
        /// Version atomique (partagée entre threads).
        pub struct Params { $(pub $name: AtomicF32,)* }
        /// Copie « photo » des valeurs, lue une fois par bloc audio.
        #[derive(Clone, Debug)]
        pub struct Values { $(pub $name: f32,)* }

        impl Params {
            pub fn new() -> Self { Self { $($name: AtomicF32::new($def),)* } }
            pub fn set_by_name(&self, n: &str, v: f32) {
                match n { $(stringify!($name) => self.$name.set(v),)* _ => {} }
            }
            /// Copie vers la structure Slint `Vals` (affichage des potards).
            pub fn to_ui(&self) -> crate::Vals { crate::Vals { $($name: self.$name.get(),)* } }
            pub fn read(&self) -> Values { Values { $($name: self.$name.get(),)* } }
            /// Valeurs sauvegardables dans un preset (hors métronome et mute).
            pub fn snapshot(&self) -> BTreeMap<String, f32> {
                let mut m = BTreeMap::new();
                $(
                    let n = stringify!($name);
                    if !n.starts_with("metro_") && n != "mute" { m.insert(n.to_string(), self.$name.get()); }
                )*
                m
            }
            pub fn load(&self, m: &BTreeMap<String, f32>) {
                $(
                    let n = stringify!($name);
                    if !n.starts_with("metro_") && n != "mute" {
                        if let Some(v) = m.get(n) { self.$name.set(*v); }
                    }
                )*
            }
        }
        impl Default for Params { fn default() -> Self { Self::new() } }
    };
}

params! {
    input = 5.0,
    gate_on = 1.0, gate_th = 3.0,
    od_on = 0.0, od_drive = 5.0, od_tone = 5.0, od_level = 5.0,
    ch_on = 0.0, ch_rate = 3.0, ch_depth = 5.0, ch_mix = 5.0,
    inst = 0.0, body = 5.0,
    amp_model = 1.0, amp_gain = 5.0, bass = 5.0, mid = 5.0, treble = 5.0, presence = 5.0, master = 5.0, cab_on = 1.0,
    dl_on = 0.0, dl_time = 4.0, dl_fb = 3.0, dl_mix = 3.0,
    rv_on = 0.0, rv_size = 4.0, rv_mix = 3.0,
    mute = 0.0,
    metro_on = 0.0, metro_bpm = 100.0, metro_vol = 5.0,
}
