// „Stern“ wie in Minimal Player: Lieblingstitel mit einem Klick in die Playlist „Stern“
import { api } from "./api";
import { store } from "./store";
import { toast } from "./ui";

let starred = new Set<number>();

export async function loadStars() {
  starred = new Set(await api.starIds().catch(() => []));
  store.emit("stars");
}

export const isStarred = (id: number) => starred.has(id);

export async function toggleStar(ids: number[]) {
  if (!ids.length) return;
  try {
    const on = await api.starToggle(ids);
    await loadStars();
    await store.loadPlaylists();
    toast(on ? (ids.length > 1 ? `★ ${ids.length} Titel markiert` : "★ markiert") : "Stern entfernt", 1800);
  } catch (e) {
    toast(String(e));
  }
}

// Playlisten geändert (auch vom Handy) → Sterne neu laden
store.on("playlists", () => { api.starIds().then((ids) => { starred = new Set(ids); store.emit("stars"); }).catch(() => {}); });
