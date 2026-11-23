// Browser-Ersatz für @tauri-apps/api/webview: Dateien vom eigenen Rechner hineinziehen gibt es am NAS nicht.
export function getCurrentWebview() {
  return { async onDragDropEvent(_cb: unknown) { return () => {}; } };
}
