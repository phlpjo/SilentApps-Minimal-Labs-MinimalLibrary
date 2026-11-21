# Minimal Library

Musikbibliothek für Windows im SilentApps-Minimal-Stil – sortieren, taggen, Duplikate finden, Playlisten für Minimal Player.

## Funktionen

| Bereich | Was es kann |
|---|---|
| **Bibliothek** | Musikordner scannen (MP3, FLAC, M4A/AAC, OGG, Opus, WAV, AIFF), Suche, Filter (nicht analysiert, fehlende Tags, ohne Cover), Sortierung nach Titel/Künstler/Album/Genre/BPM/Tonart/Dauer, Dateien per Drag & Drop hinzufügen |
| **Ordner** | Ordneransicht wie im Explorer: Baum + Unterordner-Kacheln + Titelliste mit Wellenform und Tag-Editor. Titel per Drag & Drop in Ordner verschieben (Strg = kopieren), Ordner anlegen, umbenennen, verschieben, in den Papierkorb. Dateien umbenennen mit F2. Im Explorer hinzugefügte Dateien werden beim Öffnen des Ordners automatisch aufgenommen |
| **Wellenform** | Wird einmal pro Titel berechnet und gespeichert. Klick in die Wellenform startet sofort an dieser Stelle (in der Liste, bei Duplikaten und im Player) |
| **Tags** | Einzel- und Mehrfachbearbeitung, Cover setzen/entfernen, Dateiname → Tags per Muster, Online-Suche (MusicBrainz + Cover Art Archive, nur auf Klick) |
| **Analyse** | BPM, Tonart (mit Camelot-Notation), Audio-Fingerabdruck (Chromaprint), parallel im Hintergrund, abbrechbar |
| **Duplikate** | Drei Arten kombinierbar: Metadaten, Dateiname, Audio-Inhalt. Vorschlag „beste Version behalten“ (verlustfrei > Bitrate > Tags), Rest in den Papierkorb, Playlisten zeigen danach auf die behaltene Datei |
| **Sortieren** | Muster wie `{albumartist}/{album}/{disc-}{track} - {title}`, Vorschau, verschieben oder kopieren, leere Ordner werden aufgeräumt, jeder Vorgang rückgängig machbar |
| **Playlisten** | Normale (Drag & Drop, umsortieren) und smarte (Regeln: Genre, BPM-Bereich, Tonart, Jahr, zuletzt hinzugefügt …). Import von M3U/M3U8 |
| **Minimal Player** | Playlist-Ordner: alle Playlisten werden automatisch als `.m3u8` mit relativen Pfaden aktuell gehalten |
| **Export** | Playlisten samt Original-Dateien + M3U8 kopieren (nummeriert / flach / gemeinsamer Musikordner), erneuter Export kopiert nur Fehlendes |

Riskante Aktionen (Papierkorb, Löschen, Rückgängig) werden **gedrückt gehalten** statt per Rückfrage bestätigt.

## Aufbau

```
core/         Rust-Kern (ohne GUI, testbar): db, scanner, tags, analysis, duplicates, organize, playlists, export, musicbrainz
src-tauri/    Tauri-2-App: Befehle, Fortschritts-Events, Asset-Freigabe für die Wiedergabe
src/          Oberfläche (TypeScript, ohne Framework): Liste mit virtuellem Scrollen, Tag-Editor, Ansichten
store/        Logo (Ring + Bücher-Motiv), Icon-Quellen
```

Daten liegen in `%APPDATA%\de.philipdecker.minimallibrary\library.db` (SQLite). Die Musikdateien selbst werden nur beim Speichern von Tags, beim Sortieren und beim Papierkorb verändert.

## Bauen

**Windows-Build über GitHub Actions:** jeder Push auf `main` erzeugt unter *Actions → Artefakte* den Installer (`.exe` und `.msi`) und die portable `minimal-library.exe`.

**Lokal unter Windows:**

```
npm ci
npx tauri dev      # Entwicklung
npx tauri build    # Installer in target/release/bundle/
```

Voraussetzungen: Rust (stable), Node 22, WebView2 (bei Windows 10/11 vorhanden).

**Tests:** `cargo test -p musiccore` · Ende-zu-Ende: `cargo run -p musiccore --release --example selftest -- <musikordner> <arbeitsordner>`

## Hinweise

- Wiedergabe läuft über die WebView (Chromium): MP3, FLAC, AAC/M4A, OGG, Opus, WAV. AIFF und ALAC werden analysiert und getaggt, aber nicht abgespielt.
- Opus-Dateien bekommen keine Wellenform (kein Opus-Dekoder im Rust-Kern) – Abspielen und Taggen funktioniert.
- Online-Funktionen nur auf ausdrücklichen Klick, sonst arbeitet die App komplett offline.
