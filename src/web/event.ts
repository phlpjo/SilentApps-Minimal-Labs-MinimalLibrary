// Browser-Ersatz für @tauri-apps/api/event: Ereignisse vom Server kommen per Server-Sent Events.
// Eigene Ereignisse (emit) bleiben in diesem Browser-Fenster – andere Nutzer werden nicht gestört.
export type UnlistenFn = () => void;
export interface Event<T> { event: string; id: number; payload: T }
type Cb = (e: Event<unknown>) => void;

const handlers = new Map<string, Set<Cb>>();
let es: EventSource | null = null;

function connect() {
  if (es) return;
  es = new EventSource("/api/events", { withCredentials: true });
  es.onmessage = (m) => {
    try {
      const { event, payload } = JSON.parse(m.data);
      dispatch(event, payload);
    } catch {
      /* Lebenszeichen */
    }
  };
  es.onerror = () => {
    // Verbindung weg (NAS neu gestartet?) – nach kurzer Pause neu verbinden
    es?.close();
    es = null;
    setTimeout(connect, 3000);
  };
}

function dispatch(event: string, payload: unknown) {
  handlers.get(event)?.forEach((cb) => {
    try {
      cb({ event, id: 0, payload });
    } catch (e) {
      console.error(e);
    }
  });
}

export async function listen<T>(event: string, cb: (e: Event<T>) => void): Promise<UnlistenFn> {
  connect();
  if (!handlers.has(event)) handlers.set(event, new Set());
  const set = handlers.get(event)!;
  set.add(cb as Cb);
  return () => set.delete(cb as Cb);
}

export async function once<T>(event: string, cb: (e: Event<T>) => void): Promise<UnlistenFn> {
  const un = await listen<T>(event, (e) => {
    un();
    cb(e);
  });
  return un;
}

export async function emit(event: string, payload?: unknown): Promise<void> {
  dispatch(event, payload);
}

export async function emitTo(_target: unknown, event: string, payload?: unknown): Promise<void> {
  dispatch(event, payload);
}
