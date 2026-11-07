mod audio;
mod dsp;
mod params;

use audio::{Engine, Recorder, Shared};
use serde::{Deserialize, Serialize};
use slint::{ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::{Duration, Instant};

slint::include_modules!();

#[derive(Clone, Serialize, Deserialize)]
struct Preset {
    name: String,
    values: BTreeMap<String, f32>,
}

/// Presets d'usine, du jazz à Gojira. Les valeurs non citées gardent leur défaut.
fn factory_presets() -> Vec<Preset> {
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
        mk("Jazz (clean chaud)", &[("amp_model", 0.0), ("amp_gain", 3.0), ("bass", 6.5), ("mid", 5.0), ("treble", 3.0), ("presence", 3.0), ("rv_on", 1.0), ("rv_size", 4.0), ("rv_mix", 2.5), ("gate_on", 0.0)]),
        mk("Country (clean brillant)", &[("amp_model", 0.0), ("amp_gain", 4.0), ("bass", 4.0), ("mid", 6.0), ("treble", 7.5), ("presence", 6.0), ("dl_on", 1.0), ("dl_time", 1.0), ("dl_fb", 1.0), ("dl_mix", 2.5), ("gate_on", 0.0)]),
        mk("Dire Straits (clean/crunch)", &[("amp_model", 0.0), ("amp_gain", 6.5), ("bass", 4.5), ("mid", 6.0), ("treble", 7.0), ("presence", 6.5), ("ch_on", 1.0), ("ch_mix", 2.0), ("ch_rate", 2.0), ("rv_on", 1.0), ("rv_mix", 2.0)]),
        mk("Blues (crunch léger)", &[("amp_model", 1.0), ("amp_gain", 4.0), ("bass", 5.0), ("mid", 6.5), ("treble", 5.5), ("presence", 4.5), ("od_on", 1.0), ("od_drive", 3.0), ("od_tone", 5.0), ("od_level", 6.0), ("rv_on", 1.0), ("rv_mix", 2.5)]),
        mk("Rock (crunch)", &[("amp_model", 1.0), ("amp_gain", 7.0), ("bass", 5.5), ("mid", 6.0), ("treble", 6.0), ("presence", 5.5), ("dl_on", 1.0), ("dl_time", 3.5), ("dl_mix", 2.0)]),
        mk("Solo (lead)", &[("amp_model", 2.0), ("amp_gain", 6.5), ("bass", 4.5), ("mid", 7.0), ("treble", 5.5), ("presence", 5.5), ("dl_on", 1.0), ("dl_time", 4.0), ("dl_fb", 3.5), ("dl_mix", 3.0), ("rv_on", 1.0), ("rv_mix", 3.0)]),
        mk("Gojira (metal)", &[("amp_model", 3.0), ("amp_gain", 6.5), ("bass", 6.0), ("mid", 3.5), ("treble", 6.5), ("presence", 6.0), ("od_on", 1.0), ("od_drive", 1.0), ("od_tone", 6.5), ("od_level", 8.0), ("gate_on", 1.0), ("gate_th", 5.0)]),
    ]
}

fn presets_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("guitar-amp").join("presets.json"))
}

fn load_user_presets() -> Vec<Preset> {
    presets_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_user_presets(list: &[Preset]) -> Result<(), String> {
    let path = presets_path().ok_or("Dossier de configuration introuvable")?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(path, serde_json::to_string_pretty(list).unwrap()).map_err(|e| e.to_string())
}

fn settings_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("guitar-amp").join("settings.json"))
}

/// Dossier d'enregistrement : celui choisi la dernière fois, sinon Documents\Guitar Amp.
fn load_rec_dir() -> std::path::PathBuf {
    let saved = settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["rec_dir"].as_str().map(std::path::PathBuf::from));
    saved.unwrap_or_else(|| dirs::document_dir().unwrap_or_else(|| ".".into()).join("Guitar Amp"))
}

fn save_rec_dir(dir: &std::path::Path) {
    if let Some(p) = settings_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::json!({ "rec_dir": dir.to_string_lossy() }).to_string());
    }
}

struct State {
    rec_dir: std::path::PathBuf,
    engine: Option<Engine>,
    recorder: Option<Recorder>,
    rec_start: Instant,
    devs_in: Vec<String>,
    devs_out: Vec<String>,
    factory: Vec<Preset>,
    user: Vec<Preset>,
    tuner: VecDeque<f32>,
    tick: u32,
    in_disp: f32,
    out_disp: f32,
    hz: f32,
    silent_ticks: u32,
}

fn preset_names(st: &State) -> ModelRc<SharedString> {
    let names: Vec<SharedString> = st.factory.iter().chain(st.user.iter()).map(|p| SharedString::from(p.name.as_str())).collect();
    ModelRc::new(VecModel::from(names))
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let shared = Arc::new(Shared::new());
    let g = ui.global::<S>();

    let (ins, outs) = audio::device_names();
    let with_default = |v: &Vec<String>| {
        let mut l = vec![SharedString::from("Par défaut")];
        l.extend(v.iter().map(|s| SharedString::from(s.as_str())));
        ModelRc::new(VecModel::from(l))
    };
    g.set_devs_in(with_default(&ins));
    g.set_devs_out(with_default(&outs));
    g.set_v(shared.params.to_ui());

    let state = Rc::new(RefCell::new(State {
        rec_dir: load_rec_dir(),
        engine: None,
        recorder: None,
        rec_start: Instant::now(),
        devs_in: ins,
        devs_out: outs,
        factory: factory_presets(),
        user: load_user_presets(),
        tuner: VecDeque::new(),
        tick: 0,
        in_disp: 0.0,
        out_disp: 0.0,
        hz: 0.0,
        silent_ticks: 0,
    }));
    g.set_presets(preset_names(&state.borrow()));
    g.set_rec_dir(format!("Dossier : {}", state.borrow().rec_dir.display()).into());

    // Un potard bouge : on met à jour le paramètre audio, puis l'affichage.
    {
        let (sh, weak) = (shared.clone(), ui.as_weak());
        g.on_edit(move |name, v| {
            sh.params.set_by_name(&name, v);
            if let Some(ui) = weak.upgrade() {
                ui.global::<S>().set_v(sh.params.to_ui());
            }
        });
    }

    // Démarrer l'audio
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_start(move || {
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            let mut st = st.borrow_mut();
            let pick = |sel: i32, list: &Vec<String>| if sel > 0 { list.get(sel as usize - 1).cloned() } else { None };
            let (i, o) = (pick(g.get_in_sel(), &st.devs_in), pick(g.get_out_sel(), &st.devs_out));
            match audio::start(&sh, i.as_deref(), o.as_deref(), 128) {
                Ok(e) => {
                    let ms = e.buffer as f32 / e.sample_rate as f32 * 1000.0;
                    g.set_status(format!("En marche — {} Hz, buffer {} (~{:.1} ms par bloc)", e.sample_rate, e.buffer, ms).into());
                    st.engine = Some(e);
                    g.set_running(true);
                }
                Err(err) => g.set_status(format!("Erreur : {err}").into()),
            }
        });
    }

    // Arrêter l'audio
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_stop(move || {
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            let mut st = st.borrow_mut();
            if let Some(r) = st.recorder.take() {
                g.set_rec_info(format!("Enregistré : {}", r.finish()).into());
                g.set_recording(false);
            }
            st.engine = None;
            sh.in_peak.set(0.0);
            sh.out_peak.set(0.0);
            g.set_running(false);
            g.set_status("Arrêté".into());
        });
    }

    // Enregistrement
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_toggle_rec(move || {
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            let mut st = st.borrow_mut();
            if let Some(r) = st.recorder.take() {
                g.set_rec_info(format!("Enregistré : {}", r.finish()).into());
                g.set_recording(false);
            } else if let Some(sr) = st.engine.as_ref().map(|e| e.sample_rate) {
                let dir = st.rec_dir.clone();
                match audio::start_recording(&sh, sr, &dir) {
                    Ok(r) => {
                        st.rec_start = Instant::now();
                        st.recorder = Some(r);
                        g.set_recording(true);
                    }
                    Err(e) => g.set_rec_info(format!("Erreur : {e}").into()),
                }
            } else {
                g.set_rec_info("Démarre d'abord l'audio.".into());
            }
        });
    }

    // Choix du dossier d'enregistrement
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        g.on_pick_rec_dir(move || {
            let start = st.borrow().rec_dir.clone();
            let mut dlg = rfd::FileDialog::new().set_title("Dossier des enregistrements");
            if start.exists() {
                dlg = dlg.set_directory(&start);
            }
            if let Some(dir) = dlg.pick_folder() {
                save_rec_dir(&dir);
                weak.upgrade().unwrap().global::<S>().set_rec_dir(format!("Dossier : {}", dir.display()).into());
                st.borrow_mut().rec_dir = dir;
            }
        });
    }

    // Presets
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_apply_preset(move |i| {
            let st = st.borrow();
            if let Some(p) = st.factory.iter().chain(st.user.iter()).nth(i.max(0) as usize) {
                sh.params.load(&p.values);
                weak.upgrade().unwrap().global::<S>().set_v(sh.params.to_ui());
            }
        });
    }
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_save_preset(move |name| {
            let mut name = name.trim().to_string();
            if name.is_empty() {
                return;
            }
            let mut st = st.borrow_mut();
            if st.factory.iter().any(|p| p.name == name) {
                name.push_str(" (perso)");
            }
            let preset = Preset { name: name.clone(), values: sh.params.snapshot() };
            match st.user.iter_mut().find(|p| p.name == name) {
                Some(p) => *p = preset,
                None => st.user.push(preset),
            }
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            if let Err(e) = save_user_presets(&st.user) {
                g.set_status(format!("Sauvegarde impossible : {e}").into());
            }
            g.set_presets(preset_names(&st));
            g.set_preset_sel((st.factory.len() + st.user.iter().position(|p| p.name == name).unwrap_or(0)) as i32);
        });
    }
    g.on_pick_in(|_| {});
    g.on_pick_out(|_| {});

    // Rafraîchissement ~30 fois par seconde : vu-mètres, accordeur, métronome, temps d'enregistrement.
    let timer = Timer::default();
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            let mut st = st.borrow_mut();
            st.tick += 1;
            st.in_disp = sh.in_peak.get().max(st.in_disp * 0.85);
            st.out_disp = sh.out_peak.get().max(st.out_disp * 0.85);
            g.set_in_level(st.in_disp.sqrt());
            g.set_out_level(st.out_disp.sqrt());
            g.set_beat(sh.beat.load(Relaxed) as i32);

            if st.recorder.is_some() {
                let s = st.rec_start.elapsed().as_secs();
                g.set_rec_info(format!("Enregistrement… {:02}:{:02}", s / 60, s % 60).into());
            }

            while let Some(x) = sh.tuner_q.pop() {
                if st.tuner.len() == 4096 {
                    st.tuner.pop_front();
                }
                st.tuner.push_back(x);
            }
            if st.tick % 3 == 0 {
                if let (Some(sr), true) = (st.engine.as_ref().map(|e| e.sample_rate), st.tuner.len() == 4096) {
                    let buf: Vec<f32> = st.tuner.iter().copied().collect();
                    match dsp::detect_pitch(&buf, sr as f32) {
                        Some(hz) => {
                            st.silent_ticks = 0;
                            st.hz = if st.hz > 0.0 && (st.hz / hz - 1.0).abs() < 0.05 { st.hz * 0.6 + hz * 0.4 } else { hz };
                            let (note, cents) = dsp::note_info(st.hz);
                            g.set_note(note.into());
                            g.set_cents(cents);
                            g.set_hz(format!("{:.1} Hz  ({:+.0} cents)", st.hz, cents).into());
                        }
                        None => {
                            st.silent_ticks += 1;
                            if st.silent_ticks > 10 {
                                st.hz = 0.0;
                                g.set_note("--".into());
                                g.set_cents(0.0);
                                g.set_hz("".into());
                            }
                        }
                    }
                }
            }
        });
    }

    ui.run()
}
