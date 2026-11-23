// Cover groß ansehen: fliegt aus dem kleinen Bild in die Bildmitte und neigt sich mit der Maus (wie bei Spotify).
// Klick oder Esc: fliegt zurück.
import { api } from "./api";

let open = false;

export async function zoomCover(from: HTMLElement, trackId: number, src?: string) {
  if (open) return;
  open = true;
  const url = (await api.getCover(trackId).catch(() => null)) ?? src;
  if (!url) {
    open = false;
    return;
  }
  const r0 = from.getBoundingClientRect();
  const back = document.createElement("div");
  back.className = "cz-back";
  const stage = document.createElement("div");
  stage.className = "cz-stage";
  const card = document.createElement("div");
  card.className = "cz-card";
  const img = document.createElement("img");
  img.src = url;
  img.alt = "";
  img.draggable = false;
  const sheen = document.createElement("div");
  sheen.className = "cz-sheen";
  card.append(img, sheen);
  stage.append(card);
  document.body.append(back, stage);
  await img.decode().catch(() => {});

  // Zielgröße: so groß wie gut passt, quadratisch
  const size = Math.min(window.innerWidth * 0.72, window.innerHeight * 0.72, 900);
  const tx = (window.innerWidth - size) / 2;
  const ty = (window.innerHeight - size) / 2;
  // Start: genau über dem kleinen Cover
  const startT = `translate(${r0.left - tx}px, ${r0.top - ty}px) scale(${r0.width / size}, ${r0.height / size})`;
  Object.assign(card.style, { left: tx + "px", top: ty + "px", width: size + "px", height: size + "px", transform: startT });
  from.style.visibility = "hidden";
  requestAnimationFrame(() => {
    back.classList.add("on");
    card.classList.add("fly");
    card.style.transform = "translate(0,0) scale(1)";
  });

  // Neigen mit der Maus – weich nachgeführt
  let rx = 0;
  let ry = 0;
  let wantX = 0;
  let wantY = 0;
  let raf = 0;
  let flying = true;
  setTimeout(() => {
    flying = false;
    card.classList.remove("fly");
    card.style.transformOrigin = "50% 50%";
  }, 520);
  const tick = () => {
    rx += (wantX - rx) * 0.12;
    ry += (wantY - ry) * 0.12;
    if (!flying) card.style.transform = `perspective(1400px) rotateX(${rx}deg) rotateY(${ry}deg)`;
    sheen.style.background = `radial-gradient(circle at ${50 + ry * 3}% ${50 - rx * 3}%, rgba(255,255,255,0.22), rgba(255,255,255,0) 55%)`;
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
  const move = (e: PointerEvent) => {
    const cx = window.innerWidth / 2;
    const cy = window.innerHeight / 2;
    wantY = Math.max(-1, Math.min(1, (e.clientX - cx) / cx)) * 14;
    wantX = -Math.max(-1, Math.min(1, (e.clientY - cy) / cy)) * 14;
  };
  window.addEventListener("pointermove", move);

  const close = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("keydown", key, true);
    stage.removeEventListener("click", close);
    back.removeEventListener("click", close);
    cancelAnimationFrame(raf);
    const r1 = from.isConnected ? from.getBoundingClientRect() : r0;
    card.classList.add("fly");
    card.style.transformOrigin = "0 0";
    card.style.transform = `translate(${r1.left - tx}px, ${r1.top - ty}px) scale(${r1.width / size}, ${r1.height / size})`;
    back.classList.remove("on");
    setTimeout(() => {
      back.remove();
      stage.remove();
      from.style.visibility = "";
      open = false;
    }, 480);
  };
  const key = (e: KeyboardEvent) => {
    if (e.key === "Escape" || e.key === " " || e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      close();
    }
  };
  window.addEventListener("keydown", key, true);
  stage.addEventListener("click", close);
  back.addEventListener("click", close);
}
