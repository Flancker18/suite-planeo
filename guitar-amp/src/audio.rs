//! Entrée guitare -> chaîne d'effets -> sortie, via cpal (WASAPI sur Windows).
use crate::dsp::Chain;
use crate::params::{AtomicF32, Params};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedBufferSize};
use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::Arc;

/// État partagé entre le thread audio et l'interface.
pub struct Shared {
    pub params: Params,
    pub in_peak: AtomicF32,
    pub out_peak: AtomicF32,
    pub beat: AtomicU32,
    pub rec_on: AtomicBool,
    pub tuner_q: ArrayQueue<f32>,
    pub rec_q: ArrayQueue<f32>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            params: Params::new(),
            in_peak: AtomicF32::new(0.0),
            out_peak: AtomicF32::new(0.0),
            beat: AtomicU32::new(0),
            rec_on: AtomicBool::new(false),
            tuner_q: ArrayQueue::new(4096),
            rec_q: ArrayQueue::new(1 << 18),
        }
    }
}

pub struct Engine {
    _input: Stream,
    _output: Stream,
    _output2: Option<Stream>,
    pub output2_name: Option<String>,
    pub sample_rate: u32,
    pub buffer: u32,
    pub input_name: String,
    pub output_name: String,
    pub input_rate: u32,
}

/// Convertit le flux d'entrée (à la fréquence de la carte d'entrée) vers la fréquence de la sortie,
/// par interpolation linéaire. Un câble guitare USB est souvent en 44,1 kHz alors que la sortie est en 48 kHz.
pub struct Resampler {
    ratio: f32,
    frac: f32,
    prev: f32,
    cur: f32,
}

impl Resampler {
    /// `ratio` = fréquence d'entrée / fréquence de sortie.
    pub fn new(ratio: f32) -> Self {
        Self { ratio: ratio.clamp(0.25, 4.0), frac: 1.0, prev: 0.0, cur: 0.0 }
    }
    /// Produit un échantillon de sortie ; `pop` fournit le prochain échantillon d'entrée.
    #[inline]
    pub fn next(&mut self, mut pop: impl FnMut() -> f32) -> f32 {
        while self.frac >= 1.0 {
            self.prev = self.cur;
            self.cur = pop();
            self.frac -= 1.0;
        }
        let y = self.prev + (self.cur - self.prev) * self.frac;
        self.frac += self.ratio;
        y
    }
}

/// Repère un câble guitare / interface USB dans la liste des entrées (par son nom).
pub fn guitar_input_index(names: &[String]) -> Option<usize> {
    const KEYS: [&str; 6] = ["usb", "guitar", "guitare", "interface", "link", "codec"];
    names.iter().position(|n| {
        let n = n.to_lowercase();
        KEYS.iter().any(|k| n.contains(k))
    })
}

pub fn device_names() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let names = |it: Result<Vec<cpal::Device>, String>| it.unwrap_or_default().iter().filter_map(|d| d.name().ok()).collect::<Vec<_>>();
    let ins = names(host.input_devices().map(|i| i.collect()).map_err(|e| e.to_string()));
    let outs = names(host.output_devices().map(|i| i.collect()).map_err(|e| e.to_string()));
    (ins, outs)
}

fn find_device(host: &cpal::Host, name: Option<&str>, input: bool) -> Result<cpal::Device, String> {
    if let Some(n) = name {
        let mut it = if input { host.input_devices() } else { host.output_devices() }.map_err(|e| e.to_string())?;
        if let Some(d) = it.find(|d| d.name().map(|x| x == n).unwrap_or(false)) {
            return Ok(d);
        }
    }
    let d = if input { host.default_input_device() } else { host.default_output_device() };
    d.ok_or_else(|| if input { "Aucune entrée audio trouvée".into() } else { "Aucune sortie audio trouvée".into() })
}

fn fixed_buffer(range: &SupportedBufferSize, wanted: u32) -> BufferSize {
    match range {
        SupportedBufferSize::Range { min, max } => BufferSize::Fixed(wanted.clamp(*min, *max)),
        SupportedBufferSize::Unknown => BufferSize::Default,
    }
}

fn build_input<T>(dev: &cpal::Device, cfg: &StreamConfig, link: Arc<ArrayQueue<f32>>) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let ch = cfg.channels as usize;
    dev.build_input_stream(
        cfg,
        move |data: &[T], _| {
            for frame in data.chunks(ch) {
                link.force_push(f32::from_sample(frame[0]));
            }
        },
        |e| eprintln!("Erreur entrée audio : {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}

fn build_output<T>(
    dev: &cpal::Device,
    cfg: &StreamConfig,
    link: Arc<ArrayQueue<f32>>,
    shared: Arc<Shared>,
    max_keep: usize,
    ratio: f32,
    tee: Option<Arc<ArrayQueue<f32>>>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let ch = cfg.channels as usize;
    let mut chain = Chain::new(cfg.sample_rate.0 as f32);
    let mut resampler = Resampler::new(ratio);
    dev.build_output_stream(
        cfg,
        move |data: &mut [T], _| {
            let p = shared.params.read();
            chain.update(&p);
            // Garde la latence bornée : on jette les plus vieux échantillons si l'entrée a pris de l'avance.
            while link.len() > max_keep {
                link.pop();
            }
            let recording = shared.rec_on.load(Relaxed);
            let (mut in_pk, mut out_pk) = (0.0f32, 0.0f32);
            for frame in data.chunks_mut(ch) {
                let x = resampler.next(|| link.pop().unwrap_or(0.0));
                in_pk = in_pk.max(x.abs());
                shared.tuner_q.force_push(x);
                let y = chain.process(x, &p);
                if recording {
                    shared.rec_q.force_push(y);
                }
                let (click, beat) = chain.metronome(&p);
                if let Some(b) = beat {
                    shared.beat.store(b, Relaxed);
                }
                let mut o = if p.mute > 0.5 { 0.0 } else { y + click };
                if o.abs() > 0.9 {
                    o = o.signum() * (0.9 + 0.1 * ((o.abs() - 0.9) / 0.1).tanh());
                }
                out_pk = out_pk.max(o.abs());
                if let Some(t) = &tee {
                    t.force_push(o);
                }
                for s in frame.iter_mut() {
                    *s = T::from_sample(o);
                }
            }
            shared.in_peak.set(in_pk);
            shared.out_peak.set(out_pk);
        },
        |e| eprintln!("Erreur sortie audio : {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}

/// Seconde sortie : rejoue le son déjà traité par la sortie principale (par exemple sur la prise jack du casque).
fn build_follower<T>(dev: &cpal::Device, cfg: &StreamConfig, link: Arc<ArrayQueue<f32>>, max_keep: usize, ratio: f32) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let ch = cfg.channels as usize;
    let mut resampler = Resampler::new(ratio);
    dev.build_output_stream(
        cfg,
        move |data: &mut [T], _| {
            while link.len() > max_keep {
                link.pop();
            }
            for frame in data.chunks_mut(ch) {
                let o = resampler.next(|| link.pop().unwrap_or(0.0));
                for s in frame.iter_mut() {
                    *s = T::from_sample(o);
                }
            }
        },
        |e| eprintln!("Erreur seconde sortie : {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}

pub fn start(shared: &Arc<Shared>, in_name: Option<&str>, out_name: Option<&str>, out2_name: Option<&str>, buffer: u32) -> Result<Engine, String> {
    let host = cpal::default_host();
    let in_dev = find_device(&host, in_name, true)?;
    let out_dev = find_device(&host, out_name, false)?;

    let out_def = out_dev.default_output_config().map_err(|e| format!("Sortie : {e}"))?;
    let sr = out_def.sample_rate().0;
    let out_fmt = out_def.sample_format();
    let out_buf = fixed_buffer(out_def.buffer_size(), buffer);
    let mut out_cfg = out_def.config();

    // L'entrée garde sa propre fréquence (format du mélangeur Windows) ; on convertit ensuite.
    let in_def = in_dev.default_input_config().map_err(|e| format!("Entrée : {e}"))?;
    let in_sr = in_def.sample_rate().0;
    let in_fmt = in_def.sample_format();
    let in_buf = fixed_buffer(in_def.buffer_size(), buffer);
    let mut in_cfg = in_def.config();

    // Seconde sortie éventuelle (différente de la première)
    let out_name_used = out_dev.name().unwrap_or_default();
    let out2 = match out2_name {
        Some(n) if n != out_name_used => {
            let d = host
                .output_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.name().map(|x| x == n).unwrap_or(false))
                .ok_or_else(|| format!("Seconde sortie introuvable : {n}"))?;
            let def = d.default_output_config().map_err(|e| format!("Seconde sortie : {e}"))?;
            let buf = fixed_buffer(def.buffer_size(), buffer);
            Some((d, def.sample_format(), def.sample_rate().0, buf, def.config()))
        }
        _ => None,
    };
    let tee = out2.as_ref().map(|_| Arc::new(ArrayQueue::<f32>::new(16384)));

    let link = Arc::new(ArrayQueue::<f32>::new(16384));
    let max_keep = buffer as usize * 2 + 256;
    let ratio = in_sr as f32 / sr as f32;

    // Première tentative avec le petit buffer demandé, sinon buffer par défaut du pilote.
    let mut build = |ib: BufferSize, ob: BufferSize| -> Result<(Stream, Stream), String> {
        in_cfg.buffer_size = ib;
        out_cfg.buffer_size = ob;
        let i = match in_fmt {
            SampleFormat::F32 => build_input::<f32>(&in_dev, &in_cfg, link.clone())?,
            SampleFormat::I16 => build_input::<i16>(&in_dev, &in_cfg, link.clone())?,
            SampleFormat::I32 => build_input::<i32>(&in_dev, &in_cfg, link.clone())?,
            f => return Err(format!("Format d'entrée non géré : {f:?}")),
        };
        let o = match out_fmt {
            SampleFormat::F32 => build_output::<f32>(&out_dev, &out_cfg, link.clone(), shared.clone(), max_keep, ratio, tee.clone())?,
            SampleFormat::I16 => build_output::<i16>(&out_dev, &out_cfg, link.clone(), shared.clone(), max_keep, ratio, tee.clone())?,
            SampleFormat::I32 => build_output::<i32>(&out_dev, &out_cfg, link.clone(), shared.clone(), max_keep, ratio, tee.clone())?,
            f => return Err(format!("Format de sortie non géré : {f:?}")),
        };
        Ok((i, o))
    };
    let (input, output) = build(in_buf, out_buf).or_else(|_| build(BufferSize::Default, BufferSize::Default))?;
    // Seconde sortie : mêmes essais (petit buffer, puis buffer par défaut)
    let output2 = match (&out2, &tee) {
        (Some((d, fmt, sr2, buf, cfg)), Some(t)) => {
            let ratio2 = sr as f32 / *sr2 as f32;
            let mk = |cfg: &StreamConfig| -> Result<Stream, String> {
                match fmt {
                    SampleFormat::F32 => build_follower::<f32>(d, cfg, t.clone(), max_keep, ratio2),
                    SampleFormat::I16 => build_follower::<i16>(d, cfg, t.clone(), max_keep, ratio2),
                    SampleFormat::I32 => build_follower::<i32>(d, cfg, t.clone(), max_keep, ratio2),
                    f => Err(format!("Format de seconde sortie non géré : {f:?}")),
                }
            };
            let mut c = cfg.clone();
            c.buffer_size = *buf;
            let st = mk(&c).or_else(|_| {
                c.buffer_size = BufferSize::Default;
                mk(&c)
            })?;
            Some(st)
        }
        _ => None,
    };
    input.play().map_err(|e| e.to_string())?;
    output.play().map_err(|e| e.to_string())?;
    if let Some(o2) = &output2 {
        o2.play().map_err(|e| e.to_string())?;
    }
    Ok(Engine {
        _input: input,
        _output: output,
        output2_name: output2.as_ref().and(out2_name.map(|n| n.to_string())),
        _output2: output2,
        sample_rate: sr,
        buffer,
        input_name: in_dev.name().unwrap_or_else(|_| "?".into()),
        output_name: out_dev.name().unwrap_or_else(|_| "?".into()),
        input_rate: in_sr,
    })
}

// ---------------------------------------------------------------- enregistrement WAV

pub struct Recorder {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
    pub path: String,
}

pub fn start_recording(shared: &Arc<Shared>, sample_rate: u32, dir: &std::path::Path) -> Result<Recorder, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Dossier inaccessible : {e}"))?;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("guitare-{secs}.wav")).to_string_lossy().into_owned();
    let spec = hound::WavSpec { channels: 1, sample_rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut writer = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
    while shared.rec_q.pop().is_some() {}
    shared.rec_on.store(true, Relaxed);
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2) = (shared.clone(), stop.clone());
    let handle = std::thread::spawn(move || loop {
        while let Some(x) = s2.rec_q.pop() {
            let _ = writer.write_sample(x);
        }
        if st2.load(Relaxed) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    });
    Ok(Recorder { shared: shared.clone(), stop, handle, path })
}

impl Recorder {
    pub fn finish(self) -> String {
        self.shared.rec_on.store(false, Relaxed);
        self.stop.store(true, Relaxed);
        let _ = self.handle.join();
        self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ratio: f32, n_out: usize, input: &[f32]) -> (Vec<f32>, usize) {
        let mut r = Resampler::new(ratio);
        let mut i = 0usize;
        let out: Vec<f32> = (0..n_out)
            .map(|_| {
                r.next(|| {
                    let v = input.get(i).copied().unwrap_or(0.0);
                    i += 1;
                    v
                })
            })
            .collect();
        (out, i)
    }

    #[test]
    fn same_rate_is_a_clean_passthrough() {
        let input: Vec<f32> = (0..100).map(|i| (i as f32 * 0.3).sin()).collect();
        let (out, _) = run(1.0, 90, &input);
        // retard fixe d'un échantillon, valeurs identiques
        for k in 1..90 {
            assert!((out[k] - input[k - 1]).abs() < 1e-6, "k={k}");
        }
    }

    #[test]
    fn consumption_follows_the_rate_ratio() {
        let input = vec![0.0f32; 100_000];
        for (inr, outr) in [(44100.0f32, 48000.0f32), (48000.0, 44100.0), (48000.0, 96000.0), (96000.0, 48000.0)] {
            let (_, used) = run(inr / outr, 48000, &input);
            let expected = 48000.0 * inr / outr;
            assert!((used as f32 - expected).abs() < 5.0, "{inr}->{outr} : {used} vs {expected}");
        }
    }

    #[test]
    fn upsampling_keeps_a_sine_pure_enough() {
        // 440 Hz capté à 44,1 kHz, restitué à 48 kHz : la fréquence doit rester 440 Hz
        let input: Vec<f32> = (0..44100).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44100.0).sin() * 0.5).collect();
        let (out, _) = run(44100.0 / 48000.0, 48000, &input);
        let crossings = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((crossings as i32 - 440).abs() <= 2, "{crossings} passages par zéro");
        let peak = out.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(peak > 0.48 && peak < 0.51, "pic {peak}");
    }

    #[test]
    fn usb_guitar_cable_is_recognised_by_name() {
        let names: Vec<String> = ["Microphone (Realtek High Definition Audio)", "Microphone (USB Audio Device)", "Stereo Mix"].iter().map(|s| s.to_string()).collect();
        assert_eq!(guitar_input_index(&names), Some(1));
        let names: Vec<String> = ["Microphone (Realtek)", "Stereo Mix"].iter().map(|s| s.to_string()).collect();
        assert_eq!(guitar_input_index(&names), None);
        let names: Vec<String> = ["Line (Guitar Link)"].iter().map(|s| s.to_string()).collect();
        assert_eq!(guitar_input_index(&names), Some(0));
    }
}
