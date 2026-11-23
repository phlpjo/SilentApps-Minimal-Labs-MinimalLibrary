// Eigenes Drag & Drop über Zeiger-Events.
// HTML5-Drag&Drop funktioniert in Tauri unter Windows nicht, solange Datei-Drops aus dem Explorer aktiv sind.
import { copyKey } from "./platform";
import { L } from "./i18n";

export type Payload =
  | { kind: "ids"; ids: number[] }
  | { kind: "dir"; path: string }
  | { kind: "pl"; id: number; name: string }
  /** Titel aus einem anderen Tab (dorthin gezogen) – nur auf Ordner ablegbar */
  | { kind: "tab"; from: string; fromName: string; ids: number[]; names: Record<number, string> };

export interface DropTarget {
  accepts: (p: Payload) => boolean;
  /** Während des Überfahrens (z. B. Einfügeposition berechnen) */
  over?: (p: Payload, e: PointerEvent) => void;
  enter?: (p: Payload) => void;
  leave?: () => void;
  drop: (p: Payload, opts: { copy: boolean; e: PointerEvent }) => void;
}

const targets = new WeakMap<Element, DropTarget>();

export function dropTarget(el: HTMLElement, t: DropTarget) {
  targets.set(el, t);
  el.dataset.drop = "1";
}

let ghost: HTMLElement | null = null;
let active: { payload: Payload; el: HTMLElement | null; t: DropTarget | null } | null = null;
let scrollRaf = 0;
let lastEvent: PointerEvent | null = null;

/** Gerade am Ziehen? (z. B. um Klicks nach dem Loslassen zu ignorieren) */
export let dragging = false;
let cancelCurrent: (() => void) | null = null;

/** Laufendes Ziehen abbrechen (z. B. beim Wechsel in einen anderen Tab) */
export function cancelDrag() {
  cancelCurrent?.();
}

/**
 * Ziehen, das in einem anderen Tab begonnen hat: Symbol folgt der Maus, Loslassen oder Klick legt ab, Esc bricht ab.
 */
export function externalDrag(payload: Payload) {
  cancelCurrent?.();
  dragging = true;
  active = { payload, el: null, t: null };
  ghost = document.createElement("div");
  ghost.className = "drag-ghost";
  ghost.textContent = label(payload);
  document.body.appendChild(ghost);
  document.body.classList.add("is-dragging");
  autoScroll();
  const move = (e: PointerEvent) => {
    lastEvent = e;
    ghost!.style.transform = `translate(${e.clientX + 14}px, ${e.clientY + 10}px)`;
    const [el, t] = findTarget(e.clientX, e.clientY);
    if (el !== active!.el) {
      active!.el?.classList.remove("drop");
      active!.t?.leave?.();
      active!.el = el;
      active!.t = t;
      el?.classList.add("drop");
      t?.enter?.(active!.payload);
    }
    t?.over?.(active!.payload, e);
    ghost!.classList.toggle("none", !t);
  };
  const up = (e: PointerEvent) => {
    move(e);
    if (!active?.t || !active.el) return; // daneben: weiter ziehen, bis ein Ordner getroffen wird (Esc bricht ab)
    const { payload: p, t, el } = active;
    el.classList.remove("drop");
    t.drop(p, { copy: copyKey(e), e });
    t.leave?.();
    end();
  };
  const block = (e: Event) => { e.stopPropagation(); e.preventDefault(); };
  const key = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      active?.el?.classList.remove("drop");
      active?.t?.leave?.();
      end();
    }
  };
  function end() {
    cancelCurrent = null;
    window.removeEventListener("pointermove", move, true);
    window.removeEventListener("pointerup", up, true);
    window.removeEventListener("click", block, true);
    window.removeEventListener("keydown", key, true);
    cancelAnimationFrame(scrollRaf);
    ghost?.remove();
    ghost = null;
    active = null;
    lastEvent = null;
    document.body.classList.remove("is-dragging");
    setTimeout(() => {
      dragging = false;
      window.removeEventListener("click", block, true);
    }, 50);
  }
  cancelCurrent = end;
  window.addEventListener("pointermove", move, true);
  window.addEventListener("pointerup", up, true);
  window.addEventListener("click", block, true);
  window.addEventListener("keydown", key, true);
}

function findTarget(x: number, y: number): [HTMLElement | null, DropTarget | null] {
  let el = document.elementFromPoint(x, y) as HTMLElement | null;
  while (el) {
    if (el.dataset?.drop) {
      const t = targets.get(el);
      if (t && active && t.accepts(active.payload)) return [el, t];
    }
    el = el.parentElement;
  }
  return [null, null];
}

function label(p: Payload): string {
  if (p.kind === "dir") return L("Ordner: ") + (p.path.split(/[\\/]/).pop() ?? p.path);
  if (p.kind === "pl") return p.name;
  if (p.kind === "tab") return L("{0} Titel aus „{1}“ → Ordner wählen", p.ids.length, p.fromName);
  return p.ids.length === 1 ? L("1 Titel") : L("{0} Titel", p.ids.length);
}

// Liste/Baum am Rand automatisch scrollen
function autoScroll() {
  cancelAnimationFrame(scrollRaf);
  const step = () => {
    if (!active || !lastEvent) return;
    let el = document.elementFromPoint(lastEvent.clientX, lastEvent.clientY) as HTMLElement | null;
    while (el && !(el.scrollHeight > el.clientHeight + 2 && /(auto|scroll)/.test(getComputedStyle(el).overflowY))) el = el.parentElement;
    if (el) {
      const r = el.getBoundingClientRect();
      const y = lastEvent.clientY;
      const edge = 36;
      if (y < r.top + edge) el.scrollTop -= Math.ceil((r.top + edge - y) / 3);
      else if (y > r.bottom - edge) el.scrollTop += Math.ceil((y - (r.bottom - edge)) / 3);
    }
    scrollRaf = requestAnimationFrame(step);
  };
  scrollRaf = requestAnimationFrame(step);
}

/**
 * Bei pointerdown aufrufen. Ziehen startet erst nach 6 px Bewegung.
 * `onStart` liefert die Nutzlast (z. B. die aktuelle Auswahl). Gibt `true` an `onEnd` zurück, wenn gezogen wurde.
 */
export function beginDrag(down: PointerEvent, onStart: () => Payload | null, onEnd?: (dragged: boolean) => void) {
  if (down.button !== 0) return;
  const sx = down.clientX;
  const sy = down.clientY;
  let started = false;

  const move = (e: PointerEvent) => {
    lastEvent = e;
    if (!started) {
      if (Math.hypot(e.clientX - sx, e.clientY - sy) < 6) return;
      const payload = onStart();
      if (!payload) return cleanup(false);
      started = dragging = true;
      active = { payload, el: null, t: null };
      ghost = document.createElement("div");
      ghost.className = "drag-ghost";
      ghost.textContent = label(payload);
      document.body.appendChild(ghost);
      document.body.classList.add("is-dragging");
      autoScroll();
    }
    ghost!.style.transform = `translate(${e.clientX + 14}px, ${e.clientY + 10}px)`;
    const [el, t] = findTarget(e.clientX, e.clientY);
    if (el !== active!.el) {
      active!.el?.classList.remove("drop");
      active!.t?.leave?.();
      active!.el = el;
      active!.t = t;
      el?.classList.add("drop");
      t?.enter?.(active!.payload);
    }
    t?.over?.(active!.payload, e);
    const copy = copyKey(e) && active!.payload.kind === "ids";
    ghost!.textContent = label(active!.payload) + (t ? (copy ? L("  · kopieren") : "") : "");
    ghost!.classList.toggle("none", !t);
  };

  const up = (e: PointerEvent) => {
    if (started && active?.t && active.el) {
      const { payload, t, el } = active;
      el.classList.remove("drop");
      t.drop(payload, { copy: copyKey(e), e });
      t.leave?.();
    }
    cleanup(started);
  };

  const key = (e: KeyboardEvent) => {
    if (e.key === "Escape" && started) {
      active?.el?.classList.remove("drop");
      active?.t?.leave?.();
      cleanup(true);
    }
  };

  cancelCurrent = () => {
    active?.el?.classList.remove("drop");
    active?.t?.leave?.();
    cleanup(started);
  };

  function cleanup(wasDragging: boolean) {
    cancelCurrent = null;
    window.removeEventListener("pointermove", move, true);
    window.removeEventListener("pointerup", up, true);
    window.removeEventListener("keydown", key, true);
    cancelAnimationFrame(scrollRaf);
    ghost?.remove();
    ghost = null;
    active = null;
    lastEvent = null;
    document.body.classList.remove("is-dragging");
    // Klick direkt nach dem Loslassen nicht als Klick werten
    setTimeout(() => (dragging = false));
    onEnd?.(wasDragging);
  }

  window.addEventListener("pointermove", move, true);
  window.addEventListener("pointerup", up, true);
  window.addEventListener("keydown", key, true);
}
