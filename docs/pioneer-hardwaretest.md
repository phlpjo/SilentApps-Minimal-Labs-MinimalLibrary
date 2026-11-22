# Hardwaretest: Pioneer DJ / AlphaTheta-Export (OneLibrary)

Der Export ist mit pyrekordbox geprüft, aber noch **nicht an echten Playern**. Diese Liste der Reihe
nach abarbeiten und Ergebnisse (Gerät, Firmware, Stick-Format FAT32/exFAT, Fotos vom Display) notieren.

## Vorbereitung
- [ ] Minimal Library: Einstellungen → „Export für Pioneer DJ / AlphaTheta-Player“ einschalten, Analyse durchlaufen lassen.
- [ ] Testplaylist mit 5–10 Titeln: MP3, AAC/M4A, WAV, AIFF, FLAC, je einer mit und ohne Cover; ein OGG zum Testen von „überspringen/umwandeln“.
- [ ] Bei einem Titel Grid von Hand korrigieren (Downbeat, ½/×2), Hot Cues A, B (mit Farbe + Kommentar), eine Schleife und einen Memory Cue setzen.
- [ ] Stick A: **leer** (frisch formatiert). Stick B: vorher mit **rekordbox** exportiert (eine Playlist mit 1–2 Titeln).

## Am Player (CDJ-3000 / CDJ-3000X / XDJ-AZ / OPUS-QUAD / OMNIS-DUO)
- [ ] Stick A wird erkannt, keine Fehlermeldung.
- [ ] Playlisten erscheinen unter „Playlist“, Reihenfolge stimmt.
- [ ] Titel lädt, Titel/Künstler/Album/Genre/BPM/Tonart richtig angezeigt.
- [ ] Artwork wird angezeigt (nur JPEG-Cover werden übernommen).
- [ ] Vorschau-Wellenform (Übersicht) und Detail-Wellenform sichtbar, farbig.
- [ ] Beatgrid: Taktstriche liegen auf den Kicks, Downbeat (1) richtig; BEAT SYNC/Quantize zwischen zwei Playern funktioniert.
- [ ] Hot Cues A–H an der richtigen Stelle, Farben und Kommentare; Schleife spielt als Loop.
- [ ] Memory Cue an der richtigen Stelle.
- [ ] Spulen in VBR-MP3s landet an der richtigen Stelle (PVBR-Tabelle ist leer – siehe offene Fragen).
- [ ] Stick B: die rekordbox-Playlist ist weiterhin da und spielt; die neuen Playlisten zusätzlich.
- [ ] Zweiter Export auf denselben Stick: nur Neues wird kopiert, keine doppelten Einträge.

## rekordbox
- [ ] rekordbox liest Stick A und Stick B ohne Fehler (Geräte-Ansicht).
- [ ] rekordbox-XML: Einstellungen → Erweitert → rekordbox xml einbinden → Titel, Tempo/Grid, Hot Cues und Playlisten kommen an (Windows- und Mac-Pfade).

## Bei Fehlern mitschicken
- `PIONEER/rekordbox/exportLibrary.db` und einen Ordner aus `PIONEER/USBANLZ/…` vom Stick
- zum Vergleich dieselben Dateien von einem rekordbox-Export desselben Titels
