// Start im Browser (NAS): erst anmelden, dann die normale Oberfläche laden.
import "../style.css";
document.documentElement.classList.add("web");

async function auth(): Promise<{ setup: boolean; loggedIn: boolean }> {
  const r = await fetch("/api/auth", { credentials: "same-origin" });
  return r.json();
}

function loginForm(setup: boolean): Promise<void> {
  const de = (navigator.language || "de").toLowerCase().startsWith("de");
  const t = (d: string, e: string) => (de ? d : e);
  return new Promise((resolve) => {
    const app = document.getElementById("app")!;
    app.innerHTML = "";
    const box = document.createElement("form");
    box.className = "web-login";
    box.innerHTML = `
      <div class="wl-title">Minimal Library</div>
      <div class="wl-sub">${setup ? t("Lege ein Passwort fest – damit meldest du dich künftig im Browser an.", "Choose a password – you will sign in with it from now on.") : t("Anmelden", "Sign in")}</div>
      <input type="password" autocomplete="${setup ? "new-password" : "current-password"}" placeholder="${t("Passwort", "Password")}" autofocus>
      ${setup ? `<input type="password" autocomplete="new-password" placeholder="${t("Passwort wiederholen", "Repeat password")}">` : ""}
      <button type="submit">${setup ? t("Festlegen", "Set") : t("Anmelden", "Sign in")}</button>
      <div class="wl-err"></div>`;
    app.appendChild(box);
    const inputs = box.querySelectorAll("input");
    const err = box.querySelector(".wl-err") as HTMLElement;
    box.addEventListener("submit", async (e) => {
      e.preventDefault();
      const pw = inputs[0].value;
      if (setup && pw !== inputs[1].value) {
        err.textContent = t("Die Passwörter sind verschieden.", "Passwords differ.");
        return;
      }
      const r = await fetch("/api/login", { method: "POST", credentials: "same-origin", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ password: pw }) });
      if (r.ok) {
        app.innerHTML = "";
        resolve();
      } else {
        const b = await r.json().catch(() => ({}));
        err.textContent = b.error ?? t("Anmeldung fehlgeschlagen", "Sign-in failed");
      }
    });
  });
}

(async () => {
  try {
    const a = await auth();
    if (!a.loggedIn) await loginForm(a.setup);
  } catch {
    document.getElementById("app")!.textContent = "NAS nicht erreichbar";
    return;
  }
  await import("../main");
})();
