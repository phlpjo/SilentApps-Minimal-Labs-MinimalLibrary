// Hell/Dunkel: "system" folgt Windows, sonst fest per Schalter
export type ThemeMode = "system" | "light" | "dark";
const mq = window.matchMedia("(prefers-color-scheme: light)");

export function themeMode(): ThemeMode {
  return (localStorage.getItem("ml.theme") as ThemeMode) ?? "system";
}

export function isLight(): boolean {
  const m = themeMode();
  return m === "light" || (m === "system" && mq.matches);
}

export function applyTheme(mode: ThemeMode = themeMode()) {
  localStorage.setItem("ml.theme", mode);
  if (mode === "system") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.setAttribute("data-theme", mode);
  document.documentElement.classList.toggle("light", isLight());
  document.dispatchEvent(new Event("themechange"));
}

mq.addEventListener("change", () => themeMode() === "system" && applyTheme("system"));
applyTheme();
