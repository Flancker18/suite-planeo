//! Entrée guitare -> chaîne d'effets -> sortie, via cpal (WASAPI sur Windows).
use crate::dsp::Chain;
use crate::params::{AtomicF32, Params};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, Sample, SampleFormat, SampleRate, SizedSample, Stream, StreamConfig, SupportedBufferSize};
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
    pub sample_rate: u32,
    pub buffer: u32,
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
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let ch = cfg.channels as usize;
    let mut chain = Chain::new(cfg.sample_rate.0 as f32);
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
                let x = link.pop().unwrap_or(0.0);
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

pub fn start(shared: &Arc<Shared>, in_name: Option<&str>, out_name: Option<&str>, buffer: u32) -> Result<Engine, String> {
    let host = cpal::default_host();
    let in_dev = find_device(&host, in_name, true)?;
    let out_dev = find_device(&host, out_name, false)?;

    let out_def = out_dev.default_output_config().map_err(|e| e.to_string())?;
    let sr = out_def.sample_rate().0;
    let out_fmt = out_def.sample_format();
    let out_buf = fixed_buffer(out_def.buffer_size(), buffer);
    let mut out_cfg = out_def.config();

    let in_sup = in_dev
        .supported_input_configs()
        .map_err(|e| e.to_string())?
        .filter(|c| c.min_sample_rate().0 <= sr && sr <= c.max_sample_rate().0)
        .min_by_key(|c| if c.sample_format() == SampleFormat::F32 { 0 } else { 1 })
        .ok_or_else(|| format!("L'entrée ne gère pas {sr} Hz : règle les deux périphériques sur la même fréquence dans Windows"))?;
    let in_fmt = in_sup.sample_format();
    let in_buf = fixed_buffer(in_sup.buffer_size(), buffer);
    let mut in_cfg = in_sup.with_sample_rate(SampleRate(sr)).config();

    let link = Arc::new(ArrayQueue::<f32>::new(16384));
    let max_keep = buffer as usize * 2 + 256;

    // Première tentative avec le petit buffer demandé, sinon buffer par défaut du pilote.
    let mut build = |ib: BufferSize, ob: BufferSize| -> Result<(Stream, Stream), String> {
        in_cfg.buffer_size = ib;
        out_cfg.buffer_size = ob;
        let i = match in_fmt {
            SampleFormat::F32 => build_input::<f32>(&in_dev, &in_cfg, link.clone())?,
            SampleFormat::I16 => build_input::<i16>(&in_dev, &in_cfg, link.clone())?,
            f => return Err(format!("Format d'entrée non géré : {f:?}")),
        };
        let o = match out_fmt {
            SampleFormat::F32 => build_output::<f32>(&out_dev, &out_cfg, link.clone(), shared.clone(), max_keep)?,
            SampleFormat::I16 => build_output::<i16>(&out_dev, &out_cfg, link.clone(), shared.clone(), max_keep)?,
            f => return Err(format!("Format de sortie non géré : {f:?}")),
        };
        Ok((i, o))
    };
    let (input, output) = build(in_buf, out_buf).or_else(|_| build(BufferSize::Default, BufferSize::Default))?;
    input.play().map_err(|e| e.to_string())?;
    output.play().map_err(|e| e.to_string())?;
    Ok(Engine { _input: input, _output: output, sample_rate: sr, buffer })
}

// ---------------------------------------------------------------- enregistrement WAV

pub struct Recorder {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
    pub path: String,
}

pub fn start_recording(shared: &Arc<Shared>, sample_rate: u32) -> Result<Recorder, String> {
    std::fs::create_dir_all("enregistrements").map_err(|e| e.to_string())?;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = format!("enregistrements/guitare-{secs}.wav");
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
