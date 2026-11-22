// Linien-Symbole (Lucide-Stil), feste Größe – Knöpfe ändern beim Umschalten ihre Breite nicht
const PATHS: Record<string, string> = {
  play: '<path d="M7 4.5v15a.8.8 0 0 0 1.2.7l12-7.5a.8.8 0 0 0 0-1.4l-12-7.5A.8.8 0 0 0 7 4.5z" fill="currentColor"/>',
  pause: '<rect x="6" y="4.5" width="4" height="15" rx="1" fill="currentColor"/><rect x="14" y="4.5" width="4" height="15" rx="1" fill="currentColor"/>',
  next: '<path d="M5 5v14l10-7z" fill="currentColor"/><path d="M19 5v14"/>',
  prev: '<path d="M19 5v14L9 12z" fill="currentColor"/><path d="M5 5v14"/>',
  shuffle: '<path d="M3 7h3.5c2 0 3.2 1 4.3 2.6l2.4 4.8C14.3 16 15.5 17 17.5 17H21"/><path d="M3 17h3.5c1.2 0 2.1-.4 2.9-1.1"/><path d="M14.6 8.1c.8-.7 1.7-1.1 2.9-1.1H21"/><path d="m18 4 3 3-3 3"/><path d="m18 14 3 3-3 3"/>',
  repeat: '<path d="m17 2 4 4-4 4"/><path d="M3 11v-1a4 4 0 0 1 4-4h14"/><path d="m7 22-4-4 4-4"/><path d="M21 13v1a4 4 0 0 1-4 4H3"/>',
  speaker: '<path d="M11 4.7a.7.7 0 0 0-1.2-.5L6.4 7.6A1.4 1.4 0 0 1 5.4 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.4a1.4 1.4 0 0 1 1 .4l3.4 3.4a.7.7 0 0 0 1.2-.5z" fill="currentColor"/><path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.4 18.4a9 9 0 0 0 0-12.8"/>',
  star: '<path d="M12 3.2l2.6 5.4 5.9.8-4.3 4.1 1 5.9L12 16.6l-5.2 2.8 1-5.9-4.3-4.1 5.9-.8z"/>',
  starOn: '<path d="M12 3.2l2.6 5.4 5.9.8-4.3 4.1 1 5.9L12 16.6l-5.2 2.8 1-5.9-4.3-4.1 5.9-.8z" fill="currentColor"/>',
  lyrics: '<path d="M4 6h16"/><path d="M4 10h11"/><path d="M4 14h16"/><path d="M4 18h8"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>',
  moon: '<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z"/>',
  repeat1: '<path d="m17 2 4 4-4 4"/><path d="M3 11v-1a4 4 0 0 1 4-4h14"/><path d="m7 22-4-4 4-4"/><path d="M21 13v1a4 4 0 0 1-4 4H3"/><path d="M11 10h1v4"/>',
  panelLeft: '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M9 4.5v15"/>',
  panelRight: '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M15 4.5v15"/>',
};

export function icon(name: keyof typeof PATHS | string, size = 20): SVGSVGElement {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.8");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.innerHTML = PATHS[name] ?? "";
  return svg;
}

// Menüsymbole im Kreis – gleicher Stil wie das Logo (Ring + einfache Linien)
const NAV: Record<string, string> = {
  library: '<path d="M9.6 8.6v6.8"/><path d="M12 8.6v6.8"/><path d="M13.9 9.2l1.8 6"/>',
  search: '<circle cx="11.3" cy="11.3" r="2.9"/><path d="M13.4 13.4 16 16"/>',
  folders: '<path d="M7.6 9.2h3l1 1.2h4.8v5.2H7.6z"/>',
  sync: '<path d="M8.6 11.4a3.5 3.5 0 0 1 6.1-2.3"/><path d="M14.9 7.6v1.8h-1.8"/><path d="M15.4 12.6a3.5 3.5 0 0 1-6.1 2.3"/><path d="M9.1 16.4v-1.8h1.8"/>',
  equalizer: '<path d="M9 8v8"/><path d="M12 8v8"/><path d="M15 8v8"/><path d="M8 13.5h2"/><path d="M11 10h2"/><path d="M14 14.5h2"/>',
  ripper: '<path d="M12 7.6v6"/><path d="M9.6 11.2 12 13.6l2.4-2.4"/><path d="M8.6 16.2h6.8"/>',
  organize: '<path d="M8.2 9h7.6"/><path d="M8.2 12h5.2"/><path d="M8.2 15h2.8"/>',
  duplicates: '<rect x="7.8" y="10" width="5" height="6" rx="0.6"/><path d="M10.8 8h4.8a.6.6 0 0 1 .6.6v5.4"/>',
  history: '<path d="M12 8.2V12l2.8 1.8"/>',
  settings: '<path d="M8 9.6h8"/><path d="M8 14.4h8"/><circle cx="10.4" cy="9.6" r="1.3"/><circle cx="13.6" cy="14.4" r="1.3"/>',
  playlists: '<path d="M8 9h6"/><path d="M8 12h6"/><path d="M8 15h3.4"/><path d="M14 13.6v3l2.3-1.5z"/>',
  playlist: '<path d="M8.4 9.4h7.2"/><path d="M8.4 12h7.2"/><path d="M8.4 14.6h4.4"/>',
  smart: '<path d="M12 8l4 4-4 4-4-4z"/>',
  star: '<path d="M12 8.2l1.2 2.4 2.6.4-1.9 1.8.5 2.6L12 14.2l-2.3 1.2.5-2.6-1.9-1.8 2.6-.4z"/>',
};

/** Symbol im Kreis für Menüpunkte (wie das App-Logo) */
export function navIcon(name: string, size = 20): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.6");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.classList.add("nicon");
  svg.innerHTML = '<circle cx="12" cy="12" r="10.2" stroke-width="1.8"/>' + (NAV[name] ?? "");
  return svg;
}

