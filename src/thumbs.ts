// Kleine Cover für die Titellisten: aus dem eingebetteten Bild verkleinert und im Speicher gehalten.
import { api } from "./api";

const SIZE = 72; // Pixel (für scharfe Darstellung auf 36 px)
const MAX = 4000;
const cache = new Map<number, string | null>();
const waiting = new Map<number, ((u: string | null) => void)[]>();
const queue: number[] = [];
let busy = 0;

function shrink(src: string): Promise<string | null> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      const c = document.createElement("canvas");
      c.width = c.height = SIZE;
      const g = c.getContext("2d");
      if (!g) return resolve(src);
      // mittig zuschneiden (wie object-fit: cover)
      const s = Math.min(img.width, img.height);
      g.drawImage(img, (img.width - s) / 2, (img.height - s) / 2, s, s, 0, 0, SIZE, SIZE);
      resolve(c.toDataURL("image/jpeg", 0.82));
    };
    img.onerror = () => resolve(null);
    img.src = src;
  });
}

function pump() {
  while (busy < 3 && queue.length) {
    const id = queue.pop()!; // zuletzt angefragt = gerade sichtbar → zuerst
    if (!waiting.has(id)) continue;
    busy++;
    api
      .getCover(id)
      .then((u) => (u ? shrink(u) : null))
      .catch(() => null)
      .then((u) => {
        if (cache.size > MAX) cache.delete(cache.keys().next().value!);
        cache.set(id, u);
        const cbs = waiting.get(id) ?? [];
        waiting.delete(id);
        cbs.forEach((f) => f(u));
      })
      .finally(() => {
        busy--;
        pump();
      });
  }
}

/** Cover-Vorschau eines Titels (Rückruf sofort, wenn schon geladen) */
export function thumb(id: number, cb: (url: string | null) => void) {
  if (cache.has(id)) return cb(cache.get(id)!);
  const w = waiting.get(id);
  if (w) return void w.push(cb);
  waiting.set(id, [cb]);
  queue.push(id);
  pump();
}

/** Nach Cover-Änderung neu laden */
export function forgetThumb(ids: number[]) {
  ids.forEach((id) => cache.delete(id));
}
