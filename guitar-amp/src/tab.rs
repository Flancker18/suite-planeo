//! Lecture des fichiers Guitar Pro (.gp3 .gp4 .gp5 .gpx .gp) et mise en forme pour l'affichage défilant.
use guitarpro::{NoteType, Song};

/// Une note à afficher : position en noires depuis le début, corde (0 = la plus aiguë), case.
pub struct TabNote {
    /// Hauteur MIDI de la note (None pour une note étouffée « x », qui n'a pas de hauteur).
    pub midi: Option<i32>,
    pub pos: f32,
    pub string: i32,
    pub fret: String,
}

pub struct TabTrack {
    pub name: String,
    pub strings: usize,
    pub notes: Vec<TabNote>,
    /// (début de la mesure en noires, tempo en BPM)
    pub bars: Vec<(f32, f32)>,
    pub total: f32,
}

impl TabTrack {
    /// Numéro de la mesure qui contient la position `pos`.
    pub fn bar_index(&self, pos: f32) -> usize {
        self.bars.partition_point(|b| b.0 <= pos).saturating_sub(1)
    }
}


/// Une note jouée est bonne si elle a la bonne hauteur, ou la même à une octave près
/// (la détection de hauteur se trompe parfois d'octave sur les cordes graves).
pub fn pitch_matches(played: i32, expected: &[i32]) -> bool {
    expected.iter().any(|&e| {
        let d = played - e;
        d == 0 || d.abs() == 12
    })
}

/// Mode « attend ma note » : retient la tablature sur la prochaine note jusqu'à ce qu'elle soit jouée.
/// Pour un accord, on accepte n'importe laquelle de ses notes (la détection ne trouve qu'une note à la fois).
#[derive(Default)]
pub struct Follower {
    /// Indice de la première note du prochain groupe à jouer.
    pub next: usize,
    last: Option<i32>,
    gap: bool,
}

impl Follower {
    pub fn seek(&mut self, track: &TabTrack, pos: f32) {
        self.next = track.notes.partition_point(|n| n.pos < pos - 0.001);
        self.last = None;
        self.gap = true;
    }

    /// Hors mode attente : on oublie simplement les notes dépassées.
    pub fn skip_passed(&mut self, track: &TabTrack, pos: f32) {
        while self.next < track.notes.len() && track.notes[self.next].pos < pos - 0.001 {
            self.next += 1;
        }
    }

    /// À appeler à chaque détection : un silence ou une autre note « réarme » la répétition d'une même note.
    pub fn heard(&mut self, detected: Option<i32>) {
        if detected.is_none() || detected != self.last {
            self.gap = true;
        }
    }

    /// Appelé après avoir avancé `pos`. Retient `pos` sur la note attendue.
    /// `det` = (note détectée, nombre de détections identiques de suite).
    /// Retourne les notes attendues tant qu'on attend, ou None si la lecture peut continuer.
    pub fn step(&mut self, track: &TabTrack, pos: &mut f32, det: Option<(i32, u32)>) -> Option<Vec<i32>> {
        loop {
            let first = track.notes.get(self.next)?;
            if *pos < first.pos - 0.0005 {
                return None;
            }
            let mut end = self.next;
            while end < track.notes.len() && (track.notes[end].pos - first.pos).abs() < 0.001 {
                end += 1;
            }
            let expected: Vec<i32> = track.notes[self.next..end].iter().filter_map(|n| n.midi).collect();
            if expected.is_empty() {
                self.next = end; // groupe sans hauteur (notes étouffées) : on ne l'attend pas
                continue;
            }
            *pos = first.pos;
            match det {
                Some((m, stable)) if stable >= 2 && pitch_matches(m, &expected) && (self.last != Some(m) || self.gap) => {
                    self.last = Some(m);
                    self.gap = false;
                    self.next = end;
                }
                _ => return Some(expected),
            }
        }
    }
}

pub struct TabSong {
    pub title: String,
    pub tracks: Vec<TabTrack>,
}

/// Durée d'une figure en noires (ronde = 4, noire = 1, croche = 0,5 ...).
fn quarters(value: u16, dotted: bool, double_dotted: bool, enters: u8, times: u8) -> f32 {
    let mut q = 4.0 / value.max(1) as f32;
    if double_dotted {
        q *= 1.75;
    } else if dotted {
        q *= 1.5;
    }
    if enters > 0 && times > 0 {
        q *= times as f32 / enters as f32;
    }
    q
}

pub fn from_song(song: &Song) -> TabSong {
    // Positions des mesures (les reprises ne sont pas dépliées : lecture linéaire).
    // Tempo : celui du morceau, puis chaque changement. Une mesure sans valeur (0) garde le tempo précédent.
    let mut bars = Vec::new();
    let mut pos = 0.0f32;
    let mut tempo = song.tempo.max(30) as f32;
    for h in &song.measure_headers {
        if h.tempo > 0 {
            tempo = (h.tempo as f32).max(30.0);
        }
        bars.push((pos, tempo));
        pos += h.time_signature.numerator.max(1) as f32 * 4.0 / h.time_signature.denominator.value.max(1) as f32;
    }
    let total = pos;

    let mut tracks = Vec::new();
    for t in &song.tracks {
        if t.percussion_track || t.strings.is_empty() {
            continue;
        }
        let mut notes = Vec::new();
        for (mi, m) in t.measures.iter().enumerate() {
            let Some(&(start, _)) = bars.get(mi) else { break };
            for v in &m.voices {
                let mut at = start;
                for b in &v.beats {
                    for n in &b.notes {
                        let (fret, midi) = match n.kind {
                            NoteType::Normal => {
                                // Accordage à vide de la corde + numéro de case
                                let open = t.strings.iter().find(|s| s.0 as i32 == n.string as i32).map(|s| s.1 as i32);
                                (n.value.to_string(), open.map(|o| o + n.value as i32))
                            }
                            NoteType::Dead => ("x".to_string(), None),
                            _ => continue,
                        };
                        notes.push(TabNote { midi, pos: at, string: (n.string as i32 - 1).max(0), fret });
                    }
                    let d = &b.duration;
                    at += quarters(d.value, d.dotted, d.double_dotted, d.tuplet_enters, d.tuplet_times);
                }
            }
        }
        notes.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        tracks.push(TabTrack { name: t.name.clone(), strings: t.strings.len(), notes, bars: bars.clone(), total });
    }

    let title = match (song.artist.trim(), song.name.trim()) {
        ("", "") => "(sans titre)".to_string(),
        ("", n) => n.to_string(),
        (a, "") => a.to_string(),
        (a, n) => format!("{a} — {n}"),
    };
    TabSong { title, tracks }
}

pub fn load(path: &std::path::Path) -> Result<TabSong, String> {
    let data = std::fs::read(path).map_err(|e| format!("Lecture impossible : {e}"))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    // Un fichier abîmé ne doit pas faire planter l'appli.
    let parsed = std::panic::catch_unwind(|| {
        let mut song = Song::default();
        let r = match ext.as_str() {
            "gp3" => song.read_gp3(&data),
            "gp4" => song.read_gp4(&data),
            "gp5" => song.read_gp5(&data),
            "gpx" => song.read_gpx(&data),
            "gp" => song.read_gp(&data),
            _ => return Err("Format non reconnu (attendu : .gp3 .gp4 .gp5 .gpx .gp)".to_string()),
        };
        r.map(|_| song).map_err(|e| format!("Fichier illisible : {e}"))
    })
    .map_err(|_| "Fichier illisible (corrompu ?)".to_string())??;
    let tab = from_song(&parsed);
    if tab.tracks.is_empty() {
        return Err("Aucune piste de guitare trouvée dans ce fichier".into());
    }
    Ok(tab)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guitarpro::{Beat, Measure, MeasureHeader, Note, Track, Voice};


    fn track_with(notes: &[(f32, Option<i32>)]) -> TabTrack {
        TabTrack {
            name: "t".into(),
            strings: 6,
            notes: notes.iter().map(|&(pos, midi)| TabNote { midi, pos, string: 0, fret: "0".into() }).collect(),
            bars: vec![(0.0, 120.0)],
            total: 8.0,
        }
    }

    #[test]
    fn pitch_matching_allows_octave_errors_only() {
        assert!(pitch_matches(40, &[40]));
        assert!(pitch_matches(52, &[40]));
        assert!(pitch_matches(28, &[40]));
        assert!(!pitch_matches(41, &[40]));
        assert!(!pitch_matches(47, &[40, 43]));
        assert!(pitch_matches(43, &[40, 43]));
    }

    #[test]
    fn follower_waits_for_the_right_note() {
        let tr = track_with(&[(0.0, Some(40)), (1.0, Some(43)), (2.0, Some(45))]);
        let mut f = Follower::default();
        f.seek(&tr, 0.0);
        let mut pos = 0.0;
        // rien joué : on attend la première note, la position reste
        assert_eq!(f.step(&tr, &mut pos, None), Some(vec![40]));
        pos = 0.4;
        assert_eq!(f.step(&tr, &mut pos, None), None.or(Some(vec![40])));
        assert_eq!(pos, 0.0);
        // mauvaise note, même stable : toujours bloqué
        assert!(f.step(&tr, &mut pos, Some((41, 5))).is_some());
        // bonne note mais vue une seule fois : pas encore confirmée
        assert!(f.step(&tr, &mut pos, Some((40, 1))).is_some());
        // bonne note confirmée : on avance, le temps repart
        pos = 0.0;
        assert_eq!(f.step(&tr, &mut pos, Some((40, 2))), None);
        pos = 0.5;
        assert_eq!(f.step(&tr, &mut pos, None), None);
        // la 2e note arrive : on attend à nouveau
        pos = 1.2;
        assert_eq!(f.step(&tr, &mut pos, None), Some(vec![43]));
        assert_eq!(pos, 1.0);
    }

    #[test]
    fn repeated_note_needs_a_new_attack() {
        let tr = track_with(&[(0.0, Some(40)), (1.0, Some(40))]);
        let mut f = Follower::default();
        f.seek(&tr, 0.0);
        let mut pos = 0.0;
        assert_eq!(f.step(&tr, &mut pos, Some((40, 3))), None); // 1re note validée
        pos = 1.1;
        // la même note résonne encore : ne valide pas la 2e
        f.heard(Some(40));
        assert!(f.step(&tr, &mut pos, Some((40, 9))).is_some());
        // silence entre deux : on réarme, la nouvelle attaque valide
        f.heard(None);
        pos = 1.1;
        assert_eq!(f.step(&tr, &mut pos, Some((40, 2))), None);
    }

    #[test]
    fn chord_accepts_any_note_and_dead_notes_are_skipped() {
        let tr = track_with(&[(0.0, None), (1.0, Some(40)), (1.0, Some(47)), (1.0, Some(52)), (2.0, Some(45))]);
        let mut f = Follower::default();
        f.seek(&tr, 0.0);
        let mut pos = 0.5;
        // la note étouffée du début n'est pas attendue, on arrive à l'accord
        assert_eq!(f.step(&tr, &mut pos, None), None);
        pos = 1.0;
        assert_eq!(f.step(&tr, &mut pos, None), Some(vec![40, 47, 52]));
        assert_eq!(f.step(&tr, &mut pos, Some((47, 2))), None);
    }

    #[test]
    fn seek_and_skip() {
        let tr = track_with(&[(0.0, Some(40)), (1.0, Some(43)), (2.0, Some(45))]);
        let mut f = Follower::default();
        f.seek(&tr, 1.0);
        assert_eq!(f.next, 1);
        f.skip_passed(&tr, 2.5);
        assert_eq!(f.next, 3);
    }

    #[test]
    fn positions_and_bars_from_a_simple_song() {
        let mut song = Song::default();
        for _ in 0..2 {
            let mut h = MeasureHeader::default();
            h.tempo = 90;
            song.measure_headers.push(h);
        }
        let mut track = Track::default();
        track.strings = vec![(1, 64), (2, 59), (3, 55), (4, 50), (5, 45), (6, 40)];
        for _ in 0..2 {
            let mut voice = Voice::default();
            for i in 0..4 {
                let mut beat = Beat::default();
                let mut note = Note::default();
                note.kind = NoteType::Normal;
                note.string = 1 + (i % 6) as i8;
                note.value = 3 + i as i16;
                beat.notes.push(note);
                voice.beats.push(beat);
            }
            let mut m = Measure::default();
            m.voices.push(voice);
            track.measures.push(m);
        }
        song.tracks.push(track);

        let tab = from_song(&song);
        let t = &tab.tracks[0];
        assert_eq!(t.strings, 6);
        assert_eq!(t.notes.len(), 8);
        assert_eq!(t.notes[0].pos, 0.0);
        assert_eq!(t.notes[3].pos, 3.0);
        assert_eq!(t.notes[4].pos, 4.0);
        assert_eq!(t.notes[2].string, 2);
        assert_eq!(t.notes[2].fret, "5");
        assert_eq!(t.notes[0].midi, Some(64 + 3)); // corde 1 (mi aigu = 64) case 3 = sol
        assert_eq!(t.notes[2].midi, Some(55 + 5)); // corde 3 (sol = 55) case 5
        assert_eq!(t.bars, vec![(0.0, 90.0), (4.0, 90.0)]);
        assert_eq!(t.total, 8.0);
        assert_eq!(t.bar_index(5.5), 1);
        assert_eq!(t.bar_index(0.0), 0);
    }

    /// Petit riff de 8 mesures en croches (mi mineur pentatonique), 6 cordes.
    fn demo_song() -> Song {
        let mut song = Song::default();
        song.name = "Riff d'essai".into();
        song.artist = "Guitar Amp".into();
        song.tempo = 100;
        for i in 0..8 {
            let mut h = MeasureHeader::default();
            h.tempo = if i == 4 { 140 } else { 0 }; // changement de tempo à la mesure 5
            song.measure_headers.push(h);
        }
        let mut track = Track::default();
        track.name = "Guitare".into();
        track.strings = vec![(1, 64), (2, 59), (3, 55), (4, 50), (5, 45), (6, 40)];
        let riff: [(i8, i16); 8] = [(6, 0), (6, 3), (5, 2), (5, 0), (4, 2), (4, 0), (3, 2), (2, 3)];
        for _ in 0..8 {
            let mut voice = Voice::default();
            for (string, fret) in riff {
                let mut beat = Beat::default();
                beat.duration.value = 8;
                let mut note = Note::default();
                note.kind = NoteType::Normal;
                note.string = string;
                note.value = fret;
                beat.notes.push(note);
                voice.beats.push(beat);
            }
            let mut m = Measure::default();
            m.voices.push(voice);
            track.measures.push(m);
        }
        song.tracks.push(track);
        song
    }

    #[test]
    fn real_guitar_pro_file_roundtrip() {
        let bytes = demo_song().write_gp().expect("écriture .gp");
        let dir = std::env::temp_dir().join("guitar-amp-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("riff.gp");
        std::fs::write(&path, bytes).unwrap();
        let tab = load(&path).expect("lecture .gp");
        assert_eq!(tab.title, "Guitar Amp — Riff d'essai");
        let t = &tab.tracks[0];
        assert_eq!(t.strings, 6);
        assert_eq!(t.notes.len(), 64);
        assert_eq!(t.bars.len(), 8);
        assert_eq!(t.total, 32.0);
        assert!((t.notes[1].pos - 0.5).abs() < 1e-6, "croches : {}", t.notes[1].pos);
        let tempos: Vec<f32> = t.bars.iter().map(|b| b.1).collect();
        assert_eq!(tempos, vec![100.0, 100.0, 100.0, 100.0, 140.0, 140.0, 140.0, 140.0]);
    }

    #[test]
    fn bad_file_gives_an_error_not_a_crash() {
        let dir = std::env::temp_dir().join("guitar-amp-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("faux.gp5");
        std::fs::write(&p, b"ceci n'est pas un fichier guitar pro").unwrap();
        assert!(load(&p).is_err());
        assert!(load(&dir.join("inexistant.gp5")).is_err());
    }
}
