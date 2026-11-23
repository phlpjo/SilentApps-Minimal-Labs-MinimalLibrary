// Browser-Ersatz für @tauri-apps/plugin-process
export async function relaunch() {
  location.reload();
}
export async function exit(_code?: number) {}
