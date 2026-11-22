#!/usr/bin/env python3
"""Erzeugt kurze WAV-Testtitel mit bekanntem Tempo und Downbeat (nur Standardbibliothek)."""
import math, struct, sys, wave
from pathlib import Path

def track(path, bpm, offset_ms, secs=40, rate=44100):
    n = int(secs * rate)
    x = [0.0] * n
    beat = 60.0 / bpm
    k = 0
    while True:
        t = offset_ms / 1000 + k * beat
        i0 = int(t * rate)
        if i0 >= n:
            break
        down = k % 4 == 0
        for j in range(int(rate * 0.06)):
            if i0 + j >= n:
                break
            tt = j / rate
            env = math.exp(-tt * 40)
            v = math.sin(2 * math.pi * 55 * tt) * 0.8 if down else math.sin(2 * math.pi * 1500 * tt) * 0.3
            x[i0 + j] += v * env
        k += 1
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"".join(struct.pack("<h", int(max(-1, min(1, v)) * 32000)) for v in x))

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
track(out / "A Kick 128.wav", 128.0, 250.0)
track(out / "B Kick 100.wav", 100.0, 900.0)
track(out / "C Kick 140.wav", 140.0, 60.0)
print("ok")
