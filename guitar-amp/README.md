# Guitar Amp

Ampli de guitare virtuel en Rust : ampli (Clean / Crunch / Lead / Metal), pédales (gate, overdrive, chorus, delay, reverb),
accordeur, métronome, enregistrement WAV et presets (jazz, country, Dire Straits, blues, rock, solo, Gojira).

- Audio : `cpal` (WASAPI sous Windows, donc la carte son de la carte mère suffit, sans ASIO)
- Interface : `slint`
- DSP : `src/dsp.rs` (Rust pur, testé avec `cargo test`)

## Lancer

```
cargo run --release
```

Branche la guitare, choisis l'entrée et la sortie, clique sur **Démarrer**. **Mets un casque** pour éviter le larsen.
Dans Windows, règle l'entrée et la sortie sur la même fréquence (48000 Hz de préférence) et désactive les « améliorations audio ».

Les enregistrements vont dans `Documents\Guitar Amp`, tes presets dans le dossier de configuration de l'utilisateur.

## Installateur Windows

Le fichier `GuitarAmp-Setup.exe` est construit par GitHub (onglet **Actions**, workflow « Installateur Windows », section *Artifacts*).
En local sur Windows : installer Rust et [Inno Setup](https://jrsoftware.org/isdl.php), puis lancer `installer\build.bat`.
