# Minimal Library

Musikbibliothek für Windows und macOS im SilentApps-Minimal-Stil – sortieren, taggen, Duplikate finden, Playlisten für Minimal Player.

## Funktionen

| Bereich | Was es kann |
|---|---|
| **Bibliothek** | Musikordner scannen (MP3, FLAC, M4A/AAC, OGG, Opus, WAV, AIFF), Suche, Filter (nicht analysiert, fehlende Tags, ohne Cover), Sortierung nach Titel/Künstler/Album/Genre/BPM/Tonart/Dauer, Dateien per Drag & Drop hinzufügen |
| **Ordner** | Ordneransicht wie im Explorer: Baum + Unterordner-Kacheln + Titelliste mit Wellenform und Tag-Editor. Titel per Drag & Drop in Ordner verschieben (Strg = kopieren), Ordner anlegen, umbenennen, verschieben, in den Papierkorb. Dateien umbenennen mit F2. Im Explorer hinzugefügte Dateien werden beim Öffnen des Ordners automatisch aufgenommen |
| **Verlauf** | Jede Änderung ist rückgängig machbar: Tags, Cover, Verschieben, Umbenennen, Ordner, Papierkorb (Titel kommen zurück in ihre Playlisten). Strg+Z = letzte Änderung |
| **Überwachung** | Musikordner werden laufend überwacht – im Explorer hinzugefügte, verschobene, umbenannte oder gelöschte Dateien werden sofort übernommen (beim Verschieben bleiben Analyse und Playlisten erhalten) |
| **Spalten** | Rechtsklick auf den Listenkopf: Spalten wählen (Nr., Künstler, Albumkünstler, Jahr, Bitrate, Format, kHz, Größe, Hinzugefügt, Kommentar, Dateiname, Pfad …). Breite am Spaltenrand ziehen, Doppelklick = automatisch. Pro Ansicht gespeichert |
| **Am Klang erkennen** | AcoustID: Titel ohne oder mit falschen Tags über den Audio-Fingerabdruck erkennen, Felder einzeln übernehmen (kostenloser Schlüssel nötig) |
| **Ripper** | Links von YouTube, YouTube Music, SoundCloud, Bandcamp u. v. m. sowie Spotify (Titel, Alben, Playlisten; Audio über die YouTube-Suche). Zeigt vorher, was schon in der Bibliothek ist. Lädt als MP3 320 oder im Original, mit sauberen Tags und Cover. Werkzeuge (yt-dlp, ffmpeg) werden auf Wunsch einmalig geladen |
| **Heimnetz** | Wiedergabe auf Sonos (Gruppen als ein Eintrag) und DLNA/UPnP-Geräten: Auswahl unten rechts im Player. Die App stellt die Datei über einen kleinen HTTP-Server im Heimnetz bereit |
| **Wellenform** | Wird einmal pro Titel berechnet und gespeichert. Klick in die Wellenform startet sofort an dieser Stelle (in der Liste, bei Duplikaten und im Player) |
| **Tags** | Einzel- und Mehrfachbearbeitung, Cover setzen/entfernen, Dateiname → Tags per Muster, Tags → Dateiname per Muster (Checkbox „Beim Speichern umbenennen“), Online-Suche (MusicBrainz + Cover Art Archive, nur auf Klick) mit Auswahl pro Feld |
| **Analyse** | BPM, Tonart (mit Camelot-Notation), Audio-Fingerabdruck (Chromaprint), parallel im Hintergrund, abbrechbar |
| **Duplikate** | Drei Arten kombinierbar: Metadaten, Dateiname, Audio-Inhalt. Vorschlag „beste Version behalten“ (verlustfrei > Bitrate > Tags), Rest in den Papierkorb, Playlisten zeigen danach auf die behaltene Datei |
| **Sortieren** | Muster wie `{albumartist}/{album}/{disc-}{track} - {title}`, Vorschau, verschieben oder kopieren, leere Ordner werden aufgeräumt, jeder Vorgang rückgängig machbar |
| **Playlisten** | Normale (Drag & Drop, umsortieren) und smarte (Regeln: Genre, BPM-Bereich, Tonart, Jahr, zuletzt hinzugefügt …). Import von M3U/M3U8 |
| **Minimal Player** | Playlist-Ordner: alle Playlisten werden automatisch als `.m3u8` mit relativen Pfaden aktuell gehalten |
| **Export** | Playlisten samt Original-Dateien + M3U8 kopieren (nummeriert / flach / gemeinsamer Musikordner), erneuter Export kopiert nur Fehlendes |

Hell/Dunkel: Schalter oben links oder Einstellungen → Darstellung (wie System / Dunkel / Hell).

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

**Auto-Update:** Die App prüft beim Start und alle 6 Stunden die GitHub-Releases (signiert, `latest.json`). Jeder Push auf `main` erzeugt ein Release `v1.0.<Lauf>`, sobald das Secret `TAURI_SIGNING_PRIVATE_KEY` eingetragen ist.

**Builds über GitHub Actions:** jeder Push auf `main` erzeugt unter *Actions → Artefakte*
- Windows: Installer (`.exe` und `.msi`) und die portable `minimal-library.exe`
- macOS: `.dmg` (Universal, Apple Silicon + Intel). Nicht von Apple beglaubigt: beim ersten Start Rechtsklick → Öffnen bzw. Systemeinstellungen → Datenschutz & Sicherheit → „Trotzdem öffnen“.

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

## Pioneer-Export

Optional: **Einstellungen → Export für Pioneer DJ / AlphaTheta-Player (OneLibrary) → Einschalten.**
Erst danach gibt es die zusätzliche Analyse und den Export; alle anderen Funktionen bleiben unverändert.

**Funktionen**
- Analyse: konstantes Beatgrid (Tempo auf 0,01 BPM, erster Downbeat), Wellenform im Raster der Player (150 Werte/s, Bass/Mitten/Höhen); läuft im normalen Hintergrund-Analysevorgang, bereits analysierte Titel werden nicht neu berechnet.
- Inspektor: Detail-Wellenform mit Beatgrid-Linien, Grid korrigieren (Downbeat setzen, ½/×2 Tempo, ±10 ms verschieben, Tempo eintippen), Hot Cues A–H mit Farbe und Kommentar, Memory Cues; alles mit Strg+Z rückgängig.
- Export (Rechtsklick auf Playlist → „Für Pioneer-Player exportieren …“): Vorschau (Titel, Größe, fehlende Analysen, nicht abspielbare Formate), Audio nach `/Contents`, Analysedateien `ANLZ0000.DAT/.EXT`, verschlüsselte OneLibrary-Datenbank `PIONEER/rekordbox/exportLibrary.db`, Playlisten in Reihenfolge, Cues, Cover. Erneuter Export kopiert nur Fehlendes; Einträge von rekordbox oder anderen Programmen bleiben unangetastet.
- Nicht abspielbare Formate (z. B. OGG, OPUS): überspringen oder mit ffmpeg (aus dem Ripper) nach MP3 320 kbit/s umwandeln.
- rekordbox-XML als Rückfallebene (In rekordbox unter Einstellungen → Erweitert → rekordbox xml einbinden).

**Geräte**: OneLibrary-fähige Player, z. B. CDJ-3000, CDJ-3000X, XDJ-AZ, OPUS-QUAD, OMNIS-DUO.

**Einschränkungen**
- Nicht an echter Hardware getestet – Checkliste: `docs/pioneer-hardwaretest.md`.
- Kein klassisches Geräteformat (`export.pdb`) für ältere Player.
- Nur konstantes Beatgrid; Tempowechsel innerhalb eines Titels werden nicht erkannt.
- Keine 3-Band-Wellenform (`.2EX`), keine Songstruktur/Phrasen (PSSI), keine erweiterte Beatgrid-Tabelle (PQT2) – Format nicht ausreichend dokumentiert.
- Cover nur aus JPEG-Bildern im Titel.

**Prüfung**: Die CI baut einen Test-Export (`core/examples/pioneer_selftest.rs`) und liest ihn mit pyrekordbox (MIT, nur Testwerkzeug) zurück: `scripts/verify_pioneer.py`.

**Lizenzen**: `cargo deny check licenses` und ein npm-Lizenzcheck laufen in jedem Build; GPL, LGPL und AGPL sind ausgeschlossen (`deny.toml`). Hinweise zu Drittbestandteilen: `THIRD_PARTY_NOTICES.md`.

Pioneer DJ, rekordbox und OneLibrary sind Marken der AlphaTheta Corporation. Minimal Library steht in keiner Verbindung zu AlphaTheta.

