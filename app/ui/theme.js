// Applies the look chosen in Settings before the window first paints: light or dark, and
// whether things move. Both follow the Mac unless you pick otherwise.
(() => {
  const root = document.documentElement;
  const light = matchMedia("(prefers-color-scheme: light)");
  const reduce = matchMedia("(prefers-reduced-motion: reduce)");
  const pref = (name, fallback) => {
    try {
      return localStorage.getItem(`voxen.${name}`) || fallback;
    } catch {
      return fallback;
    }
  };
  window.applyLook = () => {
    const theme = pref("theme", "system");
    root.classList.toggle("light", theme === "light" || (theme === "system" && light.matches));
    root.classList.toggle("still", pref("motion", "full") === "reduced" || reduce.matches);
    window.dispatchEvent(new Event("look"));
  };
  light.addEventListener("change", window.applyLook);
  reduce.addEventListener("change", window.applyLook);
  window.applyLook();
})();
