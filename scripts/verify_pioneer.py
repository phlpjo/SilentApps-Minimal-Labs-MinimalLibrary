#!/usr/bin/env python3
"""Unabhängige Prüfung des Pioneer-Exports mit pyrekordbox (MIT) – nur Testwerkzeug, wird nicht ausgeliefert.

Aufruf: python scripts/verify_pioneer.py <arbeitsordner von pioneer_selftest>
"""
import json, sys
from pathlib import Path

from pyrekordbox import AnlzFile, RekordboxXml
from pyrekordbox.onelibrary import OneLibrary

work = Path(sys.argv[1])
stick = work / "stick"
exp = json.loads((work / "expected.json").read_text(encoding="utf-8"))
fails = []

def check(cond, msg):
    print(("ok   " if cond else "FEHL ") + msg)
    if not cond:
        fails.append(msg)

db = OneLibrary(stick / "PIONEER/rekordbox/exportLibrary.db")  # Schlüssel kennt pyrekordbox selbst
contents = {c.title: c for c in db.get_content()}
check(len(contents) == len(exp["tracks"]), f"{len(contents)} Titel in der Datenbank (erwartet {len(exp['tracks'])})")

for t in exp["tracks"]:
    c = contents.get(t["title"])
    check(c is not None, f"Titel „{t['title']}“ vorhanden")
    if c is None:
        continue
    check((stick / c.path.lstrip("/")).exists(), f"Audiodatei {c.path} vorhanden")
    if t["bpm"]:
        check(c.bpmx100 == round(t["bpm"] * 100), f"BPM {c.bpmx100/100} = {t['bpm']}")
    dat = AnlzFile.parse_file(stick / c.analysisDataFilePath.lstrip("/"))
    ext = AnlzFile.parse_file(stick / c.analysisDataFilePath.lstrip("/").replace(".DAT", ".EXT"))
    check(dat.get("path") == c.path, "ANLZ-Pfad passt zur Datenbank")
    if t["bpm"]:
        beats, bpms, times = dat.get("beat_grid")
        down = [tm for b, tm in zip(beats, times) if b == 1]
        check(abs(bpms[0] - t["bpm"]) < 0.005, f"Beatgrid-Tempo {bpms[0]} = {t['bpm']}")
        check(abs(down[0] * 1000 - t["downbeat_ms"]) <= 1.0, f"erster Downbeat {down[0]*1000:.0f} ms = {t['downbeat_ms']:.0f} ms")
    for tag in ("PWAV", "PWV2", "PQTZ", "PCOB"):
        check(tag in dat, f"DAT enthält {tag}")
    for tag in ("PWV3", "PWV4", "PWV5", "PCO2"):
        check(tag in ext, f"EXT enthält {tag}")
    if t["cues"]:
        db_cues = sorted((q.kind, q.inUsec // 1000) for q in db.get_cue(content_id=c.content_id))
        want = sorted((q["kind"], round(q["start_ms"])) for q in t["cues"])
        check(db_cues == want, f"Cues in der Datenbank {db_cues} = {want}")
        hot = [e for e in dat.getall_tags("PCOB")[1].content.entries]
        check(sorted(e.hot_cue for e in hot) == sorted(q["kind"] for q in t["cues"] if q["kind"] > 0), "Hot Cues in ANLZ")

pl = db.get_playlist(name=exp["playlist"]).first()
check(pl is not None, f"Playlist „{exp['playlist']}“ vorhanden")
if pl is not None:
    items = sorted(db.get_playlist_content(playlist_id=pl.playlist_id), key=lambda x: x.sequenceNo)
    titles = [db.get_content(content_id=i.content_id).title for i in items]
    check(titles == exp["order"], f"Reihenfolge {titles}")

xml = RekordboxXml(work / "rekordbox.xml")
check(xml.num_tracks == len(exp["tracks"]), f"XML: {xml.num_tracks} Titel")
for t in exp["tracks"]:
    if t["bpm"]:
        tr = next(x for x in xml.get_tracks() if x.Name == t["title"])
        check(abs(tr.tempos[0].Bpm - t["bpm"]) < 0.005, f"XML-Tempo {t['title']}")
check(len(xml.get_playlist(exp["playlist"]).get_tracks()) == len(exp["order"]), "XML-Playlist")

if fails:
    print(f"\n{len(fails)} Prüfungen fehlgeschlagen")
    sys.exit(1)
print("\nAlle Prüfungen bestanden")
