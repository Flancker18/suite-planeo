mod audio;
mod dsp;
mod params;
mod tab;

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
        mk("Folk acoustique", &[("inst", 2.0), ("body", 5.0), ("treble", 5.5), ("gate_on", 0.0), ("rv_on", 1.0), ("rv_size", 3.0), ("rv_mix", 2.0)]),
        mk("Classique (nylon)", &[("inst", 1.0), ("body", 5.5), ("treble", 4.5), ("gate_on", 0.0), ("rv_on", 1.0), ("rv_size", 4.0), ("rv_mix", 2.5)]),
        mk("Country jumbo", &[("inst", 3.0), ("body", 6.0), ("bass", 6.0), ("treble", 6.0), ("gate_on", 0.0), ("dl_on", 1.0), ("dl_time", 1.0), ("dl_fb", 1.0), ("dl_mix", 2.0)]),
        mk("Ukulélé", &[("inst", 4.0), ("body", 5.0), ("gate_on", 0.0), ("rv_on", 1.0), ("rv_size", 2.0), ("rv_mix", 1.5)]),
        mk("Banjo bluegrass", &[("inst", 5.0), ("body", 5.0), ("gate_on", 0.0), ("rv_on", 1.0), ("rv_size", 2.0), ("rv_mix", 1.5)]),
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

/// Choix mémorisés d'un lancement à l'autre.
#[derive(Default, Serialize, Deserialize)]
struct Settings {
    rec_dir: Option<String>,
    input: Option<String>,
    output: Option<String>,
    output2: Option<String>,
    buffer: Option<u32>,
    theme: Option<i32>,
}

fn load_settings() -> Settings {
    settings_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn update_settings(f: impl FnOnce(&mut Settings)) {
    let mut s = load_settings();
    f(&mut s);
    if let Some(p) = settings_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_string_pretty(&s).unwrap_or_default());
    }
}

/// Dossier d'enregistrement : celui choisi la dernière fois, sinon Documents\Guitar Amp.
fn load_rec_dir() -> std::path::PathBuf {
    load_settings().rec_dir.map(std::path::PathBuf::from).unwrap_or_else(|| dirs::document_dir().unwrap_or_else(|| ".".into()).join("Guitar Amp"))
}

fn save_rec_dir(dir: &std::path::Path) {
    update_settings(|s| s.rec_dir = Some(dir.to_string_lossy().into_owned()));
}

const BUFFERS: [u32; 4] = [64, 128, 256, 512];

struct State {
    follower: tab::Follower,
    tab_wait: bool,
    detected: Option<i32>,
    det_stable: u32,
    tab: Option<Rc<tab::TabSong>>,
    tab_track: usize,
    tab_pos: f32,
    tab_playing: bool,
    tab_speed: f32,
    last_tick: Instant,
    rec_dir: std::path::PathBuf,
    engine: Option<Engine>,
    recorder: Option<Recorder>,
    rec_start: Instant,
    last_rec: Option<(Arc<Vec<f32>>, u32)>,
    play_len: usize,
    devs_in: Vec<String>,
    devs_out: Vec<String>,
    factory: Vec<Preset>,
    user: Vec<Preset>,
    tuner: VecDeque<f32>,
    tick: u32,
    auto: Option<(Instant, f32)>,
    retry: u32,
    defaults: (Option<String>, Option<String>),
    in_disp: f32,
    out_disp: f32,
    hz: f32,
    silent_ticks: u32,
}

/// (Re)démarre l'audio avec les choix actuels : au lancement, à chaque changement de réglage et en cas de périphérique perdu.
/// Termine l'enregistrement en cours : fichier WAV écrit, et copie gardée en mémoire pour la lecture.
fn finish_recording(st: &mut State, g: &S) {
    if let Some(r) = st.recorder.take() {
        let rate = st.engine.as_ref().map(|e| e.sample_rate).unwrap_or(48000);
        let (path, data) = r.finish();
        g.set_recording(false);
        if data.is_empty() {
            g.set_rec_info("Enregistrement vide.".into());
        } else {
            let secs = data.len() as f32 / rate as f32;
            st.last_rec = Some((Arc::new(data), rate));
            g.set_has_recording(true);
            g.set_rec_info(format!("Enregistré ({secs:.0} s) : {path}").into());
        }
    }
}

fn start_audio(st: &mut State, sh: &Arc<Shared>, g: &S) {
    finish_recording(st, g);
    sh.play_on.store(false, Relaxed);
    g.set_playing(false);
    st.engine = None; // libère les périphériques avant de les rouvrir
    sh.stream_error.store(false, Relaxed);
    sh.in_peak.set(0.0);
    sh.out_peak.set(0.0);
    st.defaults = audio::default_names();
    let pick = |sel: i32, list: &Vec<String>| if sel > 0 { list.get(sel as usize - 1).cloned() } else { None };
    let (i, o, o2) = (pick(g.get_in_sel(), &st.devs_in), pick(g.get_out_sel(), &st.devs_out), pick(g.get_out2_sel(), &st.devs_out));
    let buffer = BUFFERS[(g.get_buffer_sel().max(0) as usize).min(BUFFERS.len() - 1)];
    match audio::start(sh, i.as_deref(), o.as_deref(), o2.as_deref(), buffer) {
        Ok(e) => {
            let ms = e.buffer as f32 / e.sample_rate as f32 * 1000.0;
            let conv = if e.input_rate != e.sample_rate { format!(" (conversion {} → {} Hz)", e.input_rate, e.sample_rate) } else { String::new() };
            let second = e.output2_name.as_ref().map(|n| format!(" + {n}")).unwrap_or_default();
            g.set_status(format!("Entrée : {} | Sortie : {}{} | {} Hz{}, buffer {} (~{:.1} ms)", e.input_name, e.output_name, second, e.sample_rate, conv, e.buffer, ms).into());
            update_settings(|s| {
                s.input = i.clone();
                s.output = o.clone();
                s.output2 = o2.clone();
                s.buffer = Some(buffer);
            });
            st.engine = Some(e);
            g.set_running(true);
        }
        Err(err) => {
            g.set_running(false);
            g.set_status(format!("Audio arrêté : {err}").into());
        }
    }
}

fn device_model(first: &str, list: &[String]) -> ModelRc<SharedString> {
    let mut l = vec![SharedString::from(first)];
    l.extend(list.iter().map(|s| SharedString::from(s.as_str())));
    ModelRc::new(VecModel::from(l))
}

/// Relit les périphériques audio. Si un câble guitare USB vient d'être branché, il est sélectionné tout seul.
fn rescan_devices(st: &mut State, g: &S) -> bool {
    let (ins, outs) = audio::device_names();
    if ins == st.devs_in && outs == st.devs_out {
        return false;
    }
    let chosen = |sel: i32, list: &Vec<String>| if sel > 0 { list.get(sel as usize - 1).cloned() } else { None };
    let (cur_in, cur_out, cur_out2) = (chosen(g.get_in_sel(), &st.devs_in), chosen(g.get_out_sel(), &st.devs_out), chosen(g.get_out2_sel(), &st.devs_out));
    let arrived = audio::guitar_input_index(&ins).filter(|&i| !st.devs_in.contains(&ins[i]));
    st.devs_in = ins;
    st.devs_out = outs;
    g.set_devs_in(device_model("Par défaut", &st.devs_in));
    g.set_devs_out(device_model("Par défaut", &st.devs_out));
    g.set_devs_out2(device_model("Aucune", &st.devs_out));
    let pos = |name: &Option<String>, list: &Vec<String>| name.as_ref().and_then(|n| list.iter().position(|x| x == n)).map(|i| i as i32 + 1).unwrap_or(0);
    g.set_in_sel(match arrived {
        Some(i) => i as i32 + 1,
        None => pos(&cur_in, &st.devs_in),
    });
    g.set_out_sel(pos(&cur_out, &st.devs_out));
    g.set_out2_sel(pos(&cur_out2, &st.devs_out));
    true
}

fn preset_names(st: &State) -> ModelRc<SharedString> {
    let names: Vec<SharedString> = st.factory.iter().chain(st.user.iter()).map(|p| SharedString::from(p.name.as_str())).collect();
    ModelRc::new(VecModel::from(names))
}


const TAB_BEFORE: f32 = 1.5; // noires visibles avant la tête de lecture
const TAB_SPAN: f32 = 12.0; // noires visibles au total

/// Replace le suivi de notes à la position courante (après un saut, un changement de piste...).
fn sync_follower(st: &mut State) {
    if let Some(song) = st.tab.clone() {
        st.follower.seek(&song.tracks[st.tab_track], st.tab_pos);
    }
}

fn show_track(st: &State, g: &S) {
    if let Some(song) = &st.tab {
        g.set_tab_strings(song.tracks[st.tab_track].strings as i32);
    }
}


fn open_tab(path: &std::path::Path, st: &Rc<RefCell<State>>, g: &S) {
    match tab::load(path) {
        Ok(song) => {
            let mut st = st.borrow_mut();
            let names: Vec<SharedString> = song.tracks.iter().map(|t| SharedString::from(t.name.as_str())).collect();
            g.set_tab_tracks(ModelRc::new(VecModel::from(names)));
            g.set_tab_title(song.title.as_str().into());
            g.set_tab_track_sel(0);
            g.set_tab_playing(false);
            g.set_tab_shown(true);
            st.tab = Some(Rc::new(song));
            st.tab_track = 0;
            st.tab_pos = 0.0;
            st.tab_playing = false;
            sync_follower(&mut st);
            show_track(&st, g);
        }
        Err(e) => g.set_status(format!("Tablature : {e}").into()),
    }
}

/// Fait avancer la tablature et prépare ce qui est affiché.
fn update_tab(st: &mut State, g: &S, dt: f32) {
    let Some(song) = st.tab.clone() else { return };
    let track = &song.tracks[st.tab_track];
    let mut waiting: Option<Vec<i32>> = None;
    if st.tab_playing {
        let tempo = track.bars[track.bar_index(st.tab_pos)].1;
        st.tab_pos += dt * tempo * st.tab_speed / 100.0 / 60.0;
        if st.tab_pos >= track.total {
            st.tab_pos = track.total;
            st.tab_playing = false;
            g.set_tab_playing(false);
        }
        if st.tab_wait && st.engine.is_some() {
            // Mode « attend ma note » : la position reste bloquée tant que la note n'est pas jouée.
            let det = st.detected.map(|m| (m, st.det_stable));
            waiting = st.follower.step(track, &mut st.tab_pos, det);
        } else {
            st.follower.skip_passed(track, st.tab_pos);
        }
    }
    let text = if !st.tab_wait {
        String::new()
    } else if st.engine.is_none() {
        "Démarre l'audio pour jouer avec la tablature".to_string()
    } else if !st.tab_playing {
        "Appuie sur Lecture, puis joue les notes".to_string()
    } else if let Some(exp) = waiting {
        let names: Vec<String> = exp.iter().map(|&m| dsp::midi_name(m)).collect();
        format!("À jouer : {}", names.join(" / "))
    } else {
        String::new()
    };
    g.set_tab_wait_text(text.into());
    let pos = st.tab_pos;
    let lo = pos - TAB_BEFORE;
    let hi = lo + TAB_SPAN;
    let a = track.notes.partition_point(|n| n.pos < lo - 0.01);
    let b = track.notes.partition_point(|n| n.pos <= hi);
    let items: Vec<TabItem> = track.notes[a..b]
        .iter()
        .map(|n| TabItem {
            x: (n.pos - lo) / TAB_SPAN,
            string: n.string,
            fret: n.fret.as_str().into(),
            state: if n.pos + 0.3 < pos { 0 } else if n.pos <= pos { 1 } else { 2 },
        })
        .collect();
    g.set_tab_notes(ModelRc::new(VecModel::from(items)));
    let bars: Vec<f32> = track.bars.iter().map(|b| (b.0 - lo) / TAB_SPAN).filter(|x| (-0.05..=1.05).contains(x)).collect();
    g.set_tab_bars(ModelRc::new(VecModel::from(bars)));
    g.set_tab_measure(format!("Mesure {} / {}", track.bar_index(pos) + 1, track.bars.len()).into());
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let shared = Arc::new(Shared::new());
    let g = ui.global::<S>();

    let (ins, outs) = audio::device_names();
    let saved = load_settings();
    let find = |name: &Option<String>, list: &Vec<String>| name.as_ref().and_then(|n| list.iter().position(|x| x == n)).map(|i| i as i32 + 1);
    g.set_devs_in(device_model("Par défaut", &ins));
    g.set_devs_out(device_model("Par défaut", &outs));
    g.set_devs_out2(device_model("Aucune", &outs));
    // Entrée : celle choisie la dernière fois, sinon un câble guitare / interface USB repéré par son nom
    g.set_in_sel(find(&saved.input, &ins).or_else(|| audio::guitar_input_index(&ins).map(|i| i as i32 + 1)).unwrap_or(0));
    g.set_out_sel(find(&saved.output, &outs).unwrap_or(0));
    g.set_out2_sel(find(&saved.output2, &outs).unwrap_or(0));
    g.set_theme(saved.theme.unwrap_or(0));
    g.set_buffer_sel(BUFFERS.iter().position(|&b| Some(b) == saved.buffer).unwrap_or(1) as i32);
    g.set_v(shared.params.to_ui());

    let state = Rc::new(RefCell::new(State {
        follower: tab::Follower::default(),
        tab_wait: false,
        detected: None,
        det_stable: 0,
        tab: None,
        tab_track: 0,
        tab_pos: 0.0,
        tab_playing: false,
        tab_speed: 100.0,
        last_tick: Instant::now(),
        rec_dir: load_rec_dir(),
        engine: None,
        recorder: None,
        rec_start: Instant::now(),
        last_rec: None,
        play_len: 0,
        devs_in: ins,
        devs_out: outs,
        factory: factory_presets(),
        user: load_user_presets(),
        tuner: VecDeque::new(),
        tick: 0,
        auto: None,
        retry: 0,
        defaults: (None, None),
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

    // Thème de l'interface : mémorisé
    {
        let weak = ui.as_weak();
        g.on_theme_changed(move || {
            let t = weak.upgrade().unwrap().global::<S>().get_theme();
            update_settings(|s| s.theme = Some(t));
        });
    }

    // Changement d'un réglage audio (entrée, sortie, 2e sortie, buffer) : appliqué tout de suite, sans rien arrêter à la main
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_settings_changed(move || {
            start_audio(&mut st.borrow_mut(), &sh, &weak.upgrade().unwrap().global::<S>());
        });
    }

    // Enregistrement
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_toggle_rec(move || {
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            let mut st = st.borrow_mut();
            if st.recorder.is_some() {
                finish_recording(&mut st, &g);
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

    // Actualiser la liste des périphériques (bouton) et détection automatique toutes les 2 s à l'arrêt
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        g.on_refresh_devices(move || {
            rescan_devices(&mut st.borrow_mut(), &weak.upgrade().unwrap().global::<S>());
        });
    }
    let rescan_timer = Timer::default();
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        rescan_timer.start(TimerMode::Repeated, Duration::from_secs(2), move || {
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            let mut st = st.borrow_mut();
            st.retry += 1;
            let changed = rescan_devices(&mut st, &g);
            if !g.get_running() {
                // Pas d'audio : on réessaie quand la liste change (câble branché), puis toutes les 6 s
                if changed || st.retry % 3 == 0 {
                    start_audio(&mut st, &sh, &g);
                }
            } else {
                // Périphérique perdu, ou Windows a changé le périphérique par défaut (casque branché...)
                let (di, dout) = audio::default_names();
                let default_moved = (g.get_in_sel() == 0 && di != st.defaults.0) || (g.get_out_sel() == 0 && dout != st.defaults.1);
                if sh.stream_error.swap(false, Relaxed) || default_moved || (changed && st.engine.is_none()) {
                    start_audio(&mut st, &sh, &g);
                }
            }
        });
    }

    // Lecture de l'enregistrement, arrêt, boucle
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_play_recording(move || {
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            let mut st = st.borrow_mut();
            if st.recorder.is_some() {
                finish_recording(&mut st, &g);
            }
            let Some((buf, rate)) = st.last_rec.clone() else {
                g.set_rec_info("Rien à écouter : enregistre d'abord.".into());
                return;
            };
            let Some(sr) = st.engine.as_ref().map(|e| e.sample_rate) else {
                g.set_rec_info("L'audio n'est pas démarré.".into());
                return;
            };
            let data = if rate == sr { buf } else { Arc::new(audio::resample(&buf, rate, sr)) };
            st.play_len = data.len();
            sh.play_loop.store(g.get_loop_on(), Relaxed);
            *sh.play_buf.lock().unwrap() = Some(data);
            sh.play_gen.fetch_add(1, Relaxed);
            sh.play_on.store(true, Relaxed);
            g.set_playing(true);
        });
    }
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_stop_all(move || {
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            let mut st = st.borrow_mut();
            if st.recorder.is_some() {
                finish_recording(&mut st, &g);
            }
            if sh.play_on.swap(false, Relaxed) {
                g.set_rec_info("Lecture arrêtée.".into());
            }
            g.set_playing(false);
        });
    }
    {
        let (sh, weak) = (shared.clone(), ui.as_weak());
        g.on_toggle_loop(move || {
            let g = weak.upgrade().unwrap();
            let g = g.global::<S>();
            let v = !g.get_loop_on();
            g.set_loop_on(v);
            sh.play_loop.store(v, Relaxed);
        });
    }

    // Auto-niveau : mesure la crête de ta guitare pendant 3 s et règle l'entrée en conséquence
    {
        let (sh, st, weak) = (shared.clone(), state.clone(), ui.as_weak());
        g.on_auto_level(move || {
            let ui = weak.upgrade().unwrap();
            let g = ui.global::<S>();
            let mut st = st.borrow_mut();
            if st.engine.is_none() {
                g.set_status("Auto-niveau : l'audio n'est pas démarré (vérifie Paramètres).".into());
                return;
            }
            sh.in_hold.set(0.0);
            st.auto = Some((Instant::now(), 0.0));
            g.set_status("Auto-niveau : joue des accords FORT pendant 3 secondes…".into());
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

    // Tablature : ouvrir un fichier Guitar Pro (bouton, ou fichier passé en argument au lancement)
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        g.on_tab_load(move || {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Ouvrir une tablature Guitar Pro")
                .add_filter("Guitar Pro", &["gp3", "gp4", "gp5", "gpx", "gp"])
                .pick_file()
            else {
                return;
            };
            open_tab(&path, &st, &weak.upgrade().unwrap().global::<S>());
        });
    }
    if let Some(arg) = std::env::args().nth(1).filter(|a| !a.is_empty()) {
        open_tab(std::path::Path::new(&arg), &state, &g);
    }
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        g.on_tab_close(move || {
            let mut st = st.borrow_mut();
            st.tab = None;
            st.tab_playing = false;
            let g = weak.upgrade().unwrap();
            let g = g.global::<S>();
            g.set_tab_playing(false);
            g.set_tab_shown(false);
        });
    }
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        g.on_tab_toggle_play(move || {
            let mut st = st.borrow_mut();
            let Some(song) = st.tab.clone() else { return };
            if st.tab_pos >= song.tracks[st.tab_track].total {
                st.tab_pos = 0.0;
            }
            st.tab_playing = !st.tab_playing;
            st.last_tick = Instant::now();
            sync_follower(&mut st);
            weak.upgrade().unwrap().global::<S>().set_tab_playing(st.tab_playing);
        });
    }
    {
        let st = state.clone();
        g.on_tab_restart(move || {
            let mut st = st.borrow_mut();
            st.tab_pos = 0.0;
            sync_follower(&mut st);
        });
    }
    {
        let st = state.clone();
        g.on_tab_step(move |d| {
            let mut st = st.borrow_mut();
            let Some(song) = st.tab.clone() else { return };
            let t = &song.tracks[st.tab_track];
            let i = (t.bar_index(st.tab_pos) as i32 + d).clamp(0, t.bars.len() as i32 - 1) as usize;
            st.tab_pos = t.bars[i].0;
            sync_follower(&mut st);
        });
    }
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        g.on_tab_pick_track(move |i| {
            let mut st = st.borrow_mut();
            let n = st.tab.as_ref().map(|t| t.tracks.len()).unwrap_or(0);
            if n > 0 {
                st.tab_track = (i.max(0) as usize).min(n - 1);
                sync_follower(&mut st);
                show_track(&st, &weak.upgrade().unwrap().global::<S>());
            }
        });
    }
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        g.on_tab_toggle_wait(move || {
            let mut st = st.borrow_mut();
            st.tab_wait = !st.tab_wait;
            sync_follower(&mut st);
            weak.upgrade().unwrap().global::<S>().set_tab_wait(st.tab_wait);
        });
    }
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        g.on_tab_set_speed(move |v| {
            let v = v.clamp(20.0, 150.0);
            st.borrow_mut().tab_speed = v;
            weak.upgrade().unwrap().global::<S>().set_tab_speed(v);
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
            let now = Instant::now();
            let dt = (now - st.last_tick).as_secs_f32().min(0.2);
            st.last_tick = now;
            // Crêtes depuis le dernier rafraîchissement (rien n'est manqué entre deux images)
            let (ih, oh) = (sh.in_hold.get(), sh.out_hold.get());
            sh.in_hold.set(0.0);
            sh.out_hold.set(0.0);
            st.in_disp = ih.max(st.in_disp * 0.85);
            st.out_disp = oh.max(st.out_disp * 0.85);
            if let Some((t0, peak)) = st.auto {
                let peak = peak.max(ih);
                if t0.elapsed().as_secs_f32() >= 3.0 {
                    st.auto = None;
                    if peak < 0.001 {
                        g.set_status("Auto-niveau : aucun signal. Vérifie le câble, l'entrée choisie dans Paramètres et le niveau du micro dans Windows.".into());
                    } else {
                        let db = 20.0 * (0.4 / peak).log10();
                        sh.params.input.set(dsp::input_knob_for_db(db));
                        g.set_v(sh.params.to_ui());
                        let warn = if db > 36.0 { " — signal très faible : monte aussi le niveau du micro dans Windows" } else { "" };
                        g.set_status(format!("Auto-niveau : signal capté à {:.1} % → entrée réglée à {:+.0} dB{}", peak * 100.0, db.clamp(-12.0, 36.0), warn).into());
                    }
                } else {
                    st.auto = Some((t0, peak));
                }
            }
            g.set_in_level(st.in_disp.sqrt());
            g.set_out_level(st.out_disp.sqrt());
            g.set_beat(sh.beat.load(Relaxed) as i32);

            if g.get_playing() {
                if !sh.play_on.load(Relaxed) {
                    g.set_playing(false);
                    g.set_rec_info("Lecture terminée.".into());
                } else if let Some(sr) = st.engine.as_ref().map(|e| e.sample_rate) {
                    let pos = sh.play_pos.load(Relaxed) as usize;
                    let (a, b) = (pos / sr as usize, st.play_len / sr as usize);
                    let mode = if g.get_loop_on() { " (en boucle)" } else { "" };
                    g.set_rec_info(format!("Lecture… {:02}:{:02} / {:02}:{:02}{}", a / 60, a % 60, b / 60, b % 60, mode).into());
                }
            }
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
            let tuner_on = sh.params.tuner_on.get() > 0.5;
            if !tuner_on && !st.tab_wait {
                st.detected = None;
                st.det_stable = 0;
                g.set_note("--".into());
                g.set_cents(0.0);
                g.set_hz("".into());
            }
            if st.tick % 2 == 0 && (tuner_on || st.tab_wait) {
                if let (Some(sr), true) = (st.engine.as_ref().map(|e| e.sample_rate), st.tuner.len() == 4096) {
                    let buf: Vec<f32> = st.tuner.iter().copied().collect();
                    match dsp::detect_pitch(&buf, sr as f32) {
                        Some(hz) => {
                            st.silent_ticks = 0;
                            let m = (69.0 + 12.0 * (hz / 440.0).log2()).round() as i32;
                            if st.detected == Some(m) {
                                st.det_stable += 1;
                            } else {
                                st.detected = Some(m);
                                st.det_stable = 1;
                            }
                            let d = st.detected;
                            st.follower.heard(d);
                            st.hz = if st.hz > 0.0 && (st.hz / hz - 1.0).abs() < 0.05 { st.hz * 0.6 + hz * 0.4 } else { hz };
                            let (note, cents) = dsp::note_info(st.hz);
                            g.set_note(note.into());
                            g.set_cents(cents);
                            g.set_hz(format!("{:.1} Hz  ({:+.0} cents)", st.hz, cents).into());
                        }
                        None => {
                            st.detected = None;
                            st.det_stable = 0;
                            st.follower.heard(None);
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
            if st.engine.is_none() {
                st.detected = None;
                st.det_stable = 0;
            }
            update_tab(&mut st, &g, dt);
        });
    }

    // L'audio démarre tout seul
    start_audio(&mut state.borrow_mut(), &shared, &g);

    ui.run()
}
