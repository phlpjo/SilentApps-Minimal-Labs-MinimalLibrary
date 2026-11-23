// Vollbild („Bühne“): nur Sternenhimmel, Künstler, Titel und der Songtext in der Mitte.
// Der Text fließt von unten nach oben, die aktuelle Zeile ist groß und hell, die anderen werden kleiner und blasser.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "./api";
import { L } from "./i18n";
import { currentLine, parseLrc, type Line } from "./lyrics";
import { player } from "./player";
import { starfield } from "./stars";
import { store } from "./store";
import { h } from "./ui";

let open = false;

const ease = (x: number) => (x <= 0 ? 0 : x >= 1 ? 1 : x < 0.5 ? 2 * x * x : 1 - Math.pow(-2 * x + 2, 2) / 2);

export function openStage() {
  if (open) return;
  open = true;
  const artist = h("div", { class: "stage-artist" });
  const title = h("div", { class: "stage-title" });
  const lyr = h("div", { class: "stage-lyrics" });
  const close = h("button", { class: "stage-close", title: L("Vollbild beenden (Esc)"), onclick: () => shut() }, "✕");
  const box = h("div", { class: "stage" }, h("div", { class: "stage-head" }, artist, title), lyr, close);
  document.body.append(box);
  const sky = starfield(box);
  getCurrentWindow().setFullscreen(true).catch(() => {});
  requestAnimationFrame(() => box.classList.add("on"));

  let forId = -2;
  let lines: Line[] = [];
  let synced = false;
  let els: HTMLElement[] = [];
  let lastI = -1;

  const load = async () => {
    const t = player.current;
    forId = t?.id ?? -1;
    box.classList.add("swap");
    const l = t ? await api.lyricsGet(t.id).catch(() => null) : null;
    if ((player.current?.id ?? -1) !== forId) return;
    lines = l ? parseLrc(l.text).filter((x) => x.text.trim() || x.t != null) : [];
    synced = lines.some((x) => x.t != null);
    artist.textContent = t?.artist || "";
    title.textContent = t ? store.label(t) : L("Nichts läuft");
    els = lines.map((x) => h("div", { class: "stage-line" }, x.text.trim() || "♪"));
    lyr.replaceChildren(...els);
    box.classList.toggle("nolyrics", !lines.length);
    lastI = -1;
    setTimeout(() => box.classList.remove("swap"), 50);
  };

  // Position in „Zeilen“: steht still auf der aktuellen Zeile und gleitet kurz vor der nächsten weich weiter
  const position = (): number => {
    if (!lines.length) return 0;
    const t = player.currentTime;
    if (!synced) return (player.duration > 0 ? t / player.duration : 0) * (lines.length - 1);
    const i = Math.max(0, currentLine(lines, t));
    let next: number | null = null;
    for (let k = i + 1; k < lines.length; k++) if (lines[k].t != null) { next = lines[k].t!; break; }
    if (next == null) return i;
    const glide = Math.min(0.7, Math.max(0.2, (next - (lines[i].t ?? 0)) * 0.35));
    return i + ease((t - (next - glide)) / glide);
  };

  let raf = 0;
  const frame = () => {
    if (!open) return;
    if ((player.current?.id ?? -1) !== forId) load();
    const pos = position();
    const lh = Math.max(46, window.innerHeight / 11);
    for (let k = 0; k < els.length; k++) {
      const d = k - pos;
      const ad = Math.abs(d);
      if (ad > 7) { els[k].style.opacity = "0"; continue; }
      const scale = Math.max(0.62, 1.32 - ad * 0.2);
      els[k].style.transform = `translate(-50%, ${d * lh}px) scale(${scale})`;
      els[k].style.opacity = String(Math.max(0, 1 - ad * 0.27));
    }
    const cur = Math.round(pos);
    if (cur !== lastI) {
      els[lastI]?.classList.remove("on");
      els[cur]?.classList.add("on");
      if (lastI >= 0) sky.kick(400); // kleiner Warp beim Zeilenwechsel
      lastI = cur;
    }
    raf = requestAnimationFrame(frame);
  };
  raf = requestAnimationFrame(frame);

  // Maus ruhig → Zeiger und ✕ verschwinden
  let idle = 0;
  const wake = () => {
    box.classList.add("awake");
    clearTimeout(idle);
    idle = window.setTimeout(() => box.classList.remove("awake"), 2200);
  };
  const key = (e: KeyboardEvent) => {
    if (e.key === "Escape") shut();
    else if (e.key === " ") { e.preventDefault(); player.toggle(); }
    else if (e.key === "ArrowRight") player.next();
    else if (e.key === "ArrowLeft") player.prev();
    else return;
    e.stopPropagation();
  };
  box.addEventListener("mousemove", wake);
  window.addEventListener("keydown", key, true);
  const unsubLyr = store.on("lyrics", () => { forId = -2; });

  function shut() {
    if (!open) return;
    open = false;
    cancelAnimationFrame(raf);
    clearTimeout(idle);
    window.removeEventListener("keydown", key, true);
    if (typeof unsubLyr === "function") unsubLyr();
    getCurrentWindow().setFullscreen(false).catch(() => {});
    box.classList.remove("on");
    setTimeout(() => { sky.destroy(); box.remove(); }, 800);
  }
  wake();
  load();
}
