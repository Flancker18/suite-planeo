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
        self.mid.set(FilterKind::Peaking, sr, 700.0, 0.9, knob_db(p.mid) + m.mid_shift);
        self.treble.set(FilterKind::HighShelf, sr, 3200.0, 0.7, knob_db(p.treble));
        self.presence.set(FilterKind::HighShelf, sr, 5500.0, 0.7, knob_db(p.presence) * 0.7);
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
        y = self.presence.process(self.treble.process(self.mid.process(self.bass.process(y))));
        if p.cab_on > 0.5 {
            y = self.cab_dip.process(self.cab_lp2.process(self.cab_lp1.process(self.cab_hp.process(y))));
        }
        y * m.trim
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
            echo: Echo::new(sr),
            reverb: Reverb::new(sr),
            metro: Metronome::new(sr),
            input_s: Smooth::new(1.0, 15.0, sr),
            master_s: Smooth::new(0.3, 15.0, sr),
        }
    }

    /// À appeler une fois par bloc : recalcule les filtres.
    pub fn update(&mut self, p: &Values) {
        self.od.update(p);
        self.amp.update(p);
    }

    /// Traite un échantillon de guitare (mono).
    #[inline]
    pub fn process(&mut self, x: f32, p: &Values) -> f32 {
        let mut y = x * self.input_s.next(10f32.powf(knob_db(p.input) / 20.0));
        let th = if p.gate_on > 0.5 { 0.002 + p.gate_th / 10.0 * 0.05 } else { 0.0 };
        y = self.gate.process(y, th);
        if p.od_on > 0.5 {
            y = self.od.process(y, p);
        }
        if p.ch_on > 0.5 {
            y = self.chorus.process(y, p);
        }
        y = self.amp.process(y, p);
        if p.dl_on > 0.5 {
            y = self.echo.process(y, p);
        }
        if p.rv_on > 0.5 {
            y = self.reverb.process(y, p);
        }
        y * self.master_s.next((p.master / 10.0).powi(2) * 1.4)
    }

    #[inline]
    pub fn metronome(&mut self, p: &Values) -> (f32, Option<u32>) {
        self.metro.process(p)
    }
}

// ---------------------------------------------------------------- accordeur

/// Détection de hauteur par autocorrélation. Retourne la fréquence en Hz, ou None si le signal est trop faible.
pub fn detect_pitch(buf: &[f32], sr: f32) -> Option<f32> {
    let rms = (buf.iter().map(|v| v * v).sum::<f32>() / buf.len() as f32).sqrt();
    if rms < 0.01 {
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
    }
}
