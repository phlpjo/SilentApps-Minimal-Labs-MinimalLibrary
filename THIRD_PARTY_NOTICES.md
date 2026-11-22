# Hinweise zu Bestandteilen Dritter

Minimal Library ist proprietäre Software (siehe `LICENSE`). Sie enthält oder nutzt die folgenden
Bestandteile Dritter. Die vollständige, automatisch geprüfte Liste der Rust-Abhängigkeiten liefert
`cargo deny list` (Lizenzregeln in `deny.toml`; GPL, LGPL und AGPL sind ausgeschlossen).

## Formatreferenz für den Pioneer-Export

**pyrekordbox** – https://github.com/dylanljones/pyrekordbox – MIT-Lizenz

Die Dateiformate (ANLZ-Analysedateien, OneLibrary/Device Library Plus, rekordbox XML) wurden anhand
der Dokumentation und des Quellcodes von pyrekordbox umgesetzt. Übernommen wurden Konstanten und
Strukturangaben (Feldreihenfolge und feste Werte der ANLZ-Abschnitte, Tabellen/Spalten der
OneLibrary-Datenbank, Dateityp-Nummern, Bitaufteilung der Farbwellenform) sowie der Schlüssel der
OneLibrary-Datenbank. pyrekordbox wird außerdem nur als Testwerkzeug in der CI verwendet
(`scripts/verify_pioneer.py`) und nicht ausgeliefert.

```
MIT License

Copyright (c) 2022-2025, Dylan Jones

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Die Formatanalyse von Deep Symmetry (djl-analysis.deepsymmetry.org) wurde **nicht** verwendet: Die
Seite war aus der Entwicklungsumgebung nicht erreichbar, ihre Lizenz konnte daher nicht geprüft werden.
Nicht verwendet wurden außerdem `rbox`/`rbox-*` (GPL-3.0) und `rekordcrate`.

## Verschlüsselte Datenbank (OneLibrary)

- **SQLCipher Community Edition** (über das Rust-Paket `sqlcipher-src`) – BSD-3-Clause-artige
  Lizenz, © Zetetic LLC. Enthält **SQLite** (gemeinfrei, „blessing“) und Quellen von **libtomcrypt**
  (gemeinfrei/WTFPL).
- **OpenSSL 3** (über `openssl-src`, statisch eingebunden) – Apache License 2.0.
- **rusqlite / libsqlite3-sys** und das Bau-Paket **sqlcipher-src** – MIT.

SQLCipher-Lizenzhinweis:

```
Copyright (c) 2025, ZETETIC LLC
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:
    * Redistributions of source code must retain the above copyright
      notice, this list of conditions and the following disclaimer.
    * Redistributions in binary form must reproduce the above copyright
      notice, this list of conditions and the following disclaimer in the
      documentation and/or other materials provided with the distribution.
    * Neither the name of the ZETETIC LLC nor the
      names of its contributors may be used to endorse or promote products
      derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY ZETETIC LLC ''AS IS'' AND ANY
EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL ZETETIC LLC BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

OpenSSL: Licensed under the Apache License 2.0 – https://www.openssl.org/source/license.html

## Weitere wichtige Bestandteile

| Bestandteil | Lizenz |
|---|---|
| Tauri und Plugins | MIT oder Apache-2.0 |
| Symphonia (Audio-Dekodierung) | MPL-2.0 (unverändert) |
| Lofty (Tags) | MIT oder Apache-2.0 |
| rusty-chromaprint | MIT |
| rustfft | MIT oder Apache-2.0 |
| webpki-roots (Wurzelzertifikate) | CDLA-Permissive-2.0 |
| Space Grotesk (Schrift) | SIL Open Font License 1.1 |

## Marken

Pioneer DJ, rekordbox und OneLibrary sind Marken der AlphaTheta Corporation.
Minimal Library steht in keiner Verbindung zu AlphaTheta.
