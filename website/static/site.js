// The site's only script. Everything works without it except search:
// the hero conversation is simply all there, screenshot links open the
// image, and the docs menu shows above the page.
"use strict";

(() => {
  document.documentElement.classList.add("js");

  // The landing page's conversation arrives once, message by message.
  const talk = document.querySelector("[data-talk]");
  const typing = document.querySelector("[data-typing]");
  const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (talk && !calm) {
    const messages = [...talk.children];
    talk.classList.add("playing");
    let at = 400;
    messages.forEach((message, index) => {
      at += Number(message.dataset.delay || 0);
      if (index > 0 && typing) setTimeout(() => typing.classList.add("on"), at - 700);
      setTimeout(() => {
        typing?.classList.remove("on");
        message.classList.add("here");
      }, at);
    });
  }

  const copy = async (button, text) => {
    try {
      await navigator.clipboard.writeText(text);
      button.textContent = "Copied";
    } catch {
      button.textContent = "Select it";
    }
    setTimeout(() => (button.textContent = "Copy"), 2000);
  };
  for (const button of document.querySelectorAll("button[data-copy]")) {
    button.addEventListener("click", () => copy(button, button.dataset.copy));
  }

  // Copy buttons on the docs' code blocks.
  for (const pre of document.querySelectorAll(".prose pre")) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "copy-code";
    button.textContent = "Copy";
    button.addEventListener("click", () => copy(button, (pre.querySelector("code") ?? pre).innerText.trimEnd()));
    pre.append(button);
  }

  // Wide tables scroll inside their own box.
  for (const table of document.querySelectorAll(".prose > table")) {
    const box = document.createElement("div");
    box.className = "table-scroll";
    table.replaceWith(box);
    box.append(table);
  }

  // The docs menu becomes a drawer on narrow screens.
  const menu = document.querySelector(".docs-menu");
  const nav = document.getElementById("docs-nav");
  if (menu && nav) {
    menu.hidden = false;
    const set = (open) => {
      nav.classList.toggle("open", open);
      document.body.classList.toggle("drawer-open", open);
      menu.setAttribute("aria-expanded", String(open));
      // Focus once the drawer is visible; hidden elements can't take it.
      if (open) requestAnimationFrame(() => (nav.querySelector("[aria-current]") ?? nav.querySelector("a"))?.focus());
    };
    menu.addEventListener("click", () => set(!nav.classList.contains("open")));
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && nav.classList.contains("open")) {
        set(false);
        menu.focus();
      }
    });
    document.addEventListener("click", (event) => {
      if (nav.classList.contains("open") && !nav.contains(event.target) && !menu.contains(event.target)) set(false);
    });
  }

  // "On this page" marks the section being read.
  const toc = document.querySelector(".toc");
  if (toc) {
    const links = new Map([...toc.querySelectorAll("a")].map((a) => [decodeURIComponent(a.hash.slice(1)), a]));
    const headings = [...links.keys()].map((id) => document.getElementById(id)).filter(Boolean);
    const mark = () => {
      // The last heading above the upper third of the screen, or the last
      // one when the page can't scroll further.
      let current = headings[0];
      for (const heading of headings) {
        if (heading.getBoundingClientRect().top < innerHeight / 3) current = heading;
      }
      if (innerHeight + scrollY >= document.documentElement.scrollHeight - 2) current = headings.at(-1);
      for (const a of links.values()) a.removeAttribute("aria-current");
      links.get(current?.id)?.setAttribute("aria-current", "true");
    };
    let queued = false;
    addEventListener("scroll", () => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(() => {
        queued = false;
        mark();
      });
    }, { passive: true });
    mark();
  }
})();
