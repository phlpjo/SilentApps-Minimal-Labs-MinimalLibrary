// Am NAS kommen Updates über das Paketzentrum (bzw. ein neues .spk) – keine Prüfung in der App
export interface Update { version: string; body?: string; downloadAndInstall(cb?: unknown): Promise<void>; download(cb?: unknown): Promise<void>; install(): Promise<void> }
export async function check(_opts?: unknown): Promise<Update | null> {
  return null;
}
