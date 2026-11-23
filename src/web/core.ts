// Browser-Ersatz für @tauri-apps/api/core: Befehle gehen per HTTP an den NAS-Server.
export async function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  const r = await fetch(`/api/invoke/${cmd}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args ?? {}),
    credentials: "same-origin",
  });
  if (r.status === 401) {
    location.reload();
    throw "Bitte anmelden";
  }
  const body = await r.json().catch(() => null);
  if (!r.ok) throw body ?? `Fehler ${r.status}`;
  return body as T;
}

/** Musikdatei über den Server abspielen (nur freigegebene Ordner) */
export function convertFileSrc(path: string): string {
  return `/api/file?p=${encodeURIComponent(path)}`;
}
