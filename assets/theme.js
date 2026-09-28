// Runs in the page head, before anything is drawn: themes that follow the
// system switch to dark mode with it, without a flash of the wrong one.
"use strict";

(() => {
  const root = document.documentElement;
  if (root.dataset.appearance !== "system") return;
  const dark = matchMedia("(prefers-color-scheme: dark)");
  const apply = () => root.classList.toggle("dark", dark.matches);
  apply();
  dark.addEventListener("change", apply);
})();
