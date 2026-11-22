// CI: Versionsnummer setzen; ohne Signier-Schlüssel keine Update-Dateien bauen (sonst bricht der Build ab)
import { readFileSync, writeFileSync } from "node:fs";

const f = "src-tauri/tauri.conf.json";
const c = JSON.parse(readFileSync(f, "utf8"));
c.version = process.env.VERSION || c.version;
if (!process.env.TAURI_SIGNING_PRIVATE_KEY) {
  c.bundle.createUpdaterArtifacts = false;
  console.log("::warning::Secret TAURI_SIGNING_PRIVATE_KEY fehlt – Build ohne Auto-Update-Dateien, kein Release.");
}
writeFileSync(f, JSON.stringify(c, null, 2) + "\n");
console.log("Version", c.version, "· Update-Dateien:", c.bundle.createUpdaterArtifacts !== false);
