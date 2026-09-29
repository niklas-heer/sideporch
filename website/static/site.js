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
    // Outside the scrolling block, so it stays in its corner.
    const box = document.createElement("div");
    box.className = "code";
    pre.replaceWith(box);
    box.append(pre, button);
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

  // Diagrams: Mermaid loads only on pages that have one, and draws them in
  // the reader's light or dark colors.
  const diagrams = [...document.querySelectorAll("pre.mermaid")];
  if (diagrams.length > 0) {
    const script = document.createElement("script");
    script.src = "/vendor/mermaid.min.js";
    script.addEventListener("load", () => {
      const dark = matchMedia("(prefers-color-scheme: dark)").matches;
      globalThis.mermaid.initialize({
        startOnLoad: false,
        securityLevel: "strict",
        theme: dark ? "dark" : "neutral",
        fontFamily: getComputedStyle(document.body).fontFamily,
        themeVariables: { fontSize: "15px" },
      });
      globalThis.mermaid.run({ nodes: diagrams });
    });
    document.head.append(script);
  }

  // Search: Pagefind's index loads the first time search opens.
  const search = document.getElementById("search");
  if (search) {
    const input = document.getElementById("search-input");
    const results = document.getElementById("search-results");
    let pagefind;
    let run = 0;
    const load = async () => (pagefind ??= await import(search.dataset.pagefind));
    const unavailable = () => {
      // `zola serve` has no index; `mise run site-build` makes one.
      const note = document.createElement("li");
      note.className = "empty";
      note.textContent = "Search isn't available here: the search index is made by a full build of the site.";
      results.replaceChildren(note);
    };
    const open = () => {
      if (!search.open) search.showModal();
      input.focus();
      input.select();
      load().catch(() => {});
    };
    for (const button of document.querySelectorAll("[data-search-open]")) {
      button.hidden = false;
      button.addEventListener("click", open);
    }
    document.addEventListener("keydown", (event) => {
      const target = event.target;
      const typing = target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));
      const slash = event.key === "/" && !typing && !event.metaKey && !event.ctrlKey && !event.altKey;
      const k = event.key.toLowerCase() === "k" && (event.metaKey || event.ctrlKey);
      if (slash || k) {
        event.preventDefault();
        open();
      }
    });
    // A click on the dimmed page around the dialog closes it.
    search.addEventListener("click", (event) => {
      if (event.target === search) search.close();
    });
    const item = (result) => {
      // Link to the section of the page with the most matches.
      const best = (result.sub_results ?? []).reduce(
        (top, sub) => (sub.locations.length > (top?.locations.length ?? 0) ? sub : top),
        null,
      );
      const section = best && best.title !== result.meta.title ? best : null;
      const li = document.createElement("li");
      const a = document.createElement("a");
      a.href = section?.url ?? result.url;
      const title = document.createElement("strong");
      title.textContent = section ? `${result.meta.title} › ${section.title}` : result.meta.title;
      const excerpt = document.createElement("span");
      // Pagefind escapes the page text and only adds <mark> around matches.
      excerpt.innerHTML = section?.excerpt ?? result.excerpt;
      a.append(title, excerpt);
      li.append(a);
      return li;
    };
    input.addEventListener("input", async () => {
      const mine = ++run;
      const query = input.value.trim();
      if (!query) {
        results.replaceChildren();
        return;
      }
      try {
        await load();
      } catch {
        if (mine === run) unavailable();
        return;
      }
      // Keys typed while the index loaded resume here out of order; only
      // the newest may search, or an older one would cancel it.
      if (mine !== run) return;
      const found = await pagefind.debouncedSearch(query, {}, 120);
      if (found === null || mine !== run) return;
      const data = await Promise.all(found.results.slice(0, 8).map((result) => result.data()));
      if (mine !== run) return;
      if (data.length) {
        results.replaceChildren(...data.map(item));
      } else {
        const empty = document.createElement("li");
        empty.className = "empty";
        empty.textContent = `Nothing found for “${query}”.`;
        results.replaceChildren(empty);
      }
    });
    // Enter opens the first result; the arrow keys move through them.
    search.addEventListener("keydown", (event) => {
      const links = [...results.querySelectorAll("a")];
      const at = links.indexOf(document.activeElement);
      if (event.key === "Escape") {
        // A search field clears itself on the first Esc; close right away instead.
        event.preventDefault();
        search.close();
      } else if (event.key === "Enter" && event.target === input) {
        event.preventDefault();
        links[0]?.click();
      } else if (event.key === "ArrowDown" && links.length) {
        event.preventDefault();
        links[Math.min(at + 1, links.length - 1)].focus();
      } else if (event.key === "ArrowUp" && at >= 0) {
        event.preventDefault();
        (at === 0 ? input : links[at - 1]).focus();
      }
    });
  }

  // Screenshots open large in a dialog. Leaving: the close button, Esc, a
  // click beside the image, or Back. Without this script, the link opens
  // the image itself.
  const viewer = document.getElementById("viewer");
  const zoomable = [...document.querySelectorAll("a[data-zoom]")];
  if (viewer && zoomable.length) {
    const stage = viewer.querySelector(".viewer-stage");
    const image = stage.querySelector("img");
    const text = viewer.querySelector(".viewer-text");
    const count = viewer.querySelector(".viewer-count");
    const steps = viewer.querySelectorAll("[data-viewer-step]");
    const dark = matchMedia("(prefers-color-scheme: dark)");
    let index = 0;
    let opener = null;
    // A reload while the viewer was open leaves its history entry behind.
    if (history.state?.viewer) history.replaceState(null, "");

    const show = (at) => {
      index = (at + zoomable.length) % zoomable.length;
      const link = zoomable[index];
      viewer.classList.remove("actual");
      image.src = (dark.matches && link.dataset.zoomDark) || link.href;
      image.alt = link.querySelector("img")?.alt ?? "";
      text.textContent = link.closest("figure")?.querySelector("figcaption")?.textContent.trim() ?? "";
      count.textContent = zoomable.length > 1 ? `${index + 1} of ${zoomable.length}` : "";
      stage.scrollTo(0, 0);
    };
    const finish = () => {
      if (viewer.open) viewer.close();
      opener?.focus({ preventScroll: true });
    };
    // Opening adds one history entry, so Back closes the viewer. Every
    // other way of closing removes that entry too, leaving history as it
    // was, however many screenshots were looked at. Closing happens at
    // once, so a second click can't go back a second time.
    const close = () => {
      if (!viewer.open) return;
      const pushed = history.state?.viewer;
      finish();
      if (pushed) history.back();
    };
    const open = (at) => {
      opener = zoomable[at];
      show(at);
      steps.forEach((button) => (button.hidden = zoomable.length < 2));
      viewer.showModal();
      viewer.querySelector("[data-viewer-close]").focus();
    };
    // Back closes the viewer; Forward opens it again where it was.
    addEventListener("popstate", () => {
      if (viewer.open) finish();
      else if (history.state?.viewer) open(history.state.index ?? 0);
    });
    zoomable.forEach((link, at) =>
      link.addEventListener("click", (event) => {
        // Keep opening in a new tab or window working.
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
        event.preventDefault();
        open(at);
        history.pushState({ viewer: true, index: at }, "");
      }),
    );
    viewer.addEventListener("cancel", (event) => {
      event.preventDefault();
      close();
    });
    viewer.querySelector("[data-viewer-close]").addEventListener("click", close);
    steps.forEach((button) => button.addEventListener("click", () => show(index + Number(button.dataset.viewerStep))));
    viewer.addEventListener("keydown", (event) => {
      if (event.key === "ArrowRight" && zoomable.length > 1) show(index + 1);
      if (event.key === "ArrowLeft" && zoomable.length > 1) show(index - 1);
    });
    // Beside the image (the dimmed backdrop, the stage around it) closes.
    viewer.addEventListener("click", (event) => {
      // Letting go of a drag beside the image isn't a click beside it.
      if (dragged) return;
      if (event.target === viewer || event.target === stage) close();
    });

    // Click the image to see it at full size, centred on where you
    // clicked; click again to fit it to the screen.
    let dragged = false;
    image.addEventListener("click", (event) => {
      if (dragged) return;
      const x = event.offsetX / image.clientWidth;
      const y = event.offsetY / image.clientHeight;
      viewer.classList.toggle("actual");
      if (viewer.classList.contains("actual")) {
        stage.scrollTo(x * image.scrollWidth - stage.clientWidth / 2, y * image.scrollHeight - stage.clientHeight / 2);
      }
    });
    // At full size, drag with the mouse to look around (touch scrolls).
    let drag = null;
    stage.addEventListener("pointerdown", (event) => {
      dragged = false;
      if (!viewer.classList.contains("actual") || event.pointerType !== "mouse" || event.button !== 0) return;
      drag = { x: event.clientX, y: event.clientY, left: stage.scrollLeft, top: stage.scrollTop };
      event.preventDefault();
    });
    addEventListener("pointermove", (event) => {
      if (!drag) return;
      const dx = event.clientX - drag.x;
      const dy = event.clientY - drag.y;
      if (Math.abs(dx) + Math.abs(dy) > 4) {
        dragged = true;
        viewer.classList.add("dragging");
      }
      stage.scrollTo(drag.left - dx, drag.top - dy);
    });
    addEventListener("pointerup", () => {
      drag = null;
      viewer.classList.remove("dragging");
    });
    // Swipe sideways between screenshots while they fit the screen.
    let touch = null;
    stage.addEventListener("touchstart", (event) => {
      touch = event.touches.length === 1 && !viewer.classList.contains("actual")
        ? { x: event.touches[0].clientX, y: event.touches[0].clientY }
        : null;
    }, { passive: true });
    stage.addEventListener("touchend", (event) => {
      if (!touch || zoomable.length < 2) return;
      const dx = event.changedTouches[0].clientX - touch.x;
      const dy = event.changedTouches[0].clientY - touch.y;
      touch = null;
      if (Math.abs(dx) > 60 && Math.abs(dx) > 2 * Math.abs(dy)) show(index + (dx < 0 ? 1 : -1));
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
