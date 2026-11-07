//! Traitement du signal : gate -> overdrive -> chorus -> ampli (préampli, EQ, baffle) -> delay -> reverb.
use crate::params::Values;
use std::f32::consts::PI;

// ---------------------------------------------------------------- utilitaires

#[derive(Clone, Copy, Debug)]
pub enum FilterKind {
    LowPass,
    HighPass,
    LowShelf,
    HighShelf,
    Peaking,
}

/// Filtre biquad (formules RBJ), forme transposée directe II.
#[derive(Clone)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn new(kind: FilterKind, sr: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let mut b = Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 };
        b.set(kind, sr, freq, q, gain_db);
        b
    }

    pub fn set(&mut self, kind: FilterKind, sr: f32, freq: f32, q: f32, gain_db: f32) {
        let f = freq.clamp(10.0, sr * 0.45);
        let w0 = 2.0 * PI * f / sr;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        let a = 10f32.powf(gain_db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterKind::LowPass => ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
            FilterKind::HighPass => ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
            FilterKind::Peaking => (
                1.0 + alpha * a,
                -2.0 * c,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * c,
                1.0 - alpha / a,
            ),
            FilterKind::LowShelf => {
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * c + sq),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                    a * ((a + 1.0) - (a - 1.0) * c - sq),
                    (a + 1.0) + (a - 1.0) * c + sq,
                    -2.0 * ((a - 1.0) + (a + 1.0) * c),
                    (a + 1.0) + (a - 1.0) * c - sq,
                )
            }
            FilterKind::HighShelf => {
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * c + sq),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                    a * ((a + 1.0) + (a - 1.0) * c - sq),
                    (a + 1.0) - (a - 1.0) * c + sq,
                    2.0 * ((a - 1.0) - (a + 1.0) * c),
                    (a + 1.0) - (a - 1.0) * c - sq,
                )
            }
        };
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Lissage exponentiel d'un paramètre (évite les craquements quand on tourne un potard).
struct Smooth {
    v: f32,
    c: f32,
}
impl Smooth {
    fn new(init: f32, ms: f32, sr: f32) -> Self {
        Self { v: init, c: 1.0 - (-1.0 / (ms * 0.001 * sr)).exp() }
    }
    #[inline]
    fn next(&mut self, target: f32) -> f32 {
        self.v += (target - self.v) * self.c;
        self.v
    }
}

/// Potard 0..10 -> gain en dB (-12 .. +12).
fn knob_db(v: f32) -> f32 {
    (v - 5.0) * 2.4
}

/// Réglage d'entrée : 0 = -12 dB, 5 = neutre (0 dB), 10 = +36 dB.
/// Un câble guitare USB donne un signal faible : il faut beaucoup de marge vers le haut.
pub fn input_db(v: f32) -> f32 {
    if v < 5.0 {
        (v - 5.0) * 2.4
    } else {
        (v - 5.0) * 7.2
    }
}

/// Inverse de `input_db` : position du potard pour un gain voulu en dB.
pub fn input_knob_for_db(db: f32) -> f32 {
    let v = if db < 0.0 { 5.0 + db / 2.4 } else { 5.0 + db / 7.2 };
    v.clamp(0.0, 10.0)
}

/// Limiteur doux final : transparent sous 0,8, puis le son est écrasé progressivement sans jamais dépasser 1,0
/// (protège les oreilles et la carte son quand tous les réglages sont poussés à fond).
#[inline]
pub fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a < 0.8 {
        x
    } else {
        x.signum() * (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh())
    }
}

/// Saturation asymétrique (type lampe), sortie normalisée à ~1.
#[inline]
pub fn shape(x: f32, k: f32, bias: f32) -> f32 {
    ((k * (x + bias)).tanh() - (k * bias).tanh()) / k.tanh()
}

/// Sur-échantillonnage x2 pour limiter le repliement de spectre dans la distorsion.
struct Oversampler2x {
    prev: f32,
    lp1: Biquad,
    lp2: Biquad,
}
impl Oversampler2x {
    fn new(sr: f32) -> Self {
        let fs2 = sr * 2.0;
        let fc = sr * 0.42;
        Self {
            prev: 0.0,
            lp1: Biquad::new(FilterKind::LowPass, fs2, fc, 0.54, 0.0),
            lp2: Biquad::new(FilterKind::LowPass, fs2, fc, 1.3, 0.0),
        }
    }
    #[inline]
    fn process(&mut self, x: f32, f: impl Fn(f32) -> f32) -> f32 {
        let mid = 0.5 * (self.prev + x);
        self.prev = x;
        let a = self.lp2.process(self.lp1.process(f(mid)));
        let b = self.lp2.process(self.lp1.process(f(x)));
        let _ = a;
        b
    }
}

struct DelayLine {
    buf: Vec<f32>,
    w: usize,
}
impl DelayLine {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(4)], w: 0 }
    }
    #[inline]
    fn push(&mut self, x: f32) {
        self.buf[self.w] = x;
        self.w = (self.w + 1) % self.buf.len();
    }
    /// Lecture avec interpolation linéaire, `d` en échantillons (>= 1).
    #[inline]
    fn read(&self, d: f32) -> f32 {
        let len = self.buf.len();
        let d = d.clamp(1.0, (len - 2) as f32);
        let mut pos = self.w as f32 - d;
        if pos < 0.0 {
            pos += len as f32;
        }
        let i = pos.floor() as usize;
        let f = pos - i as f32;
        let a = self.buf[i % len];
        let b = self.buf[(i + 1) % len];
        a + (b - a) * f
    }
}

// ---------------------------------------------------------------- effets

struct Gate {
    env: f32,
    gain: f32,
    decay: f32,
    att: f32,
    rel: f32,
}
impl Gate {
    fn new(sr: f32) -> Self {
        Self {
            env: 0.0,
            gain: 1.0,
            decay: (-1.0 / (0.05 * sr)).exp(),
            att: 1.0 - (-1.0 / (0.005 * sr)).exp(),
            rel: 1.0 - (-1.0 / (0.08 * sr)).exp(),
        }
    }
    #[inline]
    fn process(&mut self, x: f32, threshold: f32) -> f32 {
        self.env = x.abs().max(self.env * self.decay);
        let target = if self.env > threshold { 1.0 } else { 0.0 };
        let c = if target > self.gain { self.att } else { self.rel };
        self.gain += (target - self.gain) * c;
        x * self.gain
    }
}

struct Overdrive {
    hp: Biquad,
    lp: Biquad,
    os: Oversampler2x,
    drive: Smooth,
    level: Smooth,
    sr: f32,
}
impl Overdrive {
    fn new(sr: f32) -> Self {
        Self {
            hp: Biquad::new(FilterKind::HighPass, sr, 250.0, 0.7, 0.0),
            lp: Biquad::new(FilterKind::LowPass, sr, 3000.0, 0.7, 0.0),
            os: Oversampler2x::new(sr),
            drive: Smooth::new(1.0, 20.0, sr),
            level: Smooth::new(0.5, 20.0, sr),
            sr,
        }
    }
    fn update(&mut self, p: &Values) {
        self.lp.set(FilterKind::LowPass, self.sr, 800.0 + p.od_tone * 700.0, 0.7, 0.0);
    }
    #[inline]
    fn process(&mut self, x: f32, p: &Values) -> f32 {
        let g = self.drive.next(1.0 + (p.od_drive / 10.0).powi(2) * 60.0);
        let lvl = self.level.next(0.2 + p.od_level / 10.0 * 0.9);
        let y = self.hp.process(x) * g;
        let y = self.os.process(y, |v| (v * 1.2).tanh());
        self.lp.process(y) * lvl
    }
}

struct Chorus {
    dl: DelayLine,
    phase: f32,
    sr: f32,
}
impl Chorus {
    fn new(sr: f32) -> Self {
        Self { dl: DelayLine::new((0.06 * sr) as usize), phase: 0.0, sr }
    }
    #[inline]
    fn process(&mut self, x: f32, p: &Values) -> f32 {
        let rate = 0.2 + p.ch_rate * 0.8;
        self.phase += rate / self.sr;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let lfo = (2.0 * PI * self.phase).sin();
        let d = (0.012 + lfo * (p.ch_depth / 10.0) * 0.007) * self.sr;
        self.dl.push(x);
        let wet = self.dl.read(d);
        x + wet * (p.ch_mix / 10.0) * 0.8
    }
}

struct AmpModel {
    k1: f32,
    k2: f32,
    gain_max: f32,
    two_stages: bool,
    hp: f32,
    mid_shift: f32,
    trim: f32,
}
const MODELS: [AmpModel; 4] = [
    AmpModel { k1: 1.2, k2: 1.0, gain_max: 4.0, two_stages: false, hp: 60.0, mid_shift: 0.0, trim: 1.0 },
    AmpModel { k1: 3.5, k2: 1.0, gain_max: 10.0, two_stages: false, hp: 80.0, mid_shift: 1.0, trim: 0.6 },
    AmpModel { k1: 6.0, k2: 4.8, gain_max: 25.0, two_stages: true, hp: 100.0, mid_shift: 2.0, trim: 0.45 },
    AmpModel { k1: 9.0, k2: 7.0, gain_max: 50.0, two_stages: true, hp: 140.0, mid_shift: -4.0, trim: 0.4 },
];

struct Amp {
    sr: f32,
    pre_hp: Biquad,
    inter_hp: Biquad,
    dc: Biquad,
    os1: Oversampler2x,
    os2: Oversampler2x,
    bass: Biquad,
    mid: Biquad,
    treble: Biquad,
    presence: Biquad,
    cab_hp: Biquad,
    cab_lp1: Biquad,
    cab_lp2: Biquad,
    cab_dip: Biquad,
    gain: Smooth,
}
impl Amp {
    fn new(sr: f32) -> Self {
        Self {
            sr,
            pre_hp: Biquad::new(FilterKind::HighPass, sr, 80.0, 0.7, 0.0),
            inter_hp: Biquad::new(FilterKind::HighPass, sr, 200.0, 0.5, 0.0),
            dc: Biquad::new(FilterKind::HighPass, sr, 20.0, 0.7, 0.0),
            os1: Oversampler2x::new(sr),
            os2: Oversampler2x::new(sr),
            bass: Biquad::new(FilterKind::LowShelf, sr, 120.0, 0.7, 0.0),
            mid: Biquad::new(FilterKind::Peaking, sr, 700.0, 0.9, 0.0),
            treble: Biquad::new(FilterKind::HighShelf, sr, 3200.0, 0.7, 0.0),
            presence: Biquad::new(FilterKind::HighShelf, sr, 5500.0, 0.7, 0.0),
            cab_hp: Biquad::new(FilterKind::HighPass, sr, 85.0, 0.7, 0.0),
            cab_lp1: Biquad::new(FilterKind::LowPass, sr, 5200.0, 0.7, 0.0),
            cab_lp2: Biquad::new(FilterKind::LowPass, sr, 5200.0, 0.9, 0.0),
            cab_dip: Biquad::new(FilterKind::Peaking, sr, 2800.0, 1.2, -3.0),
            gain: Smooth::new(1.0, 15.0, sr),
        }
    }
    fn model(p: &Values) -> &'static AmpModel {
        &MODELS[(p.amp_model.round() as usize).min(MODELS.len() - 1)]
    }
    fn update(&mut self, p: &Values) {
        let m = Self::model(p);
        let sr = self.sr;
        self.pre_hp.set(FilterKind::HighPass, sr, m.hp, 0.7, 0.0);
        self.bass.set(FilterKind::LowShelf, sr, 120.0, 0.7, knob_db(p.bass));
        let shift = if p.inst > 0.5 { 0.0 } else { m.mid_shift };
        self.mid.set(FilterKind::Peaking, sr, 700.0, 0.9, knob_db(p.mid) + shift);
        self.treble.set(FilterKind::HighShelf, sr, 3200.0, 0.7, knob_db(p.treble));
        self.presence.set(FilterKind::HighShelf, sr, 5500.0, 0.7, knob_db(p.presence) * 0.7);
    }
    /// Égaliseur seul (utilisé aussi par les sons acoustiques).
    #[inline]
    fn eq(&mut self, y: f32) -> f32 {
        self.presence.process(self.treble.process(self.mid.process(self.bass.process(y))))
    }
    #[inline]
    fn process(&mut self, x: f32, p: &Values) -> f32 {
        let m = Self::model(p);
        let g = self.gain.next(0.5 + p.amp_gain / 10.0 * m.gain_max);
        let mut y = self.pre_hp.process(x) * g;
        y = self.os1.process(y, |v| shape(v, m.k1, 0.03));
        if m.two_stages {
            y = self.inter_hp.process(y);
            y = self.os2.process(y * 2.0, |v| shape(v, m.k2, 0.02));
        }
        // La saturation asymétrique crée un décalage continu : on le retire avant l'égaliseur.
        y = self.dc.process(y);
        y = self.eq(y);
        if p.cab_on > 0.5 {
            y = self.cab_dip.process(self.cab_lp2.process(self.cab_lp1.process(self.cab_hp.process(y))));
        }
        y * m.trim
    }
}


// ---------------------------------------------------------------- sons acoustiques

/// Réglages d'un instrument : filtres passe-haut / passe-bas et 4 résonances du « corps ».
struct Tone {
    hp: f32,
    lp: f32,
    bands: [(FilterKind, f32, f32, f32); 4],
}
use FilterKind::{HighShelf, Peaking};
const TONES: [Tone; 5] = [
    // Guitare classique (nylon) : chaude, aigus doux
    Tone { hp: 80.0, lp: 6500.0, bands: [(Peaking, 100.0, 2.0, 5.0), (Peaking, 220.0, 1.2, 3.0), (Peaking, 500.0, 1.0, -2.0), (HighShelf, 4000.0, 0.7, -3.0)] },
    // Folk (cordes acier) : équilibrée, claire
    Tone { hp: 70.0, lp: 11000.0, bands: [(Peaking, 100.0, 2.0, 4.0), (Peaking, 250.0, 1.2, 2.0), (Peaking, 400.0, 1.0, -3.0), (HighShelf, 3500.0, 0.7, 4.0)] },
    // Grosse guitare country (jumbo) : graves puissants, médiums creusés
    Tone { hp: 60.0, lp: 11000.0, bands: [(Peaking, 90.0, 1.8, 6.0), (Peaking, 180.0, 1.2, 3.0), (Peaking, 350.0, 1.0, -4.0), (HighShelf, 3000.0, 0.7, 4.0)] },
    // Ukulélé : pas de graves, résonance vers 500 Hz
    Tone { hp: 220.0, lp: 7500.0, bands: [(Peaking, 500.0, 1.5, 5.0), (Peaking, 1200.0, 1.2, 2.0), (Peaking, 300.0, 1.0, -2.0), (HighShelf, 5000.0, 0.7, -2.0)] },
    // Banjo : nasal, métallique, attaque sèche
    Tone { hp: 150.0, lp: 9000.0, bands: [(Peaking, 1200.0, 1.2, 6.0), (Peaking, 2500.0, 1.2, 5.0), (Peaking, 300.0, 1.0, -4.0), (HighShelf, 5000.0, 0.7, 4.0)] },
];

/// Simule le corps d'une guitare acoustique à partir du signal de la guitare électrique.
struct Acoustic {
    sr: f32,
    hp: Biquad,
    lp: Biquad,
    bands: [Biquad; 4],
    key: (i32, i32),
    env: f32,
    att: f32,
    rel: f32,
}
impl Acoustic {
    fn new(sr: f32) -> Self {
        let b = || Biquad::new(Peaking, sr, 1000.0, 1.0, 0.0);
        Self {
            sr,
            hp: Biquad::new(FilterKind::HighPass, sr, 80.0, 0.7, 0.0),
            lp: Biquad::new(FilterKind::LowPass, sr, 8000.0, 0.7, 0.0),
            bands: [b(), b(), b(), b()],
            key: (-1, -1),
            env: 0.0,
            att: 1.0 - (-1.0 / (0.004 * sr)).exp(),
            rel: 1.0 - (-1.0 / (0.12 * sr)).exp(),
        }
    }
    fn update(&mut self, p: &Values) {
        if p.inst < 0.5 {
            return;
        }
        let key = (p.inst.round() as i32, (p.body * 10.0) as i32);
        if key == self.key {
            return;
        }
        self.key = key;
        let t = &TONES[(key.0 as usize - 1).min(TONES.len() - 1)];
        self.hp.set(FilterKind::HighPass, self.sr, t.hp, 0.7, 0.0);
        self.lp.set(FilterKind::LowPass, self.sr, t.lp, 0.7, 0.0);
        // « Corps » : 5 = réglage nominal, 0 = neutre, 10 = résonances doublées.
        let k = p.body / 5.0;
        for (b, &(kind, f, q, g)) in self.bands.iter_mut().zip(t.bands.iter()) {
            b.set(kind, self.sr, f, q, g * k);
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let mut y = self.lp.process(self.hp.process(x));
        for b in self.bands.iter_mut() {
            y = b.process(y);
        }
        // Compresseur doux (rapport 3:1) + gain de compensation : son plus régulier, comme un micro de scène.
        let a = y.abs();
        let c = if a > self.env { self.att } else { self.rel };
        self.env += (a - self.env) * c;
        let th = 0.12;
        let gain = if self.env > th { (th / self.env).powf(0.66) } else { 1.0 };
        y * gain * 3.0
    }
}

struct Echo {
    dl: DelayLine,
    lp: f32,
    time: Smooth,
    sr: f32,
}
impl Echo {
    fn new(sr: f32) -> Self {
        Self { dl: DelayLine::new((2.0 * sr) as usize + 4), lp: 0.0, time: Smooth::new(0.3 * sr, 80.0, sr), sr }
    }
    #[inline]
    fn process(&mut self, x: f32, p: &Values) -> f32 {
        let t = (0.05 + p.dl_time / 10.0 * 0.95) * self.sr;
        let d = self.time.next(t);
        let delayed = self.dl.read(d);
        self.lp += (delayed - self.lp) * 0.35;
        self.dl.push(x + self.lp * (p.dl_fb / 10.0 * 0.85).min(0.9));
        x + self.lp * (p.dl_mix / 10.0)
    }
}

struct Comb {
    buf: Vec<f32>,
    i: usize,
    store: f32,
}
struct Allpass {
    buf: Vec<f32>,
    i: usize,
}
/// Reverb de type Freeverb (8 filtres en peigne + 4 passe-tout).
struct Reverb {
    combs: Vec<Comb>,
    allpass: Vec<Allpass>,
}
impl Reverb {
    fn new(sr: f32) -> Self {
        let scale = sr / 44100.0;
        let mk = |n: usize| ((n as f32 * scale) as usize).max(8);
        Self {
            combs: [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617]
                .iter()
                .map(|&n| Comb { buf: vec![0.0; mk(n)], i: 0, store: 0.0 })
                .collect(),
            allpass: [556, 441, 341, 225].iter().map(|&n| Allpass { buf: vec![0.0; mk(n)], i: 0 }).collect(),
        }
    }
    #[inline]
    fn process(&mut self, x: f32, p: &Values) -> f32 {
        let feedback = 0.7 + p.rv_size / 10.0 * 0.28;
        let damp = 0.25;
        let input = x * 0.015;
        let mut sum = 0.0;
        for c in self.combs.iter_mut() {
            let out = c.buf[c.i];
            c.store = out * (1.0 - damp) + c.store * damp;
            c.buf[c.i] = input + c.store * feedback;
            c.i = (c.i + 1) % c.buf.len();
            sum += out;
        }
        for a in self.allpass.iter_mut() {
            let b = a.buf[a.i];
            let out = -sum + b;
            a.buf[a.i] = sum + b * 0.5;
            a.i = (a.i + 1) % a.buf.len();
            sum = out;
        }
        x + sum * 3.0 * (p.rv_mix / 10.0)
    }
}

struct Metronome {
    counter: f32,
    beat: u32,
    env: f32,
    phase: f32,
    freq: f32,
    sr: f32,
}
impl Metronome {
    fn new(sr: f32) -> Self {
        Self { counter: 1e9, beat: 3, env: 0.0, phase: 0.0, freq: 1000.0, sr }
    }
    /// Retourne (échantillon, numéro de temps si un clic vient de démarrer).
    #[inline]
    fn process(&mut self, p: &Values) -> (f32, Option<u32>) {
        let mut started = None;
        if p.metro_on > 0.5 {
            let spb = self.sr * 60.0 / p.metro_bpm.clamp(30.0, 300.0);
            self.counter += 1.0;
            if self.counter >= spb {
                self.counter = 0.0;
                self.beat = (self.beat + 1) % 4;
                self.env = 1.0;
                self.phase = 0.0;
                self.freq = if self.beat == 0 { 1600.0 } else { 1000.0 };
                started = Some(self.beat);
            }
        } else {
            self.counter = 1e9;
            self.beat = 3;
        }
        let out = (2.0 * PI * self.phase).sin() * self.env * 0.5 * (p.metro_vol / 10.0);
        self.phase += self.freq / self.sr;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        self.env *= 0.9988;
        (out, started)
    }
}

// ---------------------------------------------------------------- chaîne complète

pub struct Chain {
    gate: Gate,
    od: Overdrive,
    chorus: Chorus,
    amp: Amp,
    acoustic: Acoustic,
    echo: Echo,
    reverb: Reverb,
    metro: Metronome,
    input_s: Smooth,
    master_s: Smooth,
}

impl Chain {
    pub fn new(sr: f32) -> Self {
        Self {
            gate: Gate::new(sr),
            od: Overdrive::new(sr),
            chorus: Chorus::new(sr),
            amp: Amp::new(sr),
            acoustic: Acoustic::new(sr),
            echo: Echo::new(sr),
            reverb: Reverb::new(sr),
            metro: Metronome::new(sr),
            input_s: Smooth::new(1.0, 15.0, sr),
            master_s: Smooth::new(0.7, 15.0, sr),
        }
    }

    /// À appeler une fois par bloc : recalcule les filtres.
    pub fn update(&mut self, p: &Values) {
        self.od.update(p);
        self.amp.update(p);
        self.acoustic.update(p);
    }

    /// Traite un échantillon de guitare (mono).
    #[inline]
    pub fn process(&mut self, x: f32, p: &Values) -> f32 {
        let mut y = x * self.input_s.next(10f32.powf(input_db(p.input) / 20.0));
        let th = if p.gate_on > 0.5 { 0.002 + p.gate_th / 10.0 * 0.05 } else { 0.0 };
        y = self.gate.process(y, th);
        if p.inst > 0.5 {
            // Instrument acoustique : corps + égaliseur, sans distorsion ni ampli.
            y = self.acoustic.process(y);
            y = self.amp.eq(y);
            if p.ch_on > 0.5 {
                y = self.chorus.process(y, p);
            }
        } else {
            if p.od_on > 0.5 {
                y = self.od.process(y, p);
            }
            if p.ch_on > 0.5 {
                y = self.chorus.process(y, p);
            }
            y = self.amp.process(y, p);
        }
        if p.dl_on > 0.5 {
            y = self.echo.process(y, p);
        }
        if p.rv_on > 0.5 {
            y = self.reverb.process(y, p);
        }
        soft_limit(y * self.master_s.next((p.master / 10.0).powi(2) * 2.8))
    }

    #[inline]
    pub fn metronome(&mut self, p: &Values) -> (f32, Option<u32>) {
        self.metro.process(p)
    }
}

// ---------------------------------------------------------------- accordeur

/// Détection de hauteur par autocorrélation. Retourne la fréquence en Hz, ou None si le signal est trop faible.
pub fn detect_pitch(buf: &[f32], sr: f32) -> Option<f32> {
    let energy: f32 = buf.iter().map(|v| v * v).sum();
    let rms = (energy / buf.len() as f32).sqrt();
    // Seuil bas : un câble USB donne un signal faible. Le bruit est écarté plus bas par le test de clarté.
    if rms < 0.0012 {
        return None;
    }
    let min_lag = (sr / 1000.0) as usize;
    let max_lag = ((sr / 70.0) as usize).min(buf.len() / 2);
    let mut c = vec![0.0f32; max_lag + 1];
    for (lag, slot) in c.iter_mut().enumerate().take(max_lag).skip(min_lag) {
        let mut s = 0.0;
        for i in 0..buf.len() - lag {
            s += buf[i] * buf[i + lag];
        }
        *slot = s;
    }
    let is_peak = |l: usize| c[l] > c[l - 1] && c[l] >= c[l + 1];
    let mut best = 0usize;
    let mut best_v = 0.0f32;
    for l in (min_lag + 1)..(max_lag - 1) {
        if is_peak(l) && c[l] > best_v {
            best = l;
            best_v = c[l];
        }
    }
    if best == 0 {
        return None;
    }
    // Clarté : le pic doit représenter une vraie périodicité (sinon c'est du bruit).
    if c[best] < 0.45 * energy {
        return None;
    }
    // Préférer le plus petit pic proche du maximum : évite les erreurs d'octave.
    for l in (min_lag + 1)..best {
        if is_peak(l) && c[l] > best_v * 0.9 {
            best = l;
            break;
        }
    }
    let (a, b, d) = (c[best - 1], c[best], c[best + 1]);
    let denom = 2.0 * (a - 2.0 * b + d);
    let shift = if denom.abs() > 1e-12 { (a - d) / denom } else { 0.0 };
    Some(sr / (best as f32 + shift))
}

pub const NOTE_NAMES: [&str; 12] = ["Do", "Do#", "Ré", "Ré#", "Mi", "Fa", "Fa#", "Sol", "Sol#", "La", "La#", "Si"];

/// (nom de la note, écart en cents) pour une fréquence.
pub fn note_info(freq: f32) -> (String, f32) {
    let midi = 69.0 + 12.0 * (freq / 440.0).log2();
    let n = midi.round();
    let cents = (midi - n) * 100.0;
    let idx = (n as i32).rem_euclid(12) as usize;
    let octave = (n as i32) / 12 - 1;
    (format!("{}{}", NOTE_NAMES[idx], octave), cents)
}

/// Nom d'une note MIDI, par exemple 40 -> « Mi2 ».
pub fn midi_name(midi: i32) -> String {
    format!("{}{}", NOTE_NAMES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;

    const SR: f32 = 48000.0;

    fn sine(freq: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * freq * i as f32 / SR).sin() * amp).collect()
    }

    #[test]
    fn lowpass_attenuates_high_frequencies() {
        let mut f = Biquad::new(FilterKind::LowPass, SR, 1000.0, 0.7, 0.0);
        let out: Vec<f32> = sine(10000.0, 4800, 1.0).into_iter().map(|x| f.process(x)).collect();
        let peak = out[2400..].iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(peak < 0.05, "peak={peak}");
    }

    #[test]
    fn soft_limit_is_transparent_then_bounded() {
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.79), -0.79);
        for i in 0..2000 {
            let x = i as f32 / 10.0;
            assert!(soft_limit(x) <= 1.0 && soft_limit(-x) >= -1.0);
        }
        assert!(soft_limit(0.95) > 0.8 && soft_limit(0.95) < 1.0);
        assert!(soft_limit(1.0) > soft_limit(0.9)); // reste croissant
    }

    #[test]
    fn no_dc_offset_even_without_cabinet() {
        let sr = 44100.0;
        for model in 0..4 {
            let p = Params::new();
            p.amp_model.set(model as f32);
            p.cab_on.set(0.0);
            for n in ["amp_gain", "bass", "master"] {
                p.set_by_name(n, 10.0);
            }
            let v = p.read();
            let mut chain = Chain::new(sr);
            chain.update(&v);
            let (mut sum, mut n) = (0.0f32, 0usize);
            for i in 0..(sr * 3.0) as usize {
                let t = i as f32 / sr;
                let x = 0.1 * (2.0 * PI * 110.0 * t).sin() * (-t).exp();
                sum += chain.process(x, &v);
                n += 1;
            }
            assert!((sum / n as f32).abs() < 0.02, "modèle {model} : décalage {}", sum / n as f32);
        }
    }

    #[test]
    fn shape_is_bounded() {
        for i in -100..=100 {
            let y = shape(i as f32 / 10.0, 9.0, 0.03);
            assert!(y.is_finite() && y.abs() < 1.3);
        }
    }

    #[test]
    fn pitch_detection_on_guitar_strings() {
        for &f in &[82.41f32, 110.0, 146.83, 196.0, 246.94, 329.63] {
            let buf = sine(f, 4096, 0.3);
            let got = detect_pitch(&buf, SR).expect("pitch");
            assert!((got - f).abs() < 1.0, "attendu {f}, obtenu {got}");
        }
    }

    #[test]
    fn pitch_detection_with_harmonics() {
        let f = 110.0;
        let buf: Vec<f32> = (0..4096)
            .map(|i| {
                let t = i as f32 / SR;
                0.2 * (2.0 * PI * f * t).sin() + 0.15 * (2.0 * PI * 2.0 * f * t).sin() + 0.1 * (2.0 * PI * 3.0 * f * t).sin()
            })
            .collect();
        let got = detect_pitch(&buf, SR).expect("pitch");
        assert!((got - f).abs() < 1.5, "obtenu {got}");
    }

    #[test]
    fn tuner_works_with_a_very_weak_signal_but_not_on_noise() {
        // câble USB faible : crête 0,01 seulement
        for &f in &[82.41f32, 110.0, 196.0, 329.63] {
            let buf = sine(f, 4096, 0.01);
            let got = detect_pitch(&buf, SR).expect("signal faible");
            assert!((got - f).abs() < 1.0, "attendu {f}, obtenu {got}");
        }
        // bruit de fond (même niveau) : pas de note
        let mut x = 12345u32;
        let noise: Vec<f32> = (0..4096)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                ((x >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.02
            })
            .collect();
        assert!(detect_pitch(&noise, SR).is_none());
    }

    #[test]
    fn input_knob_mapping_roundtrips_and_has_headroom() {
        assert_eq!(input_db(5.0), 0.0);
        assert!((input_db(10.0) - 36.0).abs() < 1e-4);
        assert!((input_db(0.0) + 12.0).abs() < 1e-4);
        for db in [-12.0f32, -6.0, 0.0, 6.0, 20.0, 36.0] {
            assert!((input_db(input_knob_for_db(db)) - db).abs() < 1e-3);
        }
        assert_eq!(input_knob_for_db(60.0), 10.0);
    }

    #[test]
    fn weak_input_still_gets_a_usable_sound() {
        // Câble faible (crête 0,03) : avec le potard d'entrée à +30 dB, Metal doit saturer à un bon volume.
        let params = Params::new();
        params.amp_model.set(3.0);
        params.set_by_name("input", input_knob_for_db(30.0));
        let p = params.read();
        let mut chain = Chain::new(SR);
        chain.update(&p);
        let sig = sine(196.0, 48000, 0.03);
        let out: Vec<f32> = sig.iter().map(|&x| chain.process(x, &p)).collect();
        let level = (out[24000..].iter().map(|v| v * v).sum::<f32>() / 24000.0).sqrt();
        assert!(level > 0.1, "niveau trop faible : {level}");
    }

    #[test]
    fn silence_has_no_pitch() {
        assert!(detect_pitch(&vec![0.0; 4096], SR).is_none());
    }

    #[test]
    fn note_names() {
        let (n, c) = note_info(440.0);
        assert_eq!(n, "La4");
        assert!(c.abs() < 0.1);
        let (n, _) = note_info(82.41);
        assert_eq!(n, "Mi2");
        assert_eq!(midi_name(40), "Mi2");
        assert_eq!(midi_name(69), "La4");
        let (_, c) = note_info(442.0);
        assert!(c > 7.0 && c < 9.0);
    }

    #[test]
    fn all_models_and_effects_stay_finite_and_bounded() {
        let params = Params::new();
        for model in 0..4 {
            params.amp_model.set(model as f32);
            params.amp_gain.set(10.0);
            params.od_on.set(1.0);
            params.ch_on.set(1.0);
            params.dl_on.set(1.0);
            params.rv_on.set(1.0);
            params.metro_on.set(1.0);
            let p = params.read();
            let mut chain = Chain::new(SR);
            chain.update(&p);
            let input = sine(196.0, 48000, 0.5);
            let mut peak = 0.0f32;
            for x in input {
                let y = chain.process(x, &p) + chain.metronome(&p).0;
                assert!(y.is_finite(), "modèle {model}");
                peak = peak.max(y.abs());
            }
            assert!(peak > 0.01, "modèle {model} muet");
            assert!(peak < 6.0, "modèle {model} trop fort: {peak}");
        }
    }

    fn rms_through(inst: f32, freq: f32) -> f32 {
        let params = Params::new();
        params.inst.set(inst);
        params.gate_on.set(0.0);
        params.master.set(10.0);
        let p = params.read();
        let mut chain = Chain::new(SR);
        chain.update(&p);
        let out: Vec<f32> = sine(freq, 24000, 0.05).into_iter().map(|x| chain.process(x, &p)).collect();
        (out[12000..].iter().map(|v| v * v).sum::<f32>() / 12000.0).sqrt()
    }

    #[test]
    fn acoustic_instruments_have_their_own_character() {
        // Banjo : beaucoup plus de médiums-aigus que de graves
        assert!(rms_through(5.0, 1200.0) > 3.0 * rms_through(5.0, 100.0));
        // Ukulélé : graves coupés
        assert!(rms_through(4.0, 80.0) < 0.3 * rms_through(4.0, 500.0));
        // Jumbo country : plus de graves que la classique
        assert!(rms_through(3.0, 90.0) > rms_through(1.0, 90.0));
        // Tous les instruments produisent du son fini
        for inst in 1..=5 {
            let r = rms_through(inst as f32, 220.0);
            assert!(r.is_finite() && r > 0.001, "inst {inst}");
        }
    }

    #[test]
    fn gate_silences_low_level_noise() {
        let params = Params::new();
        params.gate_th.set(10.0);
        let p = params.read();
        let mut chain = Chain::new(SR);
        chain.update(&p);
        let mut peak = 0.0f32;
        for (i, x) in sine(100.0, 48000, 0.0005).into_iter().enumerate() {
            let y = chain.process(x, &p);
            if i > 24000 {
                peak = peak.max(y.abs());
            }
        }
        assert!(peak < 1e-4, "peak={peak}");
    }

    #[test]
    fn preset_roundtrip_skips_metronome_and_mute() {
        let a = Params::new();
        a.amp_gain.set(8.0);
        a.metro_bpm.set(180.0);
        let snap = a.snapshot();
        assert!(!snap.contains_key("metro_bpm") && !snap.contains_key("mute"));
        let b = Params::new();
        b.load(&snap);
        assert_eq!(b.amp_gain.get(), 8.0);
        assert_eq!(b.metro_bpm.get(), 100.0);
        // le niveau d'entrée n'est pas dans les presets : charger un preset ne doit pas dérégler le câble
        a.input.set(9.0);
        let c = Params::new();
        c.input.set(7.0);
        c.load(&a.snapshot());
        assert_eq!(c.input.get(), 7.0);
    }
}

// ---------------------------------------------------------------- campagne de tests « tous les sons »

#[cfg(test)]
mod sweep {
    use super::*;
    use crate::params::Params;
    use std::time::Instant;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    /// Note de guitare pincée : harmoniques décroissantes, extinction progressive.
    fn pluck(freq: f32, secs: f32, sr: f32, amp: f32) -> Vec<f32> {
        (0..(secs * sr) as usize)
            .map(|i| {
                let t = i as f32 / sr;
                let mut v = 0.0;
                for k in 1..=6 {
                    v += (2.0 * PI * freq * k as f32 * t).sin() / k as f32 * (-t * (1.5 + k as f32)).exp();
                }
                v * amp * (1.0 - (-t * 400.0).exp())
            })
            .collect()
    }

    fn riff(sr: f32, amp: f32) -> Vec<f32> {
        let mut out = Vec::new();
        for f in [82.41, 110.0, 146.83, 196.0, 246.94, 329.63] {
            out.extend(pluck(f, 0.35, sr, amp));
        }
        // accord de mi majeur
        let chord: Vec<Vec<f32>> = [82.41, 123.47, 164.81, 207.65, 246.94, 329.63].iter().map(|&f| pluck(f, 1.2, sr, amp * 0.5)).collect();
        for i in 0..chord[0].len() {
            out.push(chord.iter().map(|c| c[i]).sum());
        }
        out
    }

    fn noise(n: usize, amp: f32, seed: u64) -> Vec<f32> {
        let mut r = Rng(seed);
        (0..n).map(|_| (r.next() * 2.0 - 1.0) * amp).collect()
    }

    fn render(params: &Params, input: &[f32], sr: f32, mut jitter: Option<&mut Rng>) -> Vec<f32> {
        let mut chain = Chain::new(sr);
        let mut out = Vec::with_capacity(input.len());
        for block in input.chunks(256) {
            if let Some(r) = jitter.as_deref_mut() {
                // Un potard bouge pendant le jeu
                let names = ["amp_gain", "bass", "mid", "treble", "master", "dl_time", "dl_fb", "rv_size", "od_drive", "body"];
                let n = names[(r.next() * names.len() as f32) as usize % names.len()];
                params.set_by_name(n, r.next() * 10.0);
            }
            let p = params.read();
            chain.update(&p);
            for &x in block {
                out.push(chain.process(x.clamp(-1.0, 1.0), &p));
            }
        }
        out
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }
    fn db(x: f32) -> f32 {
        20.0 * x.max(1e-9).log10()
    }
    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0, |a, x| a.max(x.abs()))
    }

    /// Énergie (dB) dans trois bandes : graves < 250 Hz, médiums, aigus > 2,5 kHz.
    fn bands(v: &[f32], sr: f32) -> (f32, f32, f32) {
        let mut lp = Biquad::new(FilterKind::LowPass, sr, 250.0, 0.7, 0.0);
        let mut hp1 = Biquad::new(FilterKind::HighPass, sr, 250.0, 0.7, 0.0);
        let mut lp2 = Biquad::new(FilterKind::LowPass, sr, 2500.0, 0.7, 0.0);
        let mut hp2 = Biquad::new(FilterKind::HighPass, sr, 2500.0, 0.7, 0.0);
        let low: Vec<f32> = v.iter().map(|&x| lp.process(x)).collect();
        let mid: Vec<f32> = v.iter().map(|&x| lp2.process(hp1.process(x))).collect();
        let high: Vec<f32> = v.iter().map(|&x| hp2.process(x)).collect();
        (db(rms(&low)), db(rms(&mid)), db(rms(&high)))
    }

    fn all_effects_on(p: &Params) {
        for n in ["od_on", "ch_on", "dl_on", "rv_on", "cab_on", "gate_on"] {
            p.set_by_name(n, 1.0);
        }
    }

    /// Les 10 « sons » : 4 amplis électriques + 5 instruments acoustiques (+ ampli par défaut).
    fn sounds() -> Vec<(String, f32, f32)> {
        let mut v: Vec<(String, f32, f32)> = ["Clean", "Crunch", "Lead", "Metal"].iter().enumerate().map(|(i, n)| (format!("Électrique {n}"), 0.0, i as f32)).collect();
        for (i, n) in ["Classique", "Folk", "Country jumbo", "Ukulélé", "Banjo"].iter().enumerate() {
            v.push((format!("Acoustique {n}"), 1.0 + i as f32, 1.0));
        }
        v
    }

    fn sound_params(inst: f32, model: f32) -> Params {
        let p = Params::new();
        p.inst.set(inst);
        p.amp_model.set(model);
        p
    }

    #[test]
    fn every_sound_every_extreme_every_signal() {
        let mut combos = 0;
        for sr in [44100.0f32, 48000.0, 96000.0] {
            let signals: Vec<(&str, Vec<f32>)> = vec![
                ("riff doux", riff(sr, 0.1)),
                ("riff fort", riff(sr, 0.9)),
                ("bruit fort", noise((sr * 1.0) as usize, 1.0, 7)),
                ("silence", vec![0.0; (sr * 1.0) as usize]),
            ];
            for (name, inst, model) in sounds() {
                for knobs in [0.0f32, 5.0, 10.0] {
                    for effects in [false, true] {
                        let p = sound_params(inst, model);
                        for n in ["amp_gain", "bass", "mid", "treble", "presence", "master", "body", "input", "od_drive", "od_tone", "od_level", "ch_rate", "ch_depth", "ch_mix", "dl_time", "dl_fb", "dl_mix", "rv_size", "rv_mix", "gate_th"] {
                            p.set_by_name(n, knobs);
                        }
                        if effects {
                            all_effects_on(&p);
                        } else {
                            p.set_by_name("gate_on", 0.0);
                        }
                        for (sname, sig) in &signals {
                            let out = render(&p, sig, sr, None);
                            let ctx = format!("{name} | {sr} Hz | potards {knobs} | effets {effects} | {sname}");
                            assert!(out.iter().all(|x| x.is_finite()), "NaN/inf : {ctx}");
                            let pk = peak(&out);
                            assert!(pk <= 1.001, "le limiteur laisse passer {pk} : {ctx}");
                            if *sname == "silence" {
                                assert!(pk < 1e-3, "bruit sur silence ({pk}) : {ctx}");
                            }
                            let mean = out.iter().sum::<f32>() / out.len() as f32;
                            // Tout à fond (potards à 10) : le signal dépasse 10 fois le maximum, le limiteur écrase des pics asymétriques.
                            let max_dc = if knobs >= 10.0 { 0.25 } else { 0.05 };
                            assert!(mean.abs() < max_dc, "décalage continu ({mean}) : {ctx}");
                            combos += 1;
                        }
                    }
                }
            }
        }
        println!("Combinaisons testées : {combos}");
    }

    #[test]
    fn tails_always_decay_never_grow() {
        // Delay (répétitions max, 1 s) et reverb au maximum : après le jeu, l'énergie doit décroître régulièrement.
        let sr = 48000.0;
        for (name, inst, model) in sounds() {
            let p = sound_params(inst, model);
            all_effects_on(&p);
            for n in ["dl_fb", "dl_mix", "dl_time", "rv_size", "rv_mix", "master"] {
                p.set_by_name(n, 10.0);
            }
            let play = riff(sr, 0.5).len();
            let mut sig = riff(sr, 0.5);
            sig.extend(vec![0.0; (sr * 30.0) as usize]);
            let out = render(&p, &sig, sr, None);
            let win = |from_s: f32| {
                let a = play + (from_s * sr) as usize;
                rms(&out[a..a + (sr * 2.0) as usize])
            };
            let (w5, w15, w25) = (win(5.0), win(15.0), win(25.0));
            assert!(w15 < w5 * 0.9 && w25 < w15 * 0.9, "{name} : la queue ne décroît pas ({:.1}, {:.1}, {:.1} dBFS)", db(w5), db(w15), db(w25));
            println!("  {name:<28} queue : {:>6.1} -> {:>6.1} -> {:>6.1} dBFS (à +5 s, +15 s, +25 s)", db(w5), db(w15), db(w25));
        }
        // Avec des réglages raisonnables (répétitions moyennes), le silence revient vite
        let p = Params::new();
        all_effects_on(&p);
        let mut sig = riff(sr, 0.3);
        sig.extend(vec![0.0; (sr * 15.0) as usize]);
        let out = render(&p, &sig, sr, None);
        assert!(rms(&out[out.len() - (sr * 2.0) as usize..]) < 1e-3, "réglages normaux : la queue devrait être éteinte");
    }

    #[test]
    fn knobs_moving_while_playing_never_break_the_sound() {
        let sr = 48000.0;
        let mut r = Rng(99);
        for (name, inst, model) in sounds() {
            let p = sound_params(inst, model);
            all_effects_on(&p);
            let out = render(&p, &riff(sr, 0.4), sr, Some(&mut r));
            assert!(out.iter().all(|x| x.is_finite()), "{name}");
            assert!(peak(&out) <= 1.001, "{name}");
        }
    }

    #[test]
    fn switching_sound_on_the_fly_is_safe() {
        // On change d'instrument et d'ampli en pleine note, comme avec les boutons de l'interface.
        let sr = 48000.0;
        let sig = riff(sr, 0.5);
        let p = Params::new();
        all_effects_on(&p);
        let mut chain = Chain::new(sr);
        let mut count = 0usize;
        for block in sig.chunks(256) {
            p.inst.set((count % 6) as f32);
            p.amp_model.set((count % 4) as f32);
            let v = p.read();
            chain.update(&v);
            for &x in block {
                let y = chain.process(x, &v);
                assert!(y.is_finite() && y.abs() <= 1.001);
            }
            count += 1;
        }
    }

    #[test]
    fn performance_is_comfortably_real_time() {
        let sr = 48000.0;
        let secs = 20.0;
        let sig = noise((sr * secs) as usize, 0.3, 3);
        let mut worst = 0.0f32;
        for (name, inst, model) in sounds() {
            let p = sound_params(inst, model);
            all_effects_on(&p);
            let t = Instant::now();
            let out = render(&p, &sig, sr, None);
            let took = t.elapsed().as_secs_f32();
            assert!(out.len() == sig.len());
            let xrt = secs / took;
            println!("  {name:<28} {xrt:>7.0}x temps réel (tous effets actifs)");
            worst = worst.max(took / secs);
            if !cfg!(debug_assertions) || true {
                assert!(xrt > 20.0, "{name} trop lent : {xrt}x");
            }
        }
        println!("  charge processeur la pire : {:.2} % d'un cœur", worst * 100.0);
    }

    #[test]
    fn character_table() {
        let sr = 48000.0;
        let sig = riff(sr, 0.3);
        println!();
        println!("  {:<28} {:>7} {:>8} {:>8} {:>8} {:>8}", "Son (réglages par défaut)", "pic", "niveau", "graves", "médiums", "aigus");
        let mut levels = Vec::new();
        for (name, inst, model) in sounds() {
            let p = sound_params(inst, model);
            p.set_by_name("gate_on", 0.0);
            let out = render(&p, &sig, sr, None);
            let (l, m, h) = bands(&out, sr);
            let lvl = db(rms(&out));
            levels.push((name.clone(), lvl));
            println!("  {name:<28} {:>6.2} {:>7.1} {:>8.1} {:>8.1} {:>8.1}", peak(&out), lvl, l, m, h);
        }
        // Les sons doivent rester dans une plage de volume comparable (pour ne pas casser les oreilles en changeant de son)
        let min = levels.iter().map(|x| x.1).fold(f32::MAX, f32::min);
        let max = levels.iter().map(|x| x.1).fold(f32::MIN, f32::max);
        println!("  écart de volume entre les sons : {:.1} dB", max - min);
        assert!(max - min < 18.0, "écart de volume trop grand : {:.1} dB", max - min);
    }

    #[test]
    fn long_random_fuzz() {
        let secs: f32 = std::env::var("FUZZ_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(2.0);
        let start = Instant::now();
        let mut r = Rng(0x9E3779B97F4A7C15);
        let (mut runs, mut worst_peak) = (0u64, 0.0f32);
        let names: Vec<String> = Params::new().snapshot().keys().cloned().collect();
        while start.elapsed().as_secs_f32() < secs {
            let p = Params::new();
            for n in &names {
                let v = if n.ends_with("_on") || n == "cab_on" {
                    if r.next() > 0.5 { 1.0 } else { 0.0 }
                } else if n == "inst" {
                    (r.next() * 6.0).floor().min(5.0)
                } else if n == "amp_model" {
                    (r.next() * 4.0).floor().min(3.0)
                } else {
                    r.next() * 10.0
                };
                p.set_by_name(n, v);
            }
            p.set_by_name("metro_on", 1.0);
            p.set_by_name("metro_bpm", 40.0 + r.next() * 180.0);
            let sr = [22050.0f32, 44100.0, 48000.0, 96000.0][(r.next() * 4.0) as usize % 4];
            let sig = match (r.next() * 4.0) as usize {
                0 => riff(sr, 0.05 + r.next() * 0.9),
                1 => noise((sr * 1.5) as usize, r.next(), runs + 1),
                2 => pluck(70.0 + r.next() * 700.0, 1.5, sr, r.next()),
                _ => vec![0.0; (sr * 0.5) as usize],
            };
            let out = render(&p, &sig, sr, Some(&mut r));
            assert!(out.iter().all(|x| x.is_finite()), "NaN avec {:?}", p.snapshot());
            let pk = peak(&out);
            assert!(pk <= 1.001, "le limiteur laisse passer {pk} avec {:?}", p.snapshot());
            worst_peak = worst_peak.max(pk);
            runs += 1;
        }
        println!("  fuzz : {runs} configurations aléatoires en {:.0} s, pic maximal {:.2}, aucune anomalie", start.elapsed().as_secs_f32(), worst_peak);
    }
}


