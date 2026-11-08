# Guitar Amp

Ampli de guitare virtuel en Rust : ampli (Clean / Crunch / Lead / Metal), pédales (gate, overdrive, chorus, delay, reverb),
accordeur, métronome, enregistrement WAV et presets (jazz, country, Dire Straits, blues, rock, solo, Gojira).

- Audio : `cpal` (WASAPI sous Windows, donc la carte son de la carte mère suffit, sans ASIO)
- Interface : `slint`
- DSP : `src/dsp.rs` (Rust pur, testé avec `cargo test`)

## Tablatures

Bouton **« Ouvrir une tablature… »** (en bas) : fichiers Guitar Pro `.gp3 .gp4 .gp5 .gpx .gp`. La tablature défile au tempo du morceau,
avec choix de la piste, pause, saut de mesure et vitesse de 20 à 150 %. Sans tablature, une façade d'ampli décorative s'affiche.
Les signes de reprise ne sont pas dépliés (lecture linéaire).

## Lancer

```
cargo run --release
```

Branche la guitare, choisis l'entrée et la sortie, clique sur **Démarrer**. **Mets un casque** pour éviter le larsen.
Dans Windows, règle l'entrée et la sortie sur la même fréquence (48000 Hz de préférence) et désactive les « améliorations audio ».

Les enregistrements vont par défaut dans `Documents\Guitar Amp`; le bouton « Choisir le dossier… » permet d'en changer (le choix est mémorisé), tes presets dans le dossier de configuration de l'utilisateur.

## Installateur Windows

Le fichier `GuitarAmp-Setup.exe` est construit par GitHub (onglet **Actions**, workflow « Installateur Windows », section *Artifacts*).
En local sur Windows : installer Rust et [Inno Setup](https://jrsoftware.org/isdl.php), puis lancer `installer\build.bat`.
